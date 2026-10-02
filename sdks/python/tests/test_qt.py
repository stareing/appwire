"""app_mcp.qt：view 工具按控件显示 / 隐藏启用（不需要安装 Qt：用假的 QObject / QEvent）。"""

from __future__ import annotations

import enum
from typing import Any

import pytest

from app_mcp import qt
from app_mcp.qt import ViewToolBinding, bind_view_tool


class FakeHandle:
    def __init__(self) -> None:
        self.log: list[Any] = []

    def set_enabled(self, enabled: bool) -> None:
        self.log.append(enabled)

    def dispose(self) -> None:
        self.log.append("dispose")


class FakeEvent:
    class Type(enum.Enum):
        Show = 17
        Hide = 18
        Paint = 12

    def __init__(self, t: FakeEvent.Type) -> None:
        self._t = t

    def type(self) -> FakeEvent.Type:
        return self._t


class FakeQObject:
    def __init__(self, parent: Any = None) -> None:
        self.parent = parent


class FakeSignal:
    def __init__(self) -> None:
        self.slots: list[Any] = []

    def connect(self, fn: Any) -> None:
        self.slots.append(fn)

    def emit(self) -> None:
        for fn in self.slots:
            fn()


class FakeWidget:
    def __init__(self, visible: bool) -> None:
        self.visible = visible
        self.filters: list[Any] = []
        self.destroyed = FakeSignal()

    def isVisible(self) -> bool:  # noqa: N802
        return self.visible

    def installEventFilter(self, f: Any) -> None:  # noqa: N802
        self.filters.append(f)

    def removeEventFilter(self, f: Any) -> None:  # noqa: N802
        self.filters.remove(f)

    def send(self, t: FakeEvent.Type, visible: bool) -> None:
        self.visible = visible
        for f in list(self.filters):
            f.eventFilter(self, FakeEvent(t))


@pytest.fixture
def fake_qt(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(qt, "_qt", lambda: (FakeQObject, FakeEvent))


def test_binding_state_machine() -> None:
    h = FakeHandle()
    b = ViewToolBinding(h, visible=False)
    b.set_visible(False)
    b.set_visible(True)
    b.set_visible(True)
    b.destroyed()
    b.set_visible(False)
    assert h.log == [False, True, "dispose"]

    h2 = FakeHandle()
    b2 = ViewToolBinding(h2, visible=True, dispose_on_destroy=False)
    b2.destroyed()
    assert h2.log == [True, False]


def test_bind_view_tool_follows_show_hide(fake_qt: None) -> None:
    w, h = FakeWidget(visible=False), FakeHandle()
    unbind = bind_view_tool(w, h)
    w.send(FakeEvent.Type.Show, visible=True)
    w.send(FakeEvent.Type.Paint, visible=True)
    # 父控件隐藏：子控件收到 Hide
    w.send(FakeEvent.Type.Hide, visible=False)
    # Show 事件到达但祖先仍隐藏（isVisible 为假）：不启用
    w.send(FakeEvent.Type.Show, visible=False)
    w.send(FakeEvent.Type.Show, visible=True)
    assert h.log == [False, True, False, True]
    unbind()
    assert w.filters == []
    w.destroyed.emit()
    assert h.log == [False, True, False, True]


def test_bind_view_tool_disposes_on_destroy(fake_qt: None) -> None:
    w, h = FakeWidget(visible=True), FakeHandle()
    bind_view_tool(w, h)
    w.destroyed.emit()
    assert h.log == [True, "dispose"]


def test_missing_qt_raises(monkeypatch: pytest.MonkeyPatch) -> None:
    import builtins

    real_import = builtins.__import__

    def no_qt(name: str, *args: Any, **kwargs: Any) -> Any:
        if name.startswith(("PySide6", "PyQt6")):
            raise ImportError(name)
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", no_qt)
    with pytest.raises(ImportError, match="PySide6 或 PyQt6"):
        bind_view_tool(FakeWidget(visible=True), FakeHandle())
