"""按名寻址 e2e（spec/naming.md，Linux）：私有 D-Bus 会话总线 + ``app-mcp-host app install`` 登记 +
``app-mcp-host stdio --name-service`` 作为 Hub；``examples/named_app.py``（``register_name=True``）由 D-Bus 激活冷启动。

发现不激活 → 调用触发激活 → 宽限后通道关闭、App 进程退出 → 再次调用再激活（新进程）。

需要 ``dbus-daemon`` 与 cargo（或用 ``APP_MCP_HOST_BIN`` 指定已构建的 app-mcp-host）；缺少时跳过。
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

import pytest

pytestmark = [
    pytest.mark.integration,
    pytest.mark.skipif(not sys.platform.startswith("linux"), reason="按名寻址 e2e 只在 Linux（D-Bus）上运行"),
]

ROOT = Path(__file__).resolve().parents[3]
SDK = Path(__file__).resolve().parents[1]
TARGET_DIR = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
APP = "py-named-e2e"
TIMEOUT = 20.0
GRACE_MS = 300

SESSION_CONF = """<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={socket}</listen>
  <auth>EXTERNAL</auth>
  <standard_session_servicedirs/>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"""


@pytest.fixture(scope="module")
def host_bin() -> Path:
    explicit = os.environ.get("APP_MCP_HOST_BIN")
    if explicit:
        return Path(explicit)
    if shutil.which("cargo") is None:
        pytest.skip("没有 cargo，跳过按名寻址 e2e")
    env = {**os.environ, "CARGO_TARGET_DIR": str(TARGET_DIR)}
    r = subprocess.run(["cargo", "build", "-q", "-p", "app-mcp-host"], cwd=ROOT, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        pytest.skip(f"app-mcp-host 构建失败：{r.stderr[-2000:]}")
    return TARGET_DIR / "debug" / "app-mcp-host"


class PrivateBus:
    """临时私有会话总线（与 crates/native 的 test_bus 相同的配置）；激活目录在临时 ``XDG_DATA_HOME`` 下。"""

    def __init__(self, root: Path, env: dict[str, str]) -> None:
        daemon = shutil.which("dbus-daemon")
        if daemon is None:
            pytest.skip("本机没有 dbus-daemon")
        self.data_home = root / "data"
        (self.data_home / "dbus-1" / "services").mkdir(parents=True)
        for d in ("sys", "run"):
            (root / d).mkdir()
        conf = root / "session.conf"
        conf.write_text(SESSION_CONF.format(socket=root / "bus"), encoding="utf-8")
        # @why 被激活的 App 继承总线进程的环境：去掉用户会话总线地址，避免误连用户总线。
        child_env = {k: v for k, v in os.environ.items() if not k.startswith("DBUS_")}
        child_env.update(env)
        child_env.update(XDG_DATA_HOME=str(self.data_home), XDG_DATA_DIRS=str(root / "sys"), XDG_RUNTIME_DIR=str(root / "run"))
        self.proc = subprocess.Popen(
            [daemon, f"--config-file={conf}", "--nofork", "--print-address=1"],
            env=child_env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            text=True,
        )
        assert self.proc.stdout is not None
        self.address = self.proc.stdout.readline().strip()
        assert self.address, "dbus-daemon 未打印地址"

    def close(self) -> None:
        self.proc.kill()
        self.proc.wait()


class StdioMcp:
    """``app-mcp-host stdio`` 的最小 MCP 客户端（换行分隔的 JSON-RPC）。"""

    def __init__(self, args: list[str], env: dict[str, str]) -> None:
        self.proc = subprocess.Popen(args, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self._responses: queue.Queue[dict[str, Any]] = queue.Queue()
        self._next_id = 0
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            if line.strip():
                msg = json.loads(line)
                if "id" in msg and ("result" in msg or "error" in msg):
                    self._responses.put(msg)

    def _send(self, msg: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self._next_id += 1
        self._send({"jsonrpc": "2.0", "id": self._next_id, "method": method, "params": params})
        msg = self._responses.get(timeout=TIMEOUT)
        assert msg["id"] == self._next_id, msg
        assert "error" not in msg, msg
        return msg["result"]

    def initialize(self) -> None:
        self.request(
            "initialize",
            {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "py-e2e", "version": "0"}},
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call(self, name: str, arguments: dict[str, Any]) -> Any:
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        assert not result.get("isError"), result
        if "structuredContent" in result:
            return result["structuredContent"]
        return json.loads(result["content"][0]["text"])

    def close(self) -> None:
        if self.proc.stdin:
            self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()


def kill_leftovers(log: Path) -> None:
    """失败路径清理：结束已启动、未记录退出的 App 进程（激活启动的进程不是本测试的子进程）。"""
    if not log.exists():
        return
    started: set[int] = set()
    for line in log.read_text(encoding="utf-8").splitlines():
        kind, _, pid = line.partition(" ")
        if kind == "start":
            started.add(int(pid))
        elif kind == "exit":
            started.discard(int(pid))
    for pid in started:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def eventually(what: str, pred) -> None:
    deadline = time.monotonic() + TIMEOUT
    while not pred():
        assert time.monotonic() < deadline, f"等待超时：{what}"
        time.sleep(0.02)


def test_python_app_is_activated_by_name(host_bin: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="app-mcp-py-naming-") as tmp:
        root = Path(tmp)
        log = root / "events.log"

        def count(kind: str) -> int:
            text = log.read_text(encoding="utf-8") if log.exists() else ""
            return sum(1 for line in text.splitlines() if line.startswith(kind + " "))

        app_env = {
            "APP_MCP_EVENT_LOG": str(log),
            "APP_MCP_APP_ID": APP,
            "PYTHONPATH": str(SDK / "src"),
        }
        bus = PrivateBus(root / "bus", app_env)
        try:
            # 激活时运行的程序：固定本测试的解释器运行 examples/named_app.py（D-Bus 追加 --app-mcp-activation）。
            launcher = root / "named_app"
            launcher.write_text(
                f"#!{sys.executable}\nimport runpy\nrunpy.run_path({str(SDK / 'examples' / 'named_app.py')!r}, run_name='__main__')\n",
                encoding="utf-8",
            )
            launcher.chmod(0o755)
            manifest = root / "app-mcp.json"
            manifest.write_text(
                json.dumps(
                    {
                        "manifestVersion": 1,
                        "appId": APP,
                        "name": "按名寻址 e2e（Python）",
                        "tools": [
                            {"name": "echo", "description": "原样返回 text", "inputSchema": {"type": "object"}},
                            {"name": "pid", "description": "返回进程号", "inputSchema": {"type": "object"}},
                        ],
                    }
                ),
                encoding="utf-8",
            )
            home = root / "home"
            env = {k: v for k, v in os.environ.items() if not k.startswith("DBUS_") and k != "APP_MCP_HOME"}
            env["DBUS_SESSION_BUS_ADDRESS"] = bus.address
            r = subprocess.run(
                [str(host_bin), "app", "install", "--app-id", APP, "--exec", str(launcher), "--manifest", str(manifest),
                 "--home", str(home), "--data-home", str(bus.data_home)],
                env=env, capture_output=True, text=True, timeout=TIMEOUT,
            )
            assert r.returncode == 0, r.stderr

            mcp = StdioMcp(
                [str(host_bin), "stdio", "--home", str(home), "--listen", "127.0.0.1:0", "--ipc-endpoint", "none",
                 "--waker", "none", "--name-service", "--channel-grace-ms", str(GRACE_MS), "--lease-ms", "0"],
                env,
            )
            try:
                mcp.initialize()

                # 1. 发现不激活：apps.list 中有可激活、未运行的名字记录，App 进程从未启动。
                def named_entry() -> dict[str, Any] | None:
                    apps = mcp.call("apps.list", {})["apps"]
                    entry = next((a.get("nameService") for a in apps if a["appId"] == APP), None)
                    return entry or None

                eventually("发现记录", lambda: named_entry() is not None)
                entry = named_entry()
                assert entry is not None and entry["activatable"] is True and entry["running"] is False, entry
                time.sleep(0.2)
                assert count("start") == 0, "发现不得启动 App"

                # 2. 调用触发激活冷启动。
                assert mcp.call(f"{APP}.echo", {"text": "你好"}) == {"echo": "你好"}
                assert count("start") == 1
                first_pid = mcp.call(f"{APP}.pid", {})["pid"]
                assert count("start") == 1, "宽限内的调用合并进同一通道，不再激活"

                # 3. 宽限后通道关闭，激活启动的 App 退出。
                eventually("App 进程退出", lambda: count("exit") == 1)

                # 4. 再次调用再激活（新进程）。
                second_pid = mcp.call(f"{APP}.pid", {})["pid"]
                assert count("start") == 2
                assert second_pid != first_pid
                eventually("第二次宽限后退出", lambda: count("exit") == 2)
            finally:
                mcp.close()
        finally:
            bus.close()
            kill_leftovers(log)
