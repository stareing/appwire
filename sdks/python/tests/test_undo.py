"""单元测试：撤销（spec/protocol.md 3.8）——``undoable`` 声明与补丁型 update、``ToolResult(undo=...)`` 的形状转换。不需要 Host。"""

from __future__ import annotations

import pytest

import app_mcp
from app_mcp import AppMcp, ToolResult
from app_mcp import app_mcp_uniffi as ffi


@pytest.fixture
def client():
    c = AppMcp(app_id="unit-undo", app_name="Unit", host_url="ws://127.0.0.1:9")
    yield c
    c.close()


def test_undoable_forms_and_update(client):
    assert client.add_tool(lambda: 1, "u.plain", "缺省")._spec.undoable is False
    handle = client.add_tool(lambda: 1, "u.t", "可撤销", undoable=True)
    assert handle._spec.undoable is True

    @client.tool("u.deco", "装饰器", undoable=True)
    def deco() -> int:
        return 1

    assert client.tools["u.deco"]._spec.undoable is True
    with_undoable = client.tools_hash
    handle.update(description="新描述")
    handle.set_enabled(False)
    handle.set_enabled(True)
    assert handle._spec.undoable is True, "补丁型 update / set_enabled 不得丢失 undoable"
    assert client.tools_hash != with_undoable  # 描述变了
    described = client.tools_hash
    handle.update(undoable=False)
    assert handle._spec.undoable is False
    assert client.tools_hash != described, "清除后的声明同步到原生层（进 toolsHash）"
    handle.update(undoable=True)
    assert client.tools_hash == described


def test_tool_result_undo_forms():
    assert ToolResult(1).undo is None
    full = ToolResult(1, undo={"tool": "todo.remove", "arguments": {"id": 3}, "label": "删除刚添加的待办"})
    assert full.undo == ffi.UndoAction(tool="todo.remove", arguments_json='{"id": 3}', label="删除刚添加的待办")
    assert ToolResult(undo={"tool": "t"}).undo == ffi.UndoAction(tool="t", arguments_json=None, label=None)
    record = app_mcp.UndoAction(tool="t", arguments_json='{"on":false}', label=None)
    assert ToolResult(undo=record).undo is record
    ffi_result = full._ffi("1", [])
    assert ffi_result.undo == full.undo, "完整结果带上 undo"
    # 内容不合法（非对象参数）不在封装层拒绝：交给核心去掉并记警告。
    assert ToolResult(undo={"tool": "t", "arguments": [1]}).undo.arguments_json == "[1]"
    for bad in ({"name": "t"}, {"arguments": {}}, {"tool": 1}, {"tool": "t", "label": 2}, {"tool": "t", "arguments": {1j}}, "t", 3):
        with pytest.raises(ValueError):
            ToolResult(undo=bad)
