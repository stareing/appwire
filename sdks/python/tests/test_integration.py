"""集成测试：启动 crates/native 的 fake_host，用真实 WebSocket 连接驱动工具调用。

需要 cargo。设置 ``APP_MCP_FAKE_HOST`` 可直接指定已构建的 fake_host 可执行文件。
"""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import subprocess
import threading
from pathlib import Path

import pytest

from app_mcp import AppMcp, ToolCallError, ToolContext, ToolResult

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
TARGET_DIR = Path(os.environ.get("CARGO_TARGET_DIR", Path(__file__).resolve().parents[3] / "target"))


@pytest.fixture(scope="session")
def fake_host_bin() -> Path:
    explicit = os.environ.get("APP_MCP_FAKE_HOST")
    if explicit:
        return Path(explicit)
    if shutil.which("cargo") is None:
        pytest.skip("没有 cargo，跳过集成测试")
    env = {**os.environ, "CARGO_TARGET_DIR": str(TARGET_DIR)}
    r = subprocess.run(
        ["cargo", "build", "-q", "-p", "app-mcp-native", "--example", "fake_host"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        pytest.skip(f"fake_host 构建失败：{r.stderr[-2000:]}")
    return TARGET_DIR / "debug" / "examples" / "fake_host"


def run_host(bin_path: Path, *args: str) -> tuple[subprocess.Popen[str], str]:
    proc = subprocess.Popen(
        [str(bin_path), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert proc.stdout is not None
    first = proc.stdout.readline().strip()
    assert first.startswith("LISTENING "), first
    return proc, first.split(" ", 1)[1]


def test_invoke_tools_via_fake_host(fake_host_bin: Path):
    proc, addr = run_host(
        fake_host_bin,
        "--invoke", "math.add", "--args", '{"a": 2, "b": 40}',
        "--invoke", "echo.async", "--args", '{"text": "你好"}',
        "--invoke", "cart.checkout", "--args", "{}",
        "--invoke", "boom",
        "--read", "cart",
        "--timeout-ms", "15000",
    )
    client = AppMcp(app_id="py-it", app_name="Python 集成测试", host_url=f"ws://{addr}", overview="测试 App")
    threads: list[str] = []

    @client.tool("math.add", description="两数相加", risk="read")
    def add(a: int, b: int, ctx: ToolContext) -> dict:
        threads.append(threading.current_thread().name)
        ctx.add_state_hint("cart")
        return {"sum": a + b}

    @client.tool("echo.async", description="异步回显")
    async def echo(text: str) -> str:
        await asyncio.sleep(0.01)
        return text

    @client.tool("cart.checkout", description="结账", risk="payment")
    def checkout() -> None:
        raise ToolCallError("USER_REJECTED", "用户取消了结账")

    @client.tool("boom", description="抛异常")
    def boom() -> None:
        raise RuntimeError("炸了")

    @client.resource("cart", description="购物车")
    def cart() -> dict:
        return {"items": ["A"]}

    try:
        client.start()
        out, err = proc.communicate(timeout=30)
    finally:
        client.close()
        if proc.poll() is None:
            proc.kill()
    assert proc.returncode == 0, f"fake_host 退出码 {proc.returncode}\nstdout:\n{out}\nstderr:\n{err}"

    lines = [json.loads(line) for line in out.splitlines() if line.strip()]
    catalog = lines[0]
    assert catalog["type"] == "tools"
    assert {"math.add", "echo.async", "cart.checkout", "boom"} <= set(map(str, catalog["tools"]))

    results = {l["name"]: l for l in lines[1:]}
    assert results["math.add"]["result"] == {"data": {"sum": 42}, "stateHints": ["cart"]}
    assert results["echo.async"]["result"]["data"] == "你好"
    assert results["cart.checkout"]["error"]["data"]["kind"] == "USER_REJECTED"
    assert results["cart.checkout"]["error"]["message"] == "用户取消了结账"
    assert results["boom"]["error"]["data"]["kind"] == "HANDLER_ERROR"
    assert results["cart"]["result"]["contents"] == {"items": ["A"]}
    assert threads and threads[0].startswith("app-mcp")


def test_tool_options_and_structured_result_reach_host(fake_host_bin: Path):
    """工具注解 + outputSchema 到达 Host；结构化结果原样回给 Host；普通返回值不变。"""
    proc, addr = run_host(
        fake_host_bin,
        "--tool-info",
        "--invoke", "order.submit",
        "--invoke", "plain",
        "--timeout-ms", "15000",
    )
    client = AppMcp(app_id="py-it", app_name="Python 集成测试", host_url=f"ws://{addr}")
    schema = {"type": "object", "properties": {"orderId": {"type": "string"}}}

    @client.tool(
        "order.submit",
        description="下单",
        annotations={"idempotent_hint": False, "open_world_hint": True},
        output_schema=schema,
    )
    def submit() -> ToolResult:
        return ToolResult(
            {"orderId": "o1"},
            status="pending",
            state_resource="order.state",
            summary="已提交，等待用户在 App 内付款",
            annotations={"audience": ["user"], "priority": 0.5},
        )

    @client.tool("plain", description="普通返回值", risk="read")
    def plain() -> dict:
        return {"ok": True}

    try:
        client.start()
        out, err = proc.communicate(timeout=30)
    finally:
        client.close()
        if proc.poll() is None:
            proc.kill()
    assert proc.returncode == 0, f"fake_host 退出码 {proc.returncode}\nstdout:\n{out}\nstderr:\n{err}"

    lines = [json.loads(line) for line in out.splitlines() if line.strip()]
    info = lines[0]["toolInfo"]
    assert info["order.submit"] == {
        "risk": "write",
        "annotations": {"idempotentHint": False, "openWorldHint": True},
        "outputSchema": schema,
    }
    assert info["plain"] == {"risk": "read"}
    results = {line["name"]: line for line in lines[1:]}
    assert results["order.submit"]["result"] == {
        "data": {"orderId": "o1"},
        "status": "pending",
        "stateResource": "order.state",
        "summary": "已提交，等待用户在 App 内付款",
        "annotations": {"audience": ["user"], "priority": 0.5},
    }
    assert results["plain"]["result"] == {"data": {"ok": True}}


def test_idle_sleep_wake_roundtrip(fake_host_bin: Path, caplog: pytest.LogCaptureFixture):
    """idle 休眠 → handle_wake 快速恢复（跳过 sync）→ 调用（含 details 错误、ctx.hold）→ 再休眠。"""
    from app_mcp import LifecyclePolicy, StateStatus, WakeDescriptor

    caplog.set_level("DEBUG", logger="app_mcp")
    proc, addr = run_host(
        fake_host_bin,
        "--await-sleep",
        "--wake",
        "--invoke", "lc.echo", "--args", '{"text": "醒了"}',
        "--invoke", "lc.validate", "--args", '{"qty": 0}',
        "--await-sleep",
        "--lease-ms", "100",
        "--timeout-ms", "20000",
    )
    assert proc.stdout is not None
    states: list[StateStatus] = []
    client = AppMcp(
        app_id="py-lifecycle",
        app_name="Python 生命周期测试",
        host_url=f"ws://{addr}",
        lifecycle=LifecyclePolicy(
            mode="idle",
            idle_timeout=0.3,
            wake=WakeDescriptor(kind="uri", target="pyshop", background=True),
        ),
        connect_timeout=2.0,
        on_state=lambda s: states.append(s.status),
    )
    held: list[bool] = []

    @client.tool("lc.echo", description="回显并短暂延长持有", risk="read")
    def echo(text: str, ctx: ToolContext) -> str:
        hold = ctx.hold()
        held.append(True)
        threading.Timer(0.2, hold.release).start()
        return text

    @client.tool("lc.validate", description="校验数量")
    def validate(qty: int) -> None:
        raise ToolCallError("INVALID_INPUT", "数量必须为正", details={"field": "qty", "min": 1})

    lines: list[dict] = []

    def next_line() -> dict:
        raw = proc.stdout.readline()
        assert raw, f"fake_host 提前结束：{proc.stderr.read() if proc.stderr else ''}"
        line = json.loads(raw)
        lines.append(line)
        return line

    try:
        hash_before = client.tools_hash
        assert len(hash_before) == 16
        client.start()
        assert next_line()["type"] == "tools"
        sleep1 = next_line()
        assert sleep1 == {"type": "sleep", "accepted": True, "reason": "idle", "toolsHash": hash_before}
        assert client.wait_for_state(StateStatus.DORMANT, timeout=5)

        wake = next_line()
        assert wake["type"] == "wake"
        # 列表参数：逐个识别
        assert client.handle_wake(["--verbose", wake["arg"]]) is True

        hello = next_line()
        assert hello["type"] == "hello"
        assert hello["launchToken"] == wake["token"]
        assert hello["wakeReason"] == "os-activation"
        assert hello["toolsCurrent"] is True
        tools = next_line()
        assert tools["type"] == "tools" and tools["synced"] is False

        echo_line = next_line()
        assert echo_line["name"] == "lc.echo" and echo_line["result"]["data"] == "醒了"
        err = next_line()
        assert err["name"] == "lc.validate"
        assert err["error"]["message"] == "数量必须为正"
        assert err["error"]["data"]["kind"] == "INVALID_INPUT"
        assert err["error"]["data"]["field"] == "qty"
        assert err["error"]["data"]["min"] == 1

        sleep2 = next_line()
        assert sleep2["type"] == "sleep" and sleep2["accepted"] is True and sleep2["reason"] == "idle"
        assert proc.wait(timeout=10) == 0
        assert client.wait_for_state(StateStatus.DORMANT, timeout=5)
    finally:
        client.close()
        if proc.poll() is None:
            proc.kill()
    assert held == [True]
    assert StateStatus.WAKING in states
    messages = "\n".join(r.getMessage() for r in caplog.records)
    assert "unknown notification" not in messages, messages
    print("\n".join(json.dumps(l, ensure_ascii=False) for l in lines))
