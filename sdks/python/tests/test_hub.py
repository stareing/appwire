"""Hub SDK 集成测试：嵌入式 Hub（随机端口）+ 同进程 App 端 SDK（app_mcp.AppMcp）经真实 WebSocket 连上。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio
import threading
import time

import pytest

hub_mod = pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp  # noqa: E402
from app_mcp.hub import CallResult, Hub, HubError, ToolError, ToolFormat  # noqa: E402

pytestmark = pytest.mark.hub


def start_notes_app(hub: Hub) -> AppMcp:
    app = AppMcp("notes", "笔记", host_url=f"ws://{hub.ws_addr}", overview="测试笔记 App")

    @app.tool("add", description="添加笔记", risk="write")
    def add(text: str) -> dict:
        return {"saved": text}

    @app.tool("clear", description="清空笔记", risk="destructive")
    def clear() -> dict:
        return {"cleared": True}

    app.start()
    return app


def wait_tools(hub: Hub, n: int, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while len(hub.tools(apps=["notes"], include_builtin=False)) < n:
        assert time.monotonic() < deadline, "等待工具注册超时"
        time.sleep(0.02)


def test_end_to_end_async() -> None:
    async def main() -> None:
        approvals: list = []

        async def approve(req) -> bool:  # async handler，在本循环上执行
            approvals.append((req, asyncio.get_running_loop()))
            return False

        with Hub(ws_addr="127.0.0.1:0", approval_min_risk="destructive") as hub:
            hub.set_approval_handler(approve)
            events = hub.events()
            app = start_notes_app(hub)
            try:
                # 事件：App 连上
                e = await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "notes", 10)
                assert e.instance_id

                # 列工具
                await asyncio.to_thread(wait_tools, hub, 2)
                tools = {t.name: t for t in hub.tools(apps=["notes"], include_builtin=False)}
                assert set(tools) == {"notes.add", "notes.clear"}
                assert tools["notes.add"].availability == hub_mod.Availability.AVAILABLE
                assert any(a.app_id == "notes" and a.connected for a in hub.apps())

                # call_tool
                r = await hub.call_tool("notes.add", {"text": "买牛奶"}, timeout=5)
                assert isinstance(r, CallResult) and r.ok, r
                assert r.unwrap() == {"saved": "买牛奶"}
                assert r.overview is not None and "测试笔记" in r.overview.summary

                # export_tools + dispatch（Anthropic 格式）
                exported = hub.export_tools("anthropic", apps=["notes"])
                assert "notes__add" in {t["name"] for t in exported}
                assert all("input_schema" in t for t in exported)
                reply = await hub.dispatch(
                    ToolFormat.ANTHROPIC,
                    {"type": "tool_use", "id": "toolu_1", "name": "notes__add", "input": {"text": "来自 LLM"}},
                )
                assert reply["type"] == "tool_result" and reply["tool_use_id"] == "toolu_1"
                assert not reply.get("is_error")
                assert "来自 LLM" in json_text(reply["content"])

                # 审批拒绝 → USER_REJECTED
                rejected = await hub.call_tool("notes.clear", timeout=5)
                assert rejected.error is not None and rejected.error.kind == "USER_REJECTED"
                with pytest.raises(ToolError) as ei:
                    rejected.unwrap()
                assert ei.value.kind == "USER_REJECTED"
                assert len(approvals) == 1
                req, loop = approvals[0]
                assert req.app_id == "notes" and req.risk == hub_mod.Risk.DESTRUCTIVE
                assert loop is asyncio.get_running_loop()

                # dispatch 被拒绝时回填 is_error
                reply = await hub.dispatch(
                    "anthropic", {"type": "tool_use", "id": "toolu_2", "name": "notes__clear", "input": {}}
                )
                assert reply["is_error"] is True and "USER_REJECTED" in json_text(reply["content"])

                # 名称无法解析 → HubError
                with pytest.raises(HubError):
                    await hub.call_tool("nosuchapp.x")

                # 事件：App 断开
                app.close()
                await events.wait_for(lambda e: e.is_app_disconnected() and e.app_id == "notes", 10)
            finally:
                events.close()
                app.close()

    asyncio.run(main())


def test_sync_api_and_sync_approval() -> None:
    """非 asyncio 代码：*_sync 方法 + 同步审批回调（在线程池中执行，可阻塞）。"""
    seen: list[str] = []
    connected = threading.Event()
    with Hub(ws_addr="127.0.0.1:0", approval_min_risk="write") as hub:
        hub.on_event(lambda e: connected.set() if e.is_app_connected() else None)
        hub.set_approval_handler(lambda req: seen.append(req.tool) or req.tool.endswith("add"))
        app = start_notes_app(hub)
        try:
            assert connected.wait(10)
            wait_tools(hub, 2)
            ok = hub.call_tool_sync("notes.add", {"text": "x"}, timeout=5)
            assert ok.unwrap() == {"saved": "x"}
            no = hub.call_tool_sync("notes.clear", timeout=5)
            assert no.error is not None and no.error.kind == "USER_REJECTED"
            assert len(seen) == 2
            out = hub.dispatch_sync(
                "openai",
                {"id": "c1", "type": "function", "function": {"name": "notes__add", "arguments": '{"text":"y"}'}},
            )
            assert out["role"] == "tool" and out["tool_call_id"] == "c1" and '"y"' in out["content"]
        finally:
            app.close()


def test_formats_and_shutdown() -> None:
    assert hub_mod.parse_format("gemini") == ToolFormat.GEMINI
    with pytest.raises(HubError):
        hub_mod.parse_format("nope")
    hub = Hub(enable_ws=False)
    assert hub.ws_addr is None
    assert {t.name for t in hub.tools()} == {"apps.list", "apps.select", "apps.overview"}
    gemini = hub.export_tools("gemini")
    assert "functionDeclarations" in gemini
    hub.close()
    hub.close()  # 幂等
    with pytest.raises(HubError):
        hub.call_tool_sync("apps.list")


def json_text(content: object) -> str:
    """Anthropic tool_result.content 可能是字符串或 [{type: text, text}]。"""
    if isinstance(content, str):
        return content
    assert isinstance(content, list)
    return "".join(c.get("text", "") for c in content)


def test_dormant_app_woken_by_custom_waker() -> None:
    """App 休眠 → Hub 列出 dormant → 调用触发自定义 Waker（让同进程 App handle_wake）→ 调用成功。"""
    from app_mcp import LifecyclePolicy, WakeDescriptor

    async def main() -> None:
        with Hub(ws_addr="127.0.0.1:0", lease_ttl_ms=0, wake_timeout_ms=10_000, list_changed_debounce_ms=20) as hub:
            events = hub.events()
            app = AppMcp(
                "sleepy",
                "会睡觉的 App",
                host_url=f"ws://{hub.ws_addr}",
                instance_id="s1",
                lifecycle=LifecyclePolicy(
                    mode="idle",
                    idle_timeout=0.3,
                    wake=WakeDescriptor("android-intent", "dev.example/.WakeReceiver", background=True),
                ),
            )

            @app.tool("ping", description="回显")
            def ping(x: int = 0) -> dict:
                return {"echo": x}

            wakes: list = []

            async def waker(req) -> None:  # 厂商在这里发送广播；测试中直接让同进程 App 处理激活参数
                wakes.append(req)
                if not app.handle_wake(req.activation_arg):
                    raise hub_mod.WakeFailed("LAUNCH_FAILED", "不认识的激活参数")

            hub.set_waker(waker)
            app.start()
            try:
                e = await events.wait_for(lambda e: e.is_app_dormant() and e.app_id == "sleepy", 10)
                assert e.instance_id == "s1"
                info = next(a for a in hub.apps() if a.app_id == "sleepy")
                assert not info.connected
                assert [i.instance_id for i in info.dormant_instances] == ["s1"]
                tools = hub.tools(apps=["sleepy"], include_builtin=False)
                assert [t.availability for t in tools] == [hub_mod.Availability.DORMANT]
                assert hub.tools(apps=["sleepy"], include_builtin=False, only_available=True) == []

                r = await hub.call_tool("sleepy.ping", {"x": 1}, timeout=10)
                assert r.unwrap() == {"echo": 1}
                assert r.instance_id == "s1"
                assert len(wakes) == 1
                w = wakes[0]
                assert (w.app_id, w.instance_id) == ("sleepy", "s1")
                assert w.descriptor.kind == hub_mod.WakeKind.ANDROID_INTENT
                assert w.activation_arg == f"app-mcp-wake:{w.token}"

                # 失败类别透传（同步回调抛 WakeFailed）
                await events.wait_for(lambda e: e.is_app_dormant() and e.app_id == "sleepy", 10)

                def fail(req) -> None:
                    raise hub_mod.WakeFailed("APP_NOT_INSTALLED", "没装")

                hub.set_waker(fail)
                bad = await hub.call_tool("sleepy.ping", timeout=10)
                assert bad.error is not None and bad.error.kind == "APP_NOT_INSTALLED"
                hub.set_waker(None)
            finally:
                events.close()
                app.close()

    asyncio.run(main())
