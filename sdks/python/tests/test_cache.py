"""单元测试：结果缓存声明（spec/protocol.md 3.6）——转换、缺省、更新替换 / 清除、补丁型 update 与启停不丢、越界拒绝。不需要 Host。"""

from __future__ import annotations

import pytest

import app_mcp
from app_mcp import AppMcp
from app_mcp import app_mcp_uniffi as ffi


@pytest.fixture
def client():
    c = AppMcp(app_id="unit-cache", app_name="Unit", host_url="ws://127.0.0.1:9")
    yield c
    c.close()


PRIVATE_5S = ffi.CachePolicy(ttl_ms=5000, scope=None)
SHARED_1S = ffi.CachePolicy(ttl_ms=1000, scope=ffi.CacheScope.SHARED)


def test_cache_forms(client):
    assert client.add_tool(lambda: 1, "c.plain", "缺省", risk="read")._spec.cache is None
    assert client.add_tool(lambda: 1, "c.int", "整数", risk="read", cache=5000)._spec.cache == PRIVATE_5S
    as_dict = client.add_tool(lambda: 1, "c.dict", "字典", risk="read", cache={"ttl_ms": 1000, "scope": "shared"})
    assert as_dict._spec.cache == SHARED_1S
    explicit = client.add_tool(lambda: 1, "c.obj", "记录", risk="read", cache=app_mcp.CachePolicy(ttl_ms=7, scope=None))
    assert explicit._spec.cache == ffi.CachePolicy(ttl_ms=7, scope=None)

    @client.tool("c.deco", "装饰器", risk="read", cache={"ttl_ms": 1000, "scope": "shared"})
    def deco() -> int:
        return 1

    assert client.tools["c.deco"]._spec.cache == SHARED_1S
    for bad in ({"ttl": 1}, {"scope": "shared"}, {"ttl_ms": "1"}, {"ttl_ms": 1, "scope": "public"}, True, -1):
        with pytest.raises(ValueError):
            client.add_tool(lambda: 1, "c.bad", "非法", risk="read", cache=bad)


def test_update_keeps_replaces_and_clears(client):
    handle = client.add_tool(lambda: 1, "c.t", "读", risk="read", cache=5000)
    handle.update(description="新描述")
    handle.set_enabled(False)
    handle.set_enabled(True)
    assert handle._spec.cache == PRIVATE_5S, "补丁型 update / set_enabled 不得丢失 cache"
    with_cache = client.tools_hash
    handle.update(cache={"ttl_ms": 1000, "scope": "shared"})
    assert handle._spec.cache == SHARED_1S
    replaced = client.tools_hash
    assert replaced != with_cache, "替换后的声明同步到原生层（进 toolsHash）"
    handle.update(cache=None)
    assert handle._spec.cache is None
    assert client.tools_hash not in (with_cache, replaced), "清除后的声明同步到原生层"
    handle.update(cache=5000)
    assert client.tools_hash == with_cache


def test_out_of_range_ttl_rejected(client):
    for ttl in (0, 86_400_001):
        with pytest.raises(app_mcp.AppMcpError.InvalidConfig):
            client.add_tool(lambda: 1, "c.bad", "越界", risk="read", cache=ttl)
        with pytest.raises(app_mcp.AppMcpError.InvalidConfig):
            client.add_resource(lambda: {}, "r.bad", "越界", cache=ttl)
    handle = client.add_tool(lambda: 1, "c.ok", "上限", risk="read", cache=86_400_000)
    with pytest.raises(app_mcp.AppMcpError.InvalidConfig):
        handle.update(cache=0)
    assert handle._spec.cache == ffi.CachePolicy(ttl_ms=86_400_000, scope=None), "更新失败时保留原声明"
    client.add_resource(lambda: {}, "r.ok", "资源", cache={"ttl_ms": 30000, "scope": "shared"})

    @client.resource("r.deco", "装饰器", cache=1000)
    def deco() -> dict:
        return {}

    assert "r.deco" in client.resources
