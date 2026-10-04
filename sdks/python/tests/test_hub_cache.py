"""Hub SDK：只读结果缓存（spec/hub-api.md 3.20）——真实 App 声明 ``cache`` 的只读工具与资源 → 第二次命中（``cached_age_ms``、
App 只执行一次）、``cache_bypass`` 照常执行、``result_cache={"maxEntries": 0}`` 关闭、``status().cache`` 计数。

需要 ``bindings/hub-uniffi/scripts/generate.sh`` 与 ``bindings/uniffi/scripts/generate.sh`` 的生成物。
"""

from __future__ import annotations

import asyncio
import itertools
from typing import Any

import pytest

pytest.importorskip("app_mcp.hub", reason="没有 app_mcp_hub 生成物")

from app_mcp import AppMcp  # noqa: E402
from app_mcp.hub import CacheLimitOverrides, Hub  # noqa: E402

pytestmark = pytest.mark.hub


async def with_kv_app(hub: Hub, counts: dict[str, int]) -> AppMcp:
    """连上 ``hub`` 的 App ``kv``：声明 ``cache`` 的只读工具 ``get`` 与资源 ``snapshot``，每次执行计数。"""
    tick = itertools.count(1)

    def get() -> dict[str, Any]:
        counts["get"] += 1
        return {"n": next(tick)}

    def snapshot() -> dict[str, Any]:
        counts["snapshot"] += 1
        return {"v": counts["snapshot"]}

    events = hub.events()
    app = AppMcp("kv", "KV", host_url=f"ws://{hub.listen_addr}/app")
    app.add_tool(get, "get", "读取", annotations={"read_only_hint": True}, cache={"ttl_ms": 60_000})
    app.add_resource(snapshot, "snapshot", "快照", cache={"ttl_ms": 60_000, "scope": "shared"})
    app.start()
    await events.wait_for(lambda e: e.is_app_connected() and e.app_id == "kv", 10)
    events.close()
    loop = asyncio.get_running_loop()
    deadline = loop.time() + 10
    while not any(t.name == "kv.get" for t in hub.tools(apps=["kv"])):
        assert loop.time() < deadline, "等待工具 kv.get 超时"
        await asyncio.sleep(0.02)
    return app


def test_cache_hit_bypass_and_status() -> None:
    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False) as hub:
            counts = {"get": 0, "snapshot": 0}
            app = await with_kv_app(hub, counts)
            try:
                first = await hub.call_tool("kv.get")
                assert (first.ok, first.cached_age_ms) == (True, None)
                second = await hub.call_tool("kv.get")
                assert second.cached_age_ms is not None and second.cached_age_ms >= 0, second
                assert second.data == first.data and not second.woke
                assert counts["get"] == 1, "命中不转发给 App"

                fresh = await hub.call_tool("kv.get", cache_bypass=True)
                assert fresh.cached_age_ms is None and counts["get"] == 2, "绕过照常调用"
                again = await hub.call_tool("kv.get")
                assert again.data == fresh.data, "绕过的新结果覆盖缓存"

                for _ in range(2):
                    await hub.read_resource("app-mcp://kv/snapshot")
                assert counts["snapshot"] == 1, "声明了 cache 的资源第二次读取命中"

                cache = hub.status().cache
                assert cache is not None
                assert (cache.entries, cache.hits, cache.misses) == (2, 3, 2), cache
                assert cache.bytes > 0
            finally:
                app.close()

    asyncio.run(main())


def test_zero_max_entries_disables_cache() -> None:
    async def main() -> None:
        with Hub(listen="127.0.0.1:0", enable_ipc=False, result_cache={"maxEntries": 0}) as hub:
            counts = {"get": 0, "snapshot": 0}
            app = await with_kv_app(hub, counts)
            try:
                for _ in range(2):
                    assert (await hub.call_tool("kv.get")).cached_age_ms is None
                assert counts["get"] == 2
                cache = hub.status().cache
                assert cache is None or (cache.entries, cache.hits) == (0, 0), cache
            finally:
                app.close()

    asyncio.run(main())


def test_result_cache_config_forms() -> None:
    from app_mcp._hub_config import _result_cache

    assert _result_cache(None) is None
    assert _result_cache({"maxEntries": 0, "max_bytes": 4096}) == CacheLimitOverrides(
        max_entries=0, max_bytes=4096, max_entry_bytes=None
    )
    limits = CacheLimitOverrides(max_entries=3, max_bytes=None, max_entry_bytes=None)
    assert _result_cache(limits) is limits
    with pytest.raises(ValueError):
        _result_cache({"maxEntris": 1})
