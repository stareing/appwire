"""Hub SDK：撤销（spec/hub-api.md 3.23）——真实 App 声明 ``undoable`` 并在结果中给出 ``undo`` → ``HubTool.undoable``、
``CallResult.undo``；``apps.undo`` 的结果带 ``undo_of``；``status().undo`` 有值；不合法的 ``undo`` 被去掉、调用照常成功。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio
import logging

import pytest

pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp, ToolResult  # noqa: E402
from app_mcp.hub import Hub, UndoLimitOverrides, UndoOffer, UndoStatus  # noqa: E402

pytestmark = pytest.mark.hub


def toggle(on: bool = False) -> ToolResult:
    """开关：以相反值再调用自身即撤销。"""
    return ToolResult({"on": on}, undo={"tool": "toggle", "arguments": {"on": not on}, "label": "关掉开关" if on else "打开开关"})


def broken() -> ToolResult:
    return ToolResult({"ok": True}, undo={"tool": "bad name!"})


async def wait_tool(hub: Hub, name: str):
    loop = asyncio.get_running_loop()
    deadline = loop.time() + 10
    while True:
        found = [t for t in hub.tools() if t.name == name]
        if found:
            return found[0]
        assert loop.time() < deadline, f"等待工具 {name} 超时"
        await asyncio.sleep(0.02)


def test_undo_offer_undo_of_and_status(caplog) -> None:
    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False) as hub:
            events = hub.events()
            app = AppMcp("tg", "Toggle", host_url=f"ws://{hub.listen_addr}/app")
            app.add_tool(toggle, "toggle", "开关", undoable=True)
            app.add_tool(broken, "broken", "撤销信息不合法")
            app.start()
            try:
                await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "tg", 10)
                assert (await wait_tool(hub, "tg.toggle")).undoable is True
                assert (await wait_tool(hub, "tg.broken")).undoable is False
                assert (await wait_tool(hub, "apps.list")).undoable is False, "内置工具为 False"

                first = await hub.call_tool("tg.toggle", {"on": True})
                assert first.ok and first.data == {"on": True}, first
                assert isinstance(first.undo, UndoOffer) and first.undo.label == "关掉开关", first.undo
                assert 0 < first.undo.expires_in_ms <= 30 * 60 * 1000
                assert first.undo_of is None

                st = hub.status().undo
                assert st == UndoStatus(ttl_ms=30 * 60 * 1000, max_per_task=32, records=1), st

                undone = await hub.call_tool("apps.undo", {})
                assert undone.ok and undone.data == {"on": False}, undone
                assert undone.undo_of == first.call_id
                assert undone.undo is not None and undone.undo.label == "打开开关", "逆调用结果再次登记（重做）"
                again = await hub.call_tool("apps.undo", {"callId": first.call_id})
                assert again.error is not None and again.error.kind == "TOOL_NOT_FOUND", "只能撤销一次"

                with caplog.at_level(logging.WARNING, logger="app_mcp"):
                    bad = await hub.call_tool("tg.broken", {})
                assert bad.ok and bad.data == {"ok": True} and bad.undo is None, bad
                assert any("undo" in r.getMessage() for r in caplog.records), caplog.text
            finally:
                events.close()
                app.close()

    asyncio.run(main())


def test_undo_limits_config() -> None:
    """``undo``：字典（JSON 键或 snake_case）与 ``UndoLimitOverrides`` 均可；``max_per_task=0`` 关闭；未知键与非法上限报错。"""
    with Hub(enable_listen=False, enable_ipc=False, undo={"ttlMs": 5000}) as hub:
        st = hub.status().undo
        assert (st.ttl_ms, st.max_per_task) == (5000, 32)
        assert any(t.name == "apps.undo" for t in hub.tools())
    with Hub(enable_listen=False, enable_ipc=False, undo=UndoLimitOverrides(ttl_ms=None, max_per_task=0)) as hub:
        assert hub.status().undo.max_per_task == 0
        assert not any(t.name == "apps.undo" for t in hub.tools())
    with Hub(enable_listen=False, enable_ipc=False, undo={"max_per_task": 3}) as hub:
        assert hub.status().undo.max_per_task == 3
    with pytest.raises(ValueError):
        Hub(enable_listen=False, enable_ipc=False, undo={"ttl": 1})
    with pytest.raises(Exception, match="ttlMs"):
        Hub(enable_listen=False, enable_ipc=False, undo={"ttlMs": 0})
