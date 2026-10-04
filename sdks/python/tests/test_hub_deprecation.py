"""Hub SDK：工具演进（spec/hub-api.md 3.21）——真实 App 声明弃用工具 → ``HubTool.deprecated`` 原样、``schema_hash`` 有值；
schema 变化后 ``schema_hash`` 变化，破坏性变化记入 ``status().schema_changes``。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio
from collections.abc import Callable

import pytest

pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp  # noqa: E402
from app_mcp.hub import ChangeLevel, Deprecation, Hub, HubTool  # noqa: E402

pytestmark = pytest.mark.hub


async def wait_tool(hub: Hub, name: str, pred: Callable[[HubTool], bool] = lambda _: True) -> HubTool:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + 10
    while True:
        found = [t for t in hub.tools() if t.name == name and pred(t)]
        if found:
            return found[0]
        assert loop.time() < deadline, f"等待工具 {name} 超时"
        await asyncio.sleep(0.02)


def test_deprecated_tool_and_schema_hash() -> None:
    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False) as hub:
            events = hub.events()
            app = AppMcp("lib", "Lib", host_url=f"ws://{hub.listen_addr}/app")
            schema = {"type": "object", "properties": {"q": {"type": "string"}}}
            old = app.add_tool(
                lambda q="": {}, "q.old", "旧版查询", input_schema=schema,
                deprecated={"message": "改用 q.new", "replacement": "q.new", "until": "2027-06-30"},
            )
            app.add_tool(lambda: {}, "q.new", "新版查询")
            app.start()
            try:
                await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "lib", 10)
                t = await wait_tool(hub, "lib.q.old")
                assert t.deprecated == Deprecation(message="改用 q.new", replacement="q.new", until="2027-06-30")
                assert t.schema_hash is not None and len(t.schema_hash) == 16, t.schema_hash
                fresh = await wait_tool(hub, "lib.q.new")
                assert fresh.deprecated is None and fresh.schema_hash is not None
                builtin = await wait_tool(hub, "apps.list")
                assert (builtin.deprecated, builtin.schema_hash) == (None, None), "内置工具不带"

                old.update(input_schema={**schema, "required": ["q"]})
                changed = await wait_tool(hub, "lib.q.old", lambda x: x.schema_hash != t.schema_hash)
                assert changed.deprecated == t.deprecated, "更新 schema 不丢弃用声明"
                records = hub.status().schema_changes or []
                rec = next(r for r in records if (r.app_id, r.tool) == ("lib", "q.old"))
                assert rec.level == ChangeLevel.BREAKING and rec.changes and rec.at > 0, rec
            finally:
                events.close()
                app.close()

    asyncio.run(main())
