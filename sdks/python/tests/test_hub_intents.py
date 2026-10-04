"""Hub SDK：标准意图（spec/intents.md 第 4 节）——真实 App 以 ``implements`` 声明 → ``HubTool.implements``、Agent 会话
``apps.intents``、:meth:`Hub.set_intent_defaults` / :meth:`Hub.intents` 与 ``status().intents``。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio

import pytest

pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp  # noqa: E402
from app_mcp.hub import Hub, HubError, IntentsStatus  # noqa: E402

pytestmark = pytest.mark.hub


async def implementations(hub: Hub) -> list[tuple[str, bool]]:
    """Agent 会话中 ``apps.intents {intent: "message.send"}`` 的实现者（工具全名, 是否默认）。"""
    out = await hub.call_tool("apps.intents", {"intent": "message.send"}, session="agent")
    assert out.error is None, out.error
    [entry] = [e for e in out.data["intents"] if e["intent"] == "message.send@1"]
    assert entry["known"] is True, entry
    return [(i["tool"], i.get("default", False)) for i in entry["implementations"]]


async def wait_tool(hub: Hub, name: str, timeout: float = 10.0):
    """App 连接事件之后工具列表才送达：轮询至出现。"""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not (found := [t for t in hub.tools(apps=["mail"]) if t.name == name]):
        assert loop.time() < deadline, f"等待工具 {name} 超时"
        await asyncio.sleep(0.02)
    return found[0]


def test_implements_listed_and_defaults_reorder() -> None:
    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False) as hub:
            events = hub.events()
            app = AppMcp("mail", "邮件", host_url=f"ws://{hub.listen_addr}/app")
            for name in ("a_send", "b_send"):
                app.add_tool(lambda to, text: None, name, "发邮件", implements=["message.send@1"])
            app.start()
            try:
                await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "mail", 10)
                b = await wait_tool(hub, "mail.b_send")
                assert b.implements == ["message.send@1"], "App 声明经 Hub 透传到 HubTool"
                assert hub.intents() == IntentsStatus(defaults={}, last_error=None)
                assert await implementations(hub) == [("mail.a_send", False), ("mail.b_send", False)]

                hub.set_intent_defaults({"message.send": "mail.b_send"})
                assert await implementations(hub) == [("mail.b_send", True), ("mail.a_send", False)], "默认排首位"
                st = hub.intents()
                assert (st.defaults, st.last_error) == ({"message.send": "mail.b_send"}, None)
                assert hub.status().intents == st

                with pytest.raises(HubError.Tool) as err:
                    hub.set_intent_defaults({"Bad Verb": "mail.a_send"})
                assert err.value.kind == "INVALID_INPUT"
                st = hub.intents()
                assert st.defaults == {"message.send": "mail.b_send"}, "旧值保留"
                assert st.last_error and "Bad Verb" in st.last_error
                with pytest.raises(TypeError):
                    hub.set_intent_defaults({"message.send": 1})  # type: ignore[dict-item]

                hub.set_intent_defaults({})
                assert hub.intents() == IntentsStatus(defaults={}, last_error=None)
            finally:
                app.close()

    asyncio.run(main())


def test_config_intent_defaults() -> None:
    with Hub(enable_listen=False, enable_ipc=False, intent_defaults={"link.open@1": "web.open"}) as hub:
        assert hub.intents().defaults == {"link.open@1": "web.open"}
    # 不合法时 Hub 照常启动：空表，原因记入 last_error。
    with Hub(enable_listen=False, enable_ipc=False, intent_defaults={"link.open": "nodot"}) as hub:
        st = hub.intents()
        assert st.defaults == {} and st.last_error and "nodot" in st.last_error
