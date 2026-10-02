"""单元测试：不需要 Host。"""

from __future__ import annotations

import asyncio
import json
import threading
import time
from typing import Literal, Optional

import pytest

import app_mcp
from app_mcp import AppMcp, ToolCallError, ToolContext, ToolResult, schema_from_function
from app_mcp import app_mcp_uniffi as ffi
from app_mcp._client import _ResourceAdapter, _ToolAdapter, _Registration
from app_mcp._schema import ArgumentBinder


# ---------------------------------------------------------------------------
# schema
# ---------------------------------------------------------------------------


def test_schema_from_signature():
    def f(sku: str, qty: int = 1, note: Optional[str] = None, mode: Literal["a", "b"] = "a", ctx: ToolContext = None):
        pass

    schema = schema_from_function(f, ToolContext)
    assert schema == {
        "type": "object",
        "properties": {
            "sku": {"type": "string"},
            "qty": {"type": "integer", "default": 1},
            "note": {"type": "string", "default": None},
            "mode": {"enum": ["a", "b"], "default": "a"},
        },
        "required": ["sku"],
        "additionalProperties": False,
    }


def test_schema_no_params():
    assert schema_from_function(lambda: None) is None


def test_schema_lists():
    def f(tags: list[str], extra: dict):
        pass

    s = schema_from_function(f)
    assert s["properties"]["tags"] == {"type": "array", "items": {"type": "string"}}
    assert s["properties"]["extra"] == {"type": "object"}


try:
    import pydantic

    class Item(pydantic.BaseModel):  # 模块级定义，便于解析字符串注解
        sku: str
        qty: int = 1

except ImportError:  # pragma: no cover
    pydantic = None


@pytest.mark.skipif(pydantic is None, reason="未安装 pydantic")
def test_pydantic_model_param():
    def f(item: Item):
        return item

    binder = ArgumentBinder(f, ToolContext)
    assert binder.schema()["properties"]["sku"]["type"] == "string"
    kwargs = binder.bind({"sku": "A"}, None)
    assert kwargs["item"].qty == 1
    with pytest.raises(ValueError):
        binder.bind({"qty": "x"}, None)


def test_bind_ctx_by_name():
    def f(a, ctx):
        pass

    b = ArgumentBinder(f, ToolContext)
    assert b.context_param == "ctx"
    assert b.bind({"a": 1, "zzz": 2}, "C") == {"a": 1, "ctx": "C"}


def test_tool_call_error_validates_kind():
    e = ToolCallError("INVALID_INPUT", "坏参数")
    assert e.kind == "INVALID_INPUT"
    with pytest.raises(ValueError):
        ToolCallError("NOT_A_KIND", "x")
    assert "HANDLER_ERROR" in app_mcp.ERROR_KINDS
    assert {"RATE_LIMITED", "PAYLOAD_TOO_LARGE", "POLICY_DENIED", "USER_ACTION_REQUIRED"} <= app_mcp.ERROR_KINDS


def test_user_action_required_details():
    e = ToolCallError.user_action_required("请先登录", app_mcp.UserActionReason.LOGIN, "shop://login")
    assert (e.kind, e.message, e.details) == ("USER_ACTION_REQUIRED", "请先登录", {"reason": "login", "uri": "shop://login"})
    assert ToolCallError.user_action_required("x", reason="custom").details == {"reason": "custom"}
    assert ToolCallError.user_action_required("x", uri="shop://a").details == {"uri": "shop://a"}
    assert ToolCallError.user_action_required("切到前台").details is None


# ---------------------------------------------------------------------------
# 执行路径（用假 Call 驱动适配器）
# ---------------------------------------------------------------------------


class FakeCall:
    def __init__(self, args: dict, name: str = "t"):
        self._args = json.dumps(args)
        self._name = name
        self.done = threading.Event()
        self.result = None
        self.listener = None

    def call_id(self):
        return "c1"

    def tool_name(self):
        return self._name

    def arguments_json(self):
        return self._args

    def set_cancel_listener(self, listener):
        self.listener = listener

    def complete(self, data_json, hints):
        if self.done.is_set():
            raise ffi.AppMcpError.AlreadyCompleted()
        self.result = ("ok", json.loads(data_json) if data_json is not None else None, hints)
        self.done.set()

    def complete_with(self, result):
        if self.done.is_set():
            raise ffi.AppMcpError.AlreadyCompleted()
        self.full = result
        self.result = ("ok", json.loads(result.data_json) if result.data_json is not None else None, result.state_hints)
        self.done.set()

    def fail(self, kind, message):
        if self.done.is_set():
            raise ffi.AppMcpError.AlreadyCompleted()
        self.result = ("err", kind, message)
        self.done.set()

    def wait(self):
        assert self.done.wait(5), "调用未完成"
        return self.result


class FakeRead(FakeCall):
    def complete(self, data_json):  # type: ignore[override]
        self.result = ("ok", json.loads(data_json))
        self.done.set()


@pytest.fixture
def client():
    c = AppMcp(app_id="unit-test", app_name="Unit", host_url="ws://127.0.0.1:9")
    yield c
    c.close()


def adapter(client, fn):
    return _ToolAdapter(_Registration(client, fn, ArgumentBinder(fn, ToolContext)))


def test_sync_handler_returns_json(client):
    main = threading.get_ident()

    def add(a: int, b: int, ctx: ToolContext):
        ctx.add_state_hint("cart")
        return {"sum": a + b, "same_thread": threading.get_ident() == main}

    call = FakeCall({"a": 2, "b": 3})
    adapter(client, add).invoke(call)
    assert call.wait() == ("ok", {"sum": 5, "same_thread": False}, ["cart"])


def test_tool_result_hints(client):
    call = FakeCall({})
    adapter(client, lambda: ToolResult([1], state_hints=["x"])).invoke(call)
    assert call.wait() == ("ok", [1], ["x"])


def test_structured_tool_result(client):
    def submit(ctx: ToolContext):
        ctx.add_state_hint("cart")
        return ToolResult(
            {"orderId": "o1"},
            status="pending",
            state_resource="order.state",
            summary="已提交",
            annotations={"audience": ["user"], "priority": 0.5},
        )

    call = FakeCall({})
    adapter(client, submit).invoke(call)
    assert call.wait() == ("ok", {"orderId": "o1"}, ["cart"])
    full = call.full
    assert full.status == ffi.ResultStatus.PENDING
    assert (full.state_resource, full.summary) == ("order.state", "已提交")
    assert full.annotations == ffi.ContentAnnotations(audience=[ffi.Audience.USER], priority=0.5, last_modified=None)

    # 默认 done、无附加信息
    r = ToolResult()
    assert (r.status, r.state_resource, r.summary, r.annotations) == (ffi.ResultStatus.DONE, None, None, None)


def test_tool_result_rejects_invalid_values():
    with pytest.raises(ValueError):
        ToolResult(status="later")
    with pytest.raises(ValueError):
        ToolResult(annotations={"audience": ["robot"]})
    with pytest.raises(ValueError):
        ToolResult(annotations={"lastModified": "2026-01-01"})


def test_tool_annotations_and_output_schema(client):
    handle = client.add_tool(
        lambda: None,
        "order.submit",
        "下单",
        annotations={"idempotent_hint": False, "open_world_hint": True},
        output_schema={"type": "object"},
    )
    spec = handle._spec
    assert spec.annotations == ffi.ToolAnnotations(idempotent_hint=False, open_world_hint=True)
    assert json.loads(spec.output_schema_json) == {"type": "object"}
    handle.update(description="下单（新）")
    assert handle._spec.annotations == spec.annotations  # 未给出的字段保持不变
    handle.update(annotations=ffi.ToolAnnotations(read_only_hint=True), output_schema='{"type":"array"}')
    assert handle._spec.annotations.read_only_hint is True
    assert json.loads(handle._spec.output_schema_json) == {"type": "array"}
    with pytest.raises(ValueError):
        client.add_tool(lambda: None, "bad", "坏", annotations={"readOnlyHint": True})
    with pytest.raises(ValueError):
        client.add_tool(lambda: None, "bad2", "坏", output_schema="{")


def test_update_omitted_keeps_none_clears(client):
    handle = client.add_tool(
        lambda: None,
        "doc.save",
        "保存",
        input_schema={"type": "object", "properties": {"x": {"type": "string"}}},
        risk="destructive",
        activation="foreground",
        title="保存文档",
        annotations={"read_only_hint": False},
        output_schema={"type": "object"},
    )
    before = handle._spec
    # 未给出的字段保持不变
    handle.update(description="保存（新）")
    s = handle._spec
    assert s.description == "保存（新）"
    assert (s.input_schema_json, s.risk, s.activation, s.title, s.annotations, s.output_schema_json) == (
        before.input_schema_json,
        before.risk,
        before.activation,
        before.title,
        before.annotations,
        before.output_schema_json,
    )
    # 给出值则替换
    handle.update(title="另存", activation="background", risk="read", annotations={"idempotent_hint": True})
    s = handle._spec
    assert (s.title, s.activation, s.risk) == ("另存", ffi.Activation.BACKGROUND, ffi.Risk.READ)
    assert s.annotations == ffi.ToolAnnotations(idempotent_hint=True)
    # 显式 None 清除声明
    handle.update(title=None, activation=None, risk=None, annotations=None, output_schema=None, input_schema=None)
    s = handle._spec
    assert (s.title, s.activation, s.risk, s.annotations, s.output_schema_json, s.input_schema_json) == (
        None,
        None,
        None,
        None,
        None,
        None,
    )
    assert s.description == "保存（新）"


def test_update_keeps_enabled_state(client):
    handle = client.add_tool(lambda: None, "doc.close", "关闭")
    handle.set_enabled(False)
    handle.update(title="关闭文档")
    assert handle._spec.enabled is False


def test_none_result_is_null(client):
    call = FakeCall({})
    adapter(client, lambda: None).invoke(call)
    assert call.wait() == ("ok", None, [])


def test_errors_are_mapped(client):
    def bad():
        raise ToolCallError("USER_REJECTED", "不行")

    call = FakeCall({})
    adapter(client, bad).invoke(call)
    assert call.wait() == ("err", "USER_REJECTED", "不行")

    call = FakeCall({})
    adapter(client, lambda: 1 / 0).invoke(call)
    kind, _ = call.wait()[1:]
    assert kind == "HANDLER_ERROR"

    call = FakeCall({})
    adapter(client, lambda: object()).invoke(call)
    assert call.wait()[1] == "HANDLER_ERROR"


def test_async_handler(client):
    async def slow(x: int):
        await asyncio.sleep(0.01)
        return x * 2

    call = FakeCall({"x": 21})
    adapter(client, slow).invoke(call)
    assert call.wait() == ("ok", 42, [])


def test_async_cancel(client):
    started = threading.Event()

    async def forever():
        started.set()
        await asyncio.sleep(60)

    call = FakeCall({})
    adapter(client, forever).invoke(call)
    assert started.wait(5)
    call.listener.on_cancel(ffi.CancelReason.REQUESTED)
    assert call.wait() == ("err", "CANCELLED", "调用已取消")


def test_custom_dispatcher_and_timeout():
    queue: list = []
    c = AppMcp(app_id="unit-test", app_name="Unit", dispatcher=queue.append, dispatch_timeout=0.1)
    try:
        call = FakeCall({})
        _ToolAdapter(_Registration(c, lambda: "ui", ArgumentBinder(lambda: "ui"))).invoke(call)
        assert len(queue) == 1 and not call.done.is_set()
        queue.pop()()  # 模拟 UI 线程执行
        assert call.wait() == ("ok", "ui", [])

        stuck = FakeCall({})
        _ToolAdapter(_Registration(c, lambda: "late", ArgumentBinder(lambda: "late"))).invoke(stuck)
        assert stuck.wait()[1] == "APP_NOT_RESPONDING"
        queue.pop()()  # UI 线程恢复后不会再次完成
        time.sleep(0.05)
    finally:
        c.close()


def test_resource_adapter(client):
    read = FakeRead({})
    _ResourceAdapter(_Registration(client, lambda: {"items": []}, None)).read(read)
    assert read.wait() == ("ok", {"items": []})


class FakeDetailedRead(FakeRead):
    def fail_with_details(self, kind, message, details_json):
        self.result = ("err", kind, message, None if details_json is None else json.loads(details_json))
        self.done.set()


def test_resource_reader_errors_carry_details(client):
    def need_login():
        raise ToolCallError.user_action_required("登录已过期", "login", "shop://login")

    read = FakeDetailedRead({})
    _ResourceAdapter(_Registration(client, need_login, None)).read(read)
    assert read.wait() == ("err", "USER_ACTION_REQUIRED", "登录已过期", {"reason": "login", "uri": "shop://login"})

    def plain():
        raise ToolCallError("USER_REJECTED", "没有")

    read = FakeDetailedRead({})
    _ResourceAdapter(_Registration(client, plain, None)).read(read)
    assert read.wait() == ("err", "USER_REJECTED", "没有"), "无详情时走 fail"


def test_resource_annotations(client):
    client.add_resource(lambda: 1, "a", "a", annotations={"audience": ["user"], "priority": 0.5})
    client.add_resource(lambda: 1, "b", "b", annotations=app_mcp.ContentAnnotations(priority=1.0))
    with pytest.raises(ValueError):
        client.add_resource(lambda: 1, "c", "c", annotations={"bogus": 1})
    with pytest.raises(ValueError):
        client.add_resource(lambda: 1, "d", "d", annotations={"audience": ["bot"]})


def test_call_dedup_config():
    assert app_mcp.CallDedup(ttl=1.5, max_entries=3)._ffi() == ffi.CallDedupPolicy(ttl_ms=1500, max_entries=3)
    assert app_mcp.CallDedup()._ffi() == ffi.CallDedupPolicy(ttl_ms=300000, max_entries=64)
    assert app_mcp.CallDedup.OFF == app_mcp.CallDedup(0, 0)
    with pytest.raises(ValueError):
        app_mcp.CallDedup(ttl=-1)
    for d in (None, app_mcp.CallDedup.OFF, app_mcp.CallDedup(10, 4)):
        AppMcp("py-dedup", "Py", host_url="ws://127.0.0.1:9", call_dedup=d).close()


# ---------------------------------------------------------------------------
# 原生客户端（不连接 Host）
# ---------------------------------------------------------------------------


def test_register_and_errors(client):
    @client.tool("cart.add", description="加入购物车", risk="write")
    def add(sku: str, qty: int = 1):
        return {"ok": True}

    assert "cart.add" in client.tools
    with pytest.raises(app_mcp.AppMcpError.DuplicateName):
        client.add_tool(add, "cart.add")
    with pytest.raises(app_mcp.AppMcpError.InvalidName):
        client.add_tool(add, "bad name!")
    with pytest.raises(ValueError):
        client.add_tool(add, "x", risk="dangerous")

    client.tools["cart.add"].set_enabled(False)
    client.tools["cart.add"].update(description="新描述")

    with client.scope("page") as scope:
        scope.add_tool(lambda: 1, "page.one")
    # scope 已注销，可以重新注册同名工具
    client.add_tool(lambda: 1, "page.one")

    @client.resource("cart", description="购物车")
    def cart():
        return {"items": []}

    client.resources["cart"].notify_changed()
    assert client.state.status == app_mcp.StateStatus.IDLE
    assert client.instance_id


def test_stop_then_register_fails():
    c = AppMcp(app_id="unit-test", app_name="Unit", overview="单元测试")
    c.stop()
    with pytest.raises(app_mcp.AppMcpError.Stopped):
        c.add_tool(lambda: 1, "x")
    c.close()


# ---------------------------------------------------------------------------
# 界面级暴露与导航（spec/protocol.md 3.4）
# ---------------------------------------------------------------------------


class FakeNavigate:
    def __init__(self, page: str, params: dict | None = None, raw: str | None = None):
        self._page = page
        self._params = raw if raw is not None else (None if params is None else json.dumps(params))
        self.done = threading.Event()
        self.result = None

    def page(self):
        return self._page

    def params_json(self):
        return self._params

    def _finish(self, result):
        if self.done.is_set():
            raise ffi.AppMcpError.AlreadyCompleted()
        self.result = result
        self.done.set()

    def complete(self):
        self._finish(("ok",))

    def fail(self, message):
        self._finish(("fail", message))

    def deny(self, message):
        self._finish(("deny", message))

    def fail_user_action(self, message, reason, uri):
        self._finish(("user_action", message, reason, uri))

    def wait(self):
        assert self.done.wait(5), "导航未完成"
        return self.result


def _navigate(client, fn, request):
    from app_mcp._client import _NavigationAdapter

    _NavigationAdapter(client, fn).navigate(request)
    return request.wait()


def test_navigation_outcomes(client):
    seen = []

    def nav(page, params):
        seen.append((page, params, threading.current_thread().name))
        if page == "login":
            raise app_mcp.NavigationDenied("需要先登录")
        if page == "broken":
            raise RuntimeError("页面加载失败")

    assert _navigate(client, nav, FakeNavigate("cart", {"id": 1})) == ("ok",)
    assert seen[0][:2] == ("cart", {"id": 1})
    assert seen[0][2].startswith("app-mcp")  # 默认调度器（线程池）上执行
    assert _navigate(client, nav, FakeNavigate("login")) == ("deny", "需要先登录")
    assert _navigate(client, nav, FakeNavigate("broken")) == ("fail", "页面加载失败")
    # ToolCallError 的 NAVIGATION_DENIED 也按拒绝回复
    def deny_kind(page, params):
        raise ToolCallError("NAVIGATION_DENIED", "正在编辑")

    assert _navigate(client, deny_kind, FakeNavigate("x")) == ("deny", "正在编辑")

    # user_action_required → USER_ACTION_REQUIRED（不是 NAVIGATION_FAILED），reason / uri 原样带上
    def background(page, params):
        raise ToolCallError.user_action_required("已发通知，请点开", app_mcp.UserActionReason.FOREGROUND, "shop://cart")

    assert _navigate(client, background, FakeNavigate("cart")) == (
        "user_action",
        "已发通知，请点开",
        "foreground",
        "shop://cart",
    )

    def bare(page, params):
        raise ToolCallError.user_action_required("请先打开 App")

    assert _navigate(client, bare, FakeNavigate("cart")) == ("user_action", "请先打开 App", None, None)
    # 参数不是对象 / 不是 JSON：不调用回调，直接失败
    assert _navigate(client, nav, FakeNavigate("cart", raw="[1]"))[0] == "fail"
    assert _navigate(client, nav, FakeNavigate("cart", raw="{"))[0] == "fail"
    assert len(seen) == 3


def test_navigation_async_and_dispatcher():
    ran = []

    def dispatch(fn):
        ran.append("dispatched")
        fn()

    c = AppMcp(app_id="unit-nav", app_name="Unit", host_url="ws://127.0.0.1:9", dispatcher=dispatch)
    try:
        assert _navigate(c, lambda page, params: None, FakeNavigate("cart")) == ("ok",)
        assert ran == ["dispatched"]

        async def nav(page, params):
            await asyncio.sleep(0)
            raise app_mcp.NavigationDenied(f"不能去 {page}")

        assert _navigate(c, nav, FakeNavigate("cart")) == ("deny", "不能去 cart")
        c.set_navigation_handler(lambda page, params: None)

        @c.on_navigate
        def handler(page, params):
            pass

        c.set_navigation_handler(None)
    finally:
        c.close()


def test_navigate_in_background_option_and_setter():
    c = AppMcp(app_id="unit-nav-bg", app_name="Unit", host_url="ws://127.0.0.1:9", navigate_in_background=True)
    try:
        c.set_navigate_in_background(False)
    finally:
        c.close()


def test_surface_and_page(client):
    handle = client.add_tool(lambda: None, "v.tool", "依赖界面", surface="view", page="cart")
    assert (handle._spec.surface, handle._spec.page) == (ffi.ToolSurface.VIEW, "cart")
    # "app" 是缺省：不声明（不序列化）
    assert client.add_tool(lambda: None, "a.tool", "缺省", surface="app")._spec.surface is None
    handle.update(description="新")
    assert (handle._spec.surface, handle._spec.page) == (ffi.ToolSurface.VIEW, "cart")
    handle.update(surface=None, page=None)
    assert (handle._spec.surface, handle._spec.page) == (None, None)
    bg = client.add_tool(lambda: None, "v.bg", "后台替身", surface="view", background_tool="a.tool")
    assert bg._spec.background_tool == "a.tool"
    bg.update(description="新")
    assert bg._spec.background_tool == "a.tool"
    bg.update(background_tool=None)
    assert bg._spec.background_tool is None
    with pytest.raises(ValueError):
        client.add_tool(lambda: None, "bad.tool", "非法", surface="modal")
