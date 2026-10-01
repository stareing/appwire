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
    assert {"RATE_LIMITED", "PAYLOAD_TOO_LARGE"} <= app_mcp.ERROR_KINDS


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
