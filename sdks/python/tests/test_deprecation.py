"""单元测试：工具弃用声明（spec/protocol.md 3.7）——形式转换、缺省、更新替换 / 清除、补丁型 update 与启停不丢、非法拒绝。不需要 Host。"""

from __future__ import annotations

import pytest

import app_mcp
from app_mcp import AppMcp
from app_mcp import app_mcp_uniffi as ffi


@pytest.fixture
def client():
    c = AppMcp(app_id="unit-deprecated", app_name="Unit", host_url="ws://127.0.0.1:9")
    yield c
    c.close()


FULL = ffi.Deprecation(message="改用 d.new", replacement="d.new", until="2027-06-30")
ONLY_MESSAGE = ffi.Deprecation(message="即将移除", replacement=None, until=None)


def test_deprecation_forms(client):
    assert client.add_tool(lambda: 1, "d.plain", "缺省")._spec.deprecated is None
    assert client.add_tool(lambda: 1, "d.str", "字符串", deprecated="即将移除")._spec.deprecated == ONLY_MESSAGE
    as_dict = {"message": "改用 d.new", "replacement": "d.new", "until": "2027-06-30"}
    assert client.add_tool(lambda: 1, "d.dict", "字典", deprecated=as_dict)._spec.deprecated == FULL
    record = app_mcp.Deprecation(message="改用 d.new", replacement="d.new", until="2027-06-30")
    assert client.add_tool(lambda: 1, "d.obj", "记录", deprecated=record)._spec.deprecated == FULL

    @client.tool("d.deco", "装饰器", deprecated={"message": "即将移除"})
    def deco() -> int:
        return 1

    assert client.tools["d.deco"]._spec.deprecated == ONLY_MESSAGE
    for bad in ({"msg": "x"}, {"replacement": "d.new"}, {"message": 1}, {"message": "m", "until": 20270630}, 3, ["m"]):
        with pytest.raises(ValueError):
            client.add_tool(lambda: 1, "d.bad", "非法", deprecated=bad)


def test_update_keeps_replaces_and_clears(client):
    handle = client.add_tool(lambda: 1, "d.t", "旧", deprecated=FULL)
    plain = client.add_tool(lambda: 1, "d.u", "未弃用")
    handle.update(description="新描述")
    handle.set_enabled(False)
    handle.set_enabled(True)
    assert handle._spec.deprecated == FULL, "补丁型 update / set_enabled 不得丢失 deprecated"
    with_full = client.tools_hash
    handle.update(deprecated="即将移除")
    assert handle._spec.deprecated == ONLY_MESSAGE
    replaced = client.tools_hash
    assert replaced != with_full, "替换后的声明同步到原生层（进 toolsHash）"
    handle.update(deprecated=None)
    assert handle._spec.deprecated is None
    assert client.tools_hash not in (with_full, replaced), "清除后的声明同步到原生层"
    handle.update(deprecated=FULL)
    assert client.tools_hash == with_full
    plain.update(deprecated={"message": "改用 d.t", "replacement": "d.t"})
    assert plain._spec.deprecated == ffi.Deprecation(message="改用 d.t", replacement="d.t", until=None)


def test_update_preserves_every_field(client):
    """补丁型 update 复制全部字段：未给出的字段（含将来新增的）原样保留。"""
    handle = client.add_tool(
        lambda: 1, "d.all", "全字段", risk="read", title="T", page="p", exclusive="g", implements=["message.send@1"],
        cache=5000, deprecated=FULL, concurrency=2, output_schema={"type": "object"},
    )
    before = dict(vars(handle._spec))
    handle.update(description="变了")
    after = dict(vars(handle._spec))
    assert after.pop("description") == "变了"
    before.pop("description")
    assert after == before


def test_invalid_deprecation_rejected(client):
    bad = [
        {"message": ""},
        {"message": "   "},
        {"message": "x" * 501},
        {"message": "m", "replacement": "bad name!"},
        {"message": "m", "replacement": "d.bad"},
        {"message": "m", "until": "2027-02-30"},
        {"message": "m", "until": "2027/06/30"},
    ]
    for d in bad:
        with pytest.raises(app_mcp.AppMcpError.InvalidConfig):
            client.add_tool(lambda: 1, "d.bad", "非法", deprecated=d)
    handle = client.add_tool(lambda: 1, "d.ok", "合法", deprecated={"message": "x" * 500, "replacement": "d.missing", "until": "2028-02-29"})
    with pytest.raises(app_mcp.AppMcpError.InvalidConfig):
        handle.update(deprecated={"message": "m", "replacement": "d.ok"})
    assert handle._spec.deprecated.replacement == "d.missing", "更新失败时保留原声明"
