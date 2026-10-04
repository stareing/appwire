"""Hub SDK：App 事件、订阅与信箱（spec/hub-api.md 3.17）——真实 App 端 ``emit_event`` → Agent 会话
``apps.events.subscribe`` / ``apps.events`` 取件、``status().events``、``HubEvent.APP_EVENT`` 与 :meth:`Hub.set_event_handler`。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio
import json
import threading
import time

import pytest

pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp  # noqa: E402
from app_mcp.hub import AppEvent, EventsStatus, Hub  # noqa: E402

pytestmark = pytest.mark.hub


def emit_when_connected(app: AppMcp, name: str, payload: dict | None = None, timeout: float = 10.0) -> None:
    """握手完成前 ``emit_event`` 返回 False（丢弃），重试至发出。"""
    deadline = time.monotonic() + timeout
    while not app.emit_event(name, payload):
        assert time.monotonic() < deadline, "等待 App 连接超时"
        time.sleep(0.02)


def test_app_event_subscribe_fetch_status_and_handler() -> None:
    received: list[AppEvent] = []
    dispatched: list[str] = []
    seen = threading.Event()

    def handler(event: AppEvent) -> None:
        received.append(event)
        seen.set()

    def dispatcher(fn) -> None:
        dispatched.append(threading.current_thread().name)
        fn()

    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False) as hub:
            hub.set_event_handler(lambda _e: (_ for _ in ()).throw(RuntimeError("回调崩溃")))  # 异常只记日志
            hub.set_event_handler(handler, dispatcher=dispatcher)
            events = hub.events()
            app = AppMcp("shop", "商城", host_url=f"ws://{hub.listen_addr}/app")
            app.declare_event("order.shipped", "订单已发货", {"type": "object"})
            app.start()
            try:
                await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "shop", 10)
                sub = await hub.call_tool("apps.events.subscribe", {"appId": "shop", "event": "order.shipped"}, session="s1")
                assert sub.error is None, sub.error
                sub_id = sub.data["subscriptionId"]

                await asyncio.to_thread(emit_when_connected, app, "order.shipped", {"orderId": "o1"})
                e = await events.wait_for(lambda e: e.is_app_event(), 10)
                ev = e.event
                assert (ev.app_id, ev.name) == ("shop", "order.shipped")
                assert json.loads(ev.payload_json) == {"orderId": "o1"}
                assert ev.id.startswith("ev-") and ev.at_ms > 0
                assert await asyncio.to_thread(seen.wait, 10)
                assert received == [ev] and len(dispatched) == 1

                status = hub.status().events
                assert isinstance(status, EventsStatus) and status.dropped_invalid == 0
                [s] = status.subscriptions
                assert (s.subscription_id, s.app_id, s.event, s.delivered, s.dropped, s.pending) == (
                    sub_id, "shop", "order.shipped", 1, 0, 1,
                )

                inbox = await hub.call_tool("apps.events", session="s1")
                assert inbox.error is None, inbox.error
                assert [(x["name"], x["payload"]) for x in inbox.data["events"]] == [("order.shipped", {"orderId": "o1"})]
                assert inbox.data["pending"] == 0
                other = await hub.call_tool("apps.events", session="s2")
                assert other.data["events"] == [], "信箱按订阅方隔离"

                # 清除回调后事件流照常，回调不再收到
                hub.set_event_handler(None)
                await asyncio.to_thread(emit_when_connected, app, "order.shipped")
                e = await events.wait_for(lambda e: e.is_app_event(), 10)
                assert e.event.payload_json is None
                assert len(received) == 1
            finally:
                app.stop()

    asyncio.run(main())


def test_event_limits_override_applies() -> None:
    """``event_limits`` 字典：maxSubscriptions = 1 时第二个订阅 RATE_LIMITED；maxInboxEvents = 1 时只留最新一条、丢弃计数。"""

    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False, event_limits={"maxSubscriptions": 1, "maxInboxEvents": 1}) as hub:
            events = hub.events()
            app = AppMcp("shop", "商城", host_url=f"ws://{hub.listen_addr}/app")
            app.declare_event("order.shipped", "订单已发货")
            app.start()
            try:
                await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "shop", 10)
                sub = await hub.call_tool("apps.events.subscribe", {"appId": "shop", "event": "order.shipped"}, session="s1")
                assert sub.error is None, sub.error
                second = await hub.call_tool("apps.events.subscribe", {"appId": "shop"}, session="s1")
                assert second.error is not None and second.error.kind == "RATE_LIMITED", second.error
                assert json.loads(second.error.details_json or "{}").get("scope") == "events"

                for n in (1, 2):
                    await asyncio.to_thread(emit_when_connected, app, "order.shipped", {"n": n})
                    await events.wait_for(lambda e: e.is_app_event(), 10)
                inbox = await hub.call_tool("apps.events", session="s1")
                assert [x["payload"] for x in inbox.data["events"]] == [{"n": 2}]
                assert inbox.data["dropped"] == 1
            finally:
                app.stop()

    asyncio.run(main())


def test_event_limits_rejects_unknown_key() -> None:
    with pytest.raises(ValueError, match="event_limits"):
        Hub(enable_listen=False, enable_ipc=False, event_limits={"maxEvents": 1})
