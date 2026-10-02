"""Qt 绑定（第 4c 项 E，spec/protocol.md 3.4）：``view`` 工具只在所在控件显示时启用。

PySide6 / PyQt6 为可选依赖，只在调用时导入（按此顺序尝试）。

>>> from app_mcp.qt import bind_view_tool
>>> handle = client.add_tool(checkout, "cart.checkout", surface="view", page="cart", enabled=False)
>>> bind_view_tool(cart_page, handle)          # cart_page 显示时启用、隐藏时禁用、销毁时注销

与重写 ``showEvent`` / ``hideEvent`` 等价，但用事件过滤器实现，不需要继承控件类。
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any, Protocol

__all__ = ["bind_view_tool", "ViewToolBinding"]


class _Toggle(Protocol):
    def set_enabled(self, enabled: bool) -> None: ...

    def dispose(self) -> None: ...


class ViewToolBinding:
    """可见性状态机（与 Qt 无关，便于测试）：``show`` / ``hide`` / ``destroyed`` → 启用 / 禁用 / 注销。

    @invariant 状态不变时不重复调用 ``set_enabled``；``destroyed`` 之后忽略后续事件。
    """

    def __init__(self, handle: _Toggle, *, visible: bool, dispose_on_destroy: bool = True) -> None:
        self._handle = handle
        self._dispose_on_destroy = dispose_on_destroy
        self._enabled: bool | None = None
        self._done = False
        self.set_visible(visible)

    def set_visible(self, visible: bool) -> None:
        if self._done or self._enabled == visible:
            return
        self._enabled = visible
        self._handle.set_enabled(visible)

    def detach(self) -> None:
        """之后忽略全部事件（不改变工具当前状态）。"""
        self._done = True

    def destroyed(self) -> None:
        if self._done:
            return
        self._done = True
        if self._dispose_on_destroy:
            self._handle.dispose()
        elif self._enabled:
            self._handle.set_enabled(False)


def _qt() -> tuple[Any, Any]:
    """返回 ``(QObject, QEvent)``；都没有安装时抛 ``ImportError``。"""
    try:
        from PySide6.QtCore import QEvent, QObject  # type: ignore
    except ImportError:
        try:
            from PyQt6.QtCore import QEvent, QObject  # type: ignore
        except ImportError as e:
            raise ImportError("app_mcp.qt 需要 PySide6 或 PyQt6") from e
    return QObject, QEvent


def bind_view_tool(widget: Any, handle: _Toggle, *, dispose_on_destroy: bool = True) -> Callable[[], None]:
    """把 ``view`` 工具的启用状态绑定到 ``widget`` 的显示 / 隐藏（含所在窗口最小化、切换 ``QStackedWidget`` /
    ``QTabWidget`` 页面）；控件销毁时注销（``dispose_on_destroy=False`` 时改为禁用）。

    必须在 Qt 主线程调用。返回的函数解除绑定（不改变工具当前状态）。

    @why 用 ``isVisible()`` 判定而不是只看事件类型：父控件隐藏时子控件收到 Hide 但 ``isHidden()`` 为假，
         ``isVisible()`` 才反映"真正可见"。
    """
    q_object, q_event = _qt()
    show, hide = q_event.Type.Show, q_event.Type.Hide
    binding = ViewToolBinding(handle, visible=bool(widget.isVisible()), dispose_on_destroy=dispose_on_destroy)

    class _Filter(q_object):  # type: ignore[misc, valid-type]
        def eventFilter(self, obj: Any, event: Any) -> bool:  # noqa: N802 - Qt 接口名
            if event.type() in (show, hide):
                binding.set_visible(bool(widget.isVisible()) and event.type() == show)
            return False

    event_filter = _Filter(widget)  # 父对象为 widget：随控件销毁
    widget.installEventFilter(event_filter)
    widget.destroyed.connect(lambda *_: binding.destroyed())

    def unbind() -> None:
        binding.detach()
        widget.removeEventFilter(event_filter)

    return unbind
