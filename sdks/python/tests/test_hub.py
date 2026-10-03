"""Hub SDK 集成测试：嵌入式 Hub（随机端口）+ 同进程 App 端 SDK（app_mcp.AppMcp）经真实 WebSocket / 本地 IPC 连上。

测试关闭默认 IPC 端点（``enable_ipc=False``），不占用本机常驻 Host 的端点；IPC 用临时端点单独测试。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio
import json
import threading
import time

import pytest

hub_mod = pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp, ToolContext  # noqa: E402
from app_mcp.hub import CallResult, Hub, HubError, ToolError, ToolFormat  # noqa: E402

pytestmark = pytest.mark.hub


def start_notes_app(hub: Hub) -> AppMcp:
    app = AppMcp("notes", "笔记", host_url=f"ws://{hub.listen_addr}/app", overview="测试笔记 App")

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

        with Hub(listen="127.0.0.1:0", enable_ipc=False, approval_min_risk="destructive") as hub:
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

                # 运行状态：实例的连接 ID 与 App 端 SDK 看到的一致
                st = hub.status()
                assert st.service == "app-mcp" and st.listen == hub.listen_addr and st.reports == []
                notes = next(a for a in st.apps if a.app_id == "notes")
                assert notes.state == hub_mod.AppState.CONNECTED
                assert notes.instances[0].state == hub_mod.InstanceState.CONNECTED
                cid = notes.instances[0].info.connection_id
                assert cid is not None and cid == app.connection_id
                assert next(a for a in hub.apps() if a.app_id == "notes").instances[0].connection_id == cid

                # call_tool
                r = await hub.call_tool("notes.add", {"text": "买牛奶"}, timeout=5)
                assert isinstance(r, CallResult) and r.ok, r
                assert r.unwrap() == {"saved": "买牛奶"}
                assert r.overview is not None and "测试笔记" in r.overview.summary
                # Hub API 调用方的 Agent 任务（spec/hub-api.md 3.6）
                task = next(t for t in hub.status().tasks or [] if t.caller == "api")
                assert task.kind == hub_mod.CallerKind.API and task.id.startswith("task-")

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
                # principal / client_name 只在 MCP 出口发起的审批中出现
                assert req.principal is None and req.client_name is None

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
    with Hub(listen="127.0.0.1:0", enable_ipc=False, approval_min_risk="write") as hub:
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


def test_async_handler_without_loop_and_no_monkeypatch() -> None:
    """设置 async 回调时没有运行中的循环：在线程池中以 asyncio.run 执行。生成代码不被替换。"""
    assert hub_mod.ffi._uniffi_get_event_loop.__module__ == hub_mod.ffi.__name__
    threads: list[str] = []

    async def approve(req) -> bool:
        await asyncio.sleep(0.01)
        threads.append(threading.current_thread().name)
        return req.tool == "clear"

    with Hub(listen="127.0.0.1:0", enable_ipc=False, approval_min_risk="write") as hub:
        hub.set_approval_handler(approve)
        app = start_notes_app(hub)
        try:
            wait_tools(hub, 2)
            assert hub.call_tool_sync("notes.clear", timeout=5).unwrap() == {"cleared": True}
            no = hub.call_tool_sync("notes.add", {"text": "x"}, timeout=5)
            assert no.error is not None and no.error.kind == "USER_REJECTED"
            assert len(threads) == 2 and all(t.startswith("app-mcp-hub-cb") for t in threads)

            def boom(req) -> bool:
                raise RuntimeError("UI 崩溃")

            hub.set_approval_handler(boom)
            no = hub.call_tool_sync("notes.clear", timeout=5)
            assert no.error is not None and no.error.kind == "USER_REJECTED"
        finally:
            app.close()


def test_formats_and_shutdown() -> None:
    assert hub_mod.parse_format("gemini") == ToolFormat.GEMINI
    with pytest.raises(HubError):
        hub_mod.parse_format("nope")
    hub = Hub(enable_listen=False, enable_ipc=False)
    assert hub.listen_addr is None
    assert hub.ipc_endpoint is None
    assert {t.name for t in hub.tools()} == {
        "apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release", "apps.lock", "apps.unlock",
    }
    gemini = hub.export_tools("gemini")
    assert "functionDeclarations" in gemini
    st = hub.status()
    assert st.listen is None and st.apps == [] and not st.mcp_http and not st.auth.token_configured
    hub.close()
    hub.close()  # 幂等
    with pytest.raises(HubError.Shutdown):
        hub.status()
    with pytest.raises(HubError):
        hub.call_tool_sync("apps.list")


def test_state_dir_reports_dormant_store(tmp_path) -> None:
    dormant = tmp_path / "dormant"
    dormant.mkdir()
    (dormant / "broken.json").write_text("{")
    plain = Hub(enable_listen=False, enable_ipc=False)
    try:
        assert plain.status().dormant_store is None
    finally:
        plain.close()
    hub = Hub(enable_listen=False, enable_ipc=False, state_dir=str(tmp_path))
    try:
        store = hub.status().dormant_store
        assert store is not None
        assert store.dir == str(dormant)
        assert (store.loaded_instances, store.expired_instances, store.writes) == (0, 0, 0)
        assert [i.file for i in store.issues] == ["broken.json"]
        assert store.last_error is None
    finally:
        hub.close()


def test_unsupported_feature_is_distinct_category() -> None:
    """关闭的能力报 ``HubError.Unsupported``（与 ``Io`` 区分）。本机库为完整能力，调用成功；精简构建由 Android HubSelfTest 覆盖。"""
    def check(fn) -> None:  # type: ignore[no-untyped-def]
        try:
            fn()
        except HubError as e:
            assert isinstance(e, HubError.Unsupported), repr(e)

    check(lambda: Hub(listen="127.0.0.1:0", enable_ipc=False, mcp_http=True).close())
    hub = Hub(listen="127.0.0.1:0", enable_ipc=False)
    try:
        check(lambda: asyncio.run(hub.serve_http("127.0.0.1:0")))
    finally:
        hub.close()
    e = HubError.Unsupported("缺少 `mcp-server`")
    assert isinstance(e, HubError) and not isinstance(e, HubError.Io)
    assert "`mcp-server`" in str(e)


def test_progressive_exposure() -> None:
    from app_mcp.hub import ToolExposure, WakerConfig

    with Hub(
        listen="127.0.0.1:0", enable_ipc=False, tool_exposure=ToolExposure.PROGRESSIVE, waker=WakerConfig.DISABLED()
    ) as hub:
        app = start_notes_app(hub)
        try:
            wait_tools(hub, 2)
            builtins = [
                "apps.list", "apps.select", "apps.overview", "apps.tools", "apps.activate", "apps.release",
                "apps.lock", "apps.unlock",
            ]
            assert [t.name for t in hub.tools(session="c1")] == builtins
            r = hub.call_tool_sync("apps.tools", {"appId": "notes"}, session="c1")
            assert r.error is None
            assert {t["name"] for t in r.data["tools"]} == {"notes.add", "notes.clear"}
            assert "notes.add" in {t.name for t in hub.tools(session="c1")}
            assert [t.name for t in hub.tools()] == builtins
            names = [t["name"] for t in hub.export_tools("anthropic", session="c1")]
            assert len(names) == len(builtins) + 2
        finally:
            app.stop()


def test_stateless_config() -> None:
    """spec/hub-api.md 3.6 / 3.7：无会话 MCP 请求的配置可设置；启动时没有 Agent 任务。"""
    from app_mcp.hub import ToolExposure

    with Hub(
        enable_listen=False,
        enable_ipc=False,
        task_idle_ttl_ms=0,
        stateless_tool_exposure=ToolExposure.PROGRESSIVE,
        principal_select_ttl_ms=1500,
        stateless_list_ttl_ms=750,
    ) as hub:
        assert hub.status().tasks == []
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, task_idle_ttl_ms=-1)


def test_mcp_listen_config() -> None:
    """spec/hub-api.md 3.6：MCP 出口协议版本与 listen 上限可设置；status 报告 listen 流数。"""
    from app_mcp.hub import McpProtocolMode

    with Hub(
        enable_listen=False,
        enable_ipc=False,
        mcp_protocol_mode=McpProtocolMode.LEGACY_ONLY,
        max_listen_streams=0,
        max_listen_resources=8,
    ) as hub:
        assert hub.status().mcp_listen_streams == 0
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, max_listen_streams=-1)
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, max_listen_resources=2**32)


def test_max_task_handles_config() -> None:
    """spec/hub-api.md 3.6「任务句柄」：每主体任务句柄上限可设置（0 关闭）；越界报错。"""
    for n in (0, 5):
        with Hub(enable_listen=False, enable_ipc=False, max_task_handles=n) as hub:
            assert hub.status().mcp_sessions == 0
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, max_task_handles=-1)
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, max_task_handles=2**32)


SHOP_MANIFEST = json.dumps({
    "manifestVersion": 1, "appId": "shop", "name": "商城",
    "tools": [{"name": "cart.add", "description": "加购", "inputSchema": {"type": "object"}}],
})


def test_object_locks() -> None:
    """spec/hub-api.md 3.6「对象锁」：缺省列出 apps.lock / apps.unlock；他人持有 → LOCKED；status().locks 列出未到期的锁。"""
    from app_mcp.hub import LockStatus

    with Hub(enable_listen=False, enable_ipc=False, manifests_json=[SHOP_MANIFEST]) as hub:
        names = {t.name for t in hub.tools()}
        assert {"apps.lock", "apps.unlock"} <= names
        ok = hub.call_tool_sync("apps.lock", {"appId": "shop", "ttlMs": 30000}, session="s1")
        assert ok.error is None and ok.data["renewed"] is False
        denied = hub.call_tool_sync("apps.lock", {"appId": "shop"}, session="s2")
        assert denied.error is not None and denied.error.kind == "LOCKED"
        assert json.loads(denied.error.details_json or "null")["holder"] == "api"
        locks = hub.status().locks
        assert locks is not None and len(locks) == 1
        lock = locks[0]
        assert isinstance(lock, LockStatus)
        assert (lock.app_id, lock.key, lock.caller, lock.holder) == ("shop", None, "api:s1", "api")
        assert 0 < lock.expires_in_ms <= 30000
        released = hub.call_tool_sync("apps.unlock", {"appId": "shop"}, session="s1")
        assert released.error is None and released.data["released"] is True
        assert hub.status().locks == []


def test_max_locks_config() -> None:
    """max_locks = 0 关闭对象锁（不列出，调用为 TOOL_NOT_FOUND）；越界报错。"""
    with Hub(enable_listen=False, enable_ipc=False, manifests_json=[SHOP_MANIFEST], max_locks=0) as hub:
        assert not {"apps.lock", "apps.unlock"} & {t.name for t in hub.tools()}
        r = hub.call_tool_sync("apps.lock", {"appId": "shop"}, session="s1")
        assert r.error is not None and r.error.kind == "TOOL_NOT_FOUND"
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, max_locks=-1)
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, max_locks=2**32)


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
        with Hub(listen="127.0.0.1:0", enable_ipc=False, lease_ttl_ms=0, wake_timeout_ms=10_000, list_changed_debounce_ms=20) as hub:
            events = hub.events()
            app = AppMcp(
                "sleepy",
                "会睡觉的 App",
                host_url=f"ws://{hub.listen_addr}/app",
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


def test_native_app_over_ipc(tmp_path) -> None:
    import os
    import sys

    if sys.platform == "win32":
        endpoint = rf"pipe:\\.\pipe\app-mcp-py-test-{os.getpid()}"
    else:
        endpoint = f"unix:{tmp_path / 'run' / 'hub.sock'}"
    with Hub(enable_listen=False, ipc_endpoint=endpoint) as hub:
        assert hub.ipc_endpoint == endpoint
        app = AppMcp("notes", "笔记", host_url=hub.ipc_endpoint)

        @app.tool("add", description="添加笔记", risk="write")
        def add(text: str) -> dict:
            return {"saved": text}

        app.start()
        try:
            wait_tools(hub, 1)
            r = hub.call_tool_sync("notes.add", {"text": "经 IPC"})
            assert r.error is None and r.data == {"saved": "经 IPC"}
            inst = next(a for a in hub.apps() if a.app_id == "notes").instances[0]
            assert inst.pid == os.getpid()
        finally:
            app.stop()


def test_surface_page_and_idempotency_key() -> None:
    """spec/hub-api.md 3.14 / 3.15：HubTool.surface / page、call_tool(idempotency_key=) 原样转交、routed_to、navigate_timeout_ms。"""
    from app_mcp.hub import ToolSurface

    with Hub(listen="127.0.0.1:0", enable_ipc=False, navigate_timeout_ms=800) as hub:
        app = AppMcp("cafe", "咖啡", host_url=f"ws://{hub.listen_addr}/app")

        @app.tool("cart.checkout", description="结算", surface="view", page="cart")
        def checkout(ctx: ToolContext) -> dict:
            return {"key": ctx.idempotency_key}

        @app.tool("order.submit", description="下单")
        def submit(ctx: ToolContext) -> dict:
            return {"key": ctx.idempotency_key}

        app.start()
        try:
            deadline = time.monotonic() + 10
            while True:
                tools = hub.tools(apps=["cafe"], include_builtin=False)
                if len(tools) == 2:
                    break
                assert time.monotonic() < deadline, "等待工具注册超时"
                time.sleep(0.02)
            by_tool = {t.tool: t for t in tools}
            assert (by_tool["cart.checkout"].surface, by_tool["cart.checkout"].page) == (ToolSurface.VIEW, "cart")
            assert (by_tool["order.submit"].surface, by_tool["order.submit"].page) == (ToolSurface.APP, None)
            builtins = {t.name: t for t in hub.tools() if t.name.startswith("apps.")}
            assert {"apps.activate", "apps.release", "apps.page", "apps.navigate"} <= builtins.keys()
            assert all(t.surface is None and t.page is None for t in builtins.values())

            out = hub.call_tool_sync("cafe.order.submit", idempotency_key="order-7")
            assert out.error is None, out
            assert out.data == {"key": "order-7"}
            assert out.routed_to is None
            assert out.woke is False, "已连接的 App 不唤醒"
            assert out.duration_ms >= 0
            assert hub.call_tool_sync("cafe.order.submit").data == {"key": None}
            bad = hub.call_tool_sync("cafe.order.submit", idempotency_key="")
            assert bad.error is not None and bad.error.kind == "INVALID_INPUT"
        finally:
            app.stop()


def test_limits_annotations_and_structured_result() -> None:
    """第 14 / 19 项：限流 / 大小上限配置与统计、工具注解 / outputSchema、结构化调用结果。"""
    from app_mcp import ToolResult
    from app_mcp.hub import LimitsConfig, OutputValidation, ResultStatus

    # 非法：限流时 burst 须 ≥ 1；未知键 / 取值
    with pytest.raises(HubError.InvalidConfig):
        Hub(listen="127.0.0.1:0", enable_ipc=False, limits={"toolRateBurst": 0})
    with pytest.raises(ValueError):
        Hub(listen="127.0.0.1:0", enable_ipc=False, limits={"toolRate": 1})
    with pytest.raises(ValueError):
        Hub(listen="127.0.0.1:0", enable_ipc=False, output_validation="strict")

    schema = {"type": "object", "properties": {"orderId": {"type": "string"}}}
    with Hub(
        listen="127.0.0.1:0",
        enable_ipc=False,
        limits={"toolRatePerMinute": 1, "tool_rate_burst": 1, "maxArgumentsBytes": 64},
        output_validation="reject",
    ) as hub:
        app = AppMcp("orders", "订单", host_url=f"ws://{hub.listen_addr}/app")

        @app.tool("submit", description="下单", annotations={"idempotent_hint": False}, output_schema=schema)
        def submit() -> ToolResult:
            return ToolResult(
                {"orderId": "o1"},
                status="pending",
                state_resource="order.state",
                summary="已提交，等待付款",
                annotations={"priority": 0.5},
            )

        @app.tool("echo", description="回显")
        def echo(text: str = "") -> dict:
            return {"text": text}

        app.start()
        try:
            deadline = time.monotonic() + 10
            while True:
                tools = hub.tools(apps=["orders"], include_builtin=False)
                if len(tools) == 2 and all(t.availability.name == "AVAILABLE" for t in tools):
                    break
                assert time.monotonic() < deadline, "等待工具注册超时"
                time.sleep(0.02)
            by_name = {t.tool: t for t in tools}
            submit_tool = by_name["submit"]
            assert submit_tool.annotations.idempotent_hint is False
            assert submit_tool.annotations.read_only_hint is False  # 缺少的字段按 risk（write）推导
            assert json.loads(submit_tool.output_schema_json) == schema
            assert by_name["echo"].output_schema_json is None

            out = hub.call_tool_sync("orders.submit")
            assert out.ok, out
            assert out.data == {"orderId": "o1"}
            assert out.status == ResultStatus.PENDING
            assert out.state_resource == "app-mcp://orders/order.state"
            assert out.summary == "已提交，等待付款"
            assert out.annotations is not None and out.annotations.priority == 0.5
            assert hub.call_tool_sync("orders.submit").error.kind == "RATE_LIMITED"
            assert hub.call_tool_sync("orders.echo", {"text": "x" * 100}).error.kind == "PAYLOAD_TOO_LARGE"
            plain = hub.call_tool_sync("orders.echo", {"text": "hi"})
            assert plain.ok and plain.status == ResultStatus.DONE and plain.summary is None

            st = hub.status()
            assert st.limits == LimitsConfig(
                tool_rate_per_minute=1,
                tool_rate_burst=1,
                app_rate_per_minute=600,
                app_rate_burst=60,
                max_arguments_bytes=64,
                max_result_bytes=4 * 1024 * 1024,
                max_resource_bytes=4 * 1024 * 1024,
                agent_rate_per_minute=0,
                agent_rate_burst=0,
            )
            assert st.output_validation == OutputValidation.REJECT
            orders = next(a for a in st.apps if a.app_id == "orders")
            assert (orders.rate_limited, orders.too_large) == (1, 1)
            decl = next(t for t in orders.tools if t.name == "submit")
            assert decl.annotations is not None and decl.annotations.idempotent_hint is False
            assert decl.effective.read_only_hint is False
            assert decl.output_schema is True
        finally:
            app.close()


def test_policy_hide_deny_and_set_policy() -> None:
    """策略挂点（spec/hub-api.md 3.13）：hide / deny、set_policy 替换、不合法规则保留旧规则、命中计数。"""
    from app_mcp.hub import PolicyAction, PolicyConfig, PolicyHook, PolicyRule

    # 非法：通配符不在末尾（Hub 校验）；未知键 / 取值（封装层）
    with pytest.raises(HubError.InvalidConfig):
        Hub(listen=None, enable_listen=False, enable_ipc=False, policy={"rules": [{"id": "x", "action": "hide", "app": "a*b"}]})
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, policy={"rules": [{"id": "x", "action": "block", "app": "a"}]})
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, policy={"rules": [], "bogus": 1})

    policy = {
        "rules": [
            {"id": "hide-clear", "action": "hide", "app": "notes", "tool": "clear"},
            {"id": "deny-add", "action": "deny", "app": "notes", "tool": "add", "hooks": ["call", "wake"]},
        ]
    }
    with Hub(listen="127.0.0.1:0", enable_ipc=False, policy=policy) as hub:
        app = start_notes_app(hub)
        try:
            wait_tools(hub, 1)
            assert [t.name for t in hub.tools(apps=["notes"], include_builtin=False)] == ["notes.add"]
            hidden = hub.call_tool_sync("notes.clear")
            assert hidden.error is not None and hidden.error.kind == "TOOL_NOT_FOUND"
            denied = hub.call_tool_sync("notes.add", {"text": "x"})
            assert denied.error is not None and denied.error.kind == "POLICY_DENIED"
            details = json.loads(denied.error.details_json or "{}")
            assert details["ruleId"] == "deny-add" and details["hook"] == "call"

            st = hub.policy()
            assert [(r.rule.id, r.hits) for r in st.rules] == [("hide-clear", 1), ("deny-add", 1)]
            assert st.rules[1].rule.hooks == [PolicyHook.CALL, PolicyHook.WAKE]
            assert st.last_error is None
            assert hub.status().policy is not None

            # 不合法（hide 不能写 hooks）→ INVALID_INPUT，旧规则继续生效
            bad = PolicyRule(id="h", action=PolicyAction.HIDE, app="notes", tool=None, annotations=None, hooks=[PolicyHook.CALL])
            with pytest.raises(HubError.Tool) as e:
                hub.set_policy(PolicyConfig(rules=[bad]))
            assert e.value.kind == "INVALID_INPUT"
            assert hub.call_tool_sync("notes.add", {"text": "x"}).error.kind == "POLICY_DENIED"
            assert hub.policy().last_error is not None

            # 按注解匹配（字典形式）：清空后只拒绝 destructive 工具
            hub.set_policy({"rules": [{"id": "no-destructive", "action": "deny", "app": "*", "annotations": {"destructiveHint": True}}]})
            assert hub.call_tool_sync("notes.add", {"text": "x"}).data == {"saved": "x"}
            out = hub.call_tool_sync("notes.clear")
            assert out.error is not None and out.error.kind == "POLICY_DENIED"
            # 按 Agent 的规则（字典键 agent）：Hub API 调用方没有 Agent 身份，不匹配
            hub.set_policy({"rules": [{"id": "no-add-cursor", "action": "deny", "app": "notes", "agent": "cursor"}]})
            assert hub.policy().rules[0].rule.agent == "cursor"
            assert hub.call_tool_sync("notes.add", {"text": "x"}).data == {"saved": "x"}
            hub.set_policy({})
            assert hub.call_tool_sync("notes.clear").data == {"cleared": True}
            assert hub.policy().rules == []
        finally:
            app.stop()


def test_progress_callback_and_resource_annotations() -> None:
    """第 16 项 O2：``call_tool(on_progress=...)`` 在结果返回前收到合并后的进度；资源内容标注经 Hub 列出。"""
    from app_mcp.hub import ProgressUpdate

    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False) as hub:
            app = AppMcp("work", "长任务", host_url=f"ws://{hub.listen_addr}/app")

            @app.tool("run", description="长任务", risk="read")
            def run(ctx: ToolContext) -> dict:
                ctx.progress(1, 2, "第一步")
                time.sleep(0.35)  # 超过 Hub 默认合并间隔 250 ms
                ctx.progress(2, 2)
                time.sleep(0.35)
                return {"done": True}

            @app.resource("cart", description="购物车", annotations={"audience": ["user"], "priority": 0.5})
            def cart() -> dict:
                return {}

            app.start()
            try:
                deadline = time.monotonic() + 10
                while not hub.tools(apps=["work"], include_builtin=False) or not hub.resources():
                    assert time.monotonic() < deadline, "等待注册超时"
                    await asyncio.sleep(0.02)
                (res,) = [r for r in hub.resources() if r.app_id == "work"]
                assert res.annotations is not None
                assert (res.annotations.priority, [a.name for a in res.annotations.audience]) == (0.5, ["USER"])

                loop_thread = threading.get_ident()
                got: list[tuple[ProgressUpdate, int]] = []
                out = await hub.call_tool("work.run", on_progress=lambda u: got.append((u, threading.get_ident())))
                assert out.error is None and out.data == {"done": True}
                assert [(u.progress, u.total, u.message) for u, _ in got] == [(1.0, 2.0, "第一步"), (2.0, 2.0, None)]
                assert all(t == loop_thread for _, t in got), "在调用方的事件循环线程上回调"

                def boom(_u: ProgressUpdate) -> None:
                    raise RuntimeError("UI 崩溃")

                out = await hub.call_tool("work.run", on_progress=boom)
                assert out.error is None, "回调异常不影响结果"
            finally:
                app.close()

    asyncio.run(main())
