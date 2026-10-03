"""Qt 兜底的常量、按需导入与控件辅助函数（纯移动自 ``_qt_elements.py``）。所有函数只在 Qt GUI 线程调用。"""

from __future__ import annotations

import functools
import re
from types import SimpleNamespace
from typing import Any

from . import _format as fmt
from ._input import UiKey, UiScrollDirection

DECLARED_PROPERTY = "appMcpTools"
"""控件 / QAction 的动态属性：已声明的工具名（逗号分隔），由 ``declare_mcp_tools`` 写入。"""

ITEM_SCAN_MAX = fmt.LIMIT_MAX
"""每个列表 / 树 / 表格最多展开的可见项（B-07：大模型不逐行遍历）。"""

_FOCUS_CHAIN_MAX = 10000
_POPUP_CLOSE_MAX = 20
_OPTIONS_LISTED = 20
_RICH_TAG = re.compile(r"<[A-Za-z/!][^>]*>")


@functools.cache
def qt() -> SimpleNamespace:
    """``SimpleNamespace(core=QtCore, gui=QtGui, widgets=QtWidgets)``；都没有安装时抛 ``ImportError``。"""
    try:
        from PySide6 import QtCore, QtGui, QtWidgets  # type: ignore
    except ImportError:
        try:
            from PyQt6 import QtCore, QtGui, QtWidgets  # type: ignore
        except ImportError as e:
            raise ImportError("app_mcp.uifallback.qt 需要 PySide6 或 PyQt6") from e
    return SimpleNamespace(core=QtCore, gui=QtGui, widgets=QtWidgets)


def is_alive(obj: Any) -> bool:
    """C++ 对象仍存在（PySide6 / PyQt6 对已删除对象的调用都抛 ``RuntimeError``）。"""
    try:
        obj.objectName()
    except RuntimeError:
        return False
    return True


def strip_mnemonic(text: str) -> str:
    """去掉助记符 ``&``（``&&`` → ``&``）。"""
    return text.replace("&&", "\0").replace("&", "").replace("\0", "&")


def declared_of(obj: Any) -> str | None:
    value = obj.property(DECLARED_PROPERTY)
    return value if isinstance(value, str) and value else None


def _accessible_name(w: Any) -> str:
    """名称：``accessibleName`` → Qt 无障碍接口给出的名称（含伙伴标签 ``QLabel.setBuddy`` / 表单布局标签）。"""
    name = w.accessibleName()
    if name:
        return name
    q = qt()
    iface = q.gui.QAccessible.queryAccessibleInterface(w)
    return iface.text(q.gui.QAccessible.Text.Name) if iface is not None else ""


def _label_name(w: Any) -> str:
    """名称：``accessibleName`` → 伙伴标签（``QLabel.setBuddy`` / ``QFormLayout.addRow``）→ 提示文字。

    @why 下拉框、数字框的 Qt 无障碍名称是当前值而不是标签，因此不用无障碍接口取名称。
    """
    name = w.accessibleName()
    if name:
        return name
    for label in w.window().findChildren(qt().widgets.QLabel):
        if label.buddy() is w:
            return strip_mnemonic(label.text())
    return w.toolTip()


def _label_text(label: Any) -> str:
    """``QLabel`` 的文字；富文本去掉标签（纯文本格式原样）。"""
    text = label.text()
    return text if label.textFormat() == qt().core.Qt.TextFormat.PlainText else _RICH_TAG.sub(" ", text)


def _check_states(state: Any) -> tuple[str, ...]:
    cs = qt().core.Qt.CheckState
    return {cs.Checked: ("checked",), cs.PartiallyChecked: ("mixed",)}.get(state, ("unchecked",))


def _scroll_area_of(w: Any) -> Any | None:
    """``w`` 自身或最近的 ``QAbstractScrollArea`` 祖先（视口内的控件归其所在滚动区）。"""
    area_type = qt().widgets.QAbstractScrollArea
    while w is not None:
        if isinstance(w, area_type):
            return w
        w = w.parentWidget()
    return None


def _scroll_page(area: Any | None, direction: UiScrollDirection) -> bool:
    if area is None:
        return False
    vertical = direction in (UiScrollDirection.UP, UiScrollDirection.DOWN)
    bar = area.verticalScrollBar() if vertical else area.horizontalScrollBar()
    action = qt().widgets.QAbstractSlider.SliderAction
    before = bar.value()
    bar.triggerAction(
        action.SliderPageStepAdd if direction in (UiScrollDirection.DOWN, UiScrollDirection.RIGHT) else action.SliderPageStepSub
    )
    return bar.value() != before


_KEY_MAP: dict[UiKey, tuple[str, str, str]] = {
    UiKey.ENTER: ("Key_Return", "NoModifier", "\r"),
    UiKey.ESCAPE: ("Key_Escape", "NoModifier", "\x1b"),
    UiKey.TAB: ("Key_Tab", "NoModifier", "\t"),
    UiKey.SHIFT_TAB: ("Key_Backtab", "ShiftModifier", ""),
    UiKey.SPACE: ("Key_Space", "NoModifier", " "),
}


def send_key(target: Any, key: UiKey) -> bool:
    """按下 + 抬起交给 ``target``（未处理时 Qt 沿父控件传递）；返回是否有控件接受。"""
    q = qt()
    key_name, modifier, text = _KEY_MAP[key]
    code, mods = getattr(q.core.Qt.Key, key_name), getattr(q.core.Qt.KeyboardModifier, modifier)
    accepted = False
    for kind in (q.core.QEvent.Type.KeyPress, q.core.QEvent.Type.KeyRelease):
        if not is_alive(target):
            break
        event = q.gui.QKeyEvent(kind, code, mods, text)
        q.widgets.QApplication.sendEvent(target, event)
        accepted = accepted or event.isAccepted()
    return accepted


def close_popups() -> None:
    app = qt().widgets.QApplication
    for _ in range(_POPUP_CLOSE_MAX):
        popup = app.activePopupWidget()
        if popup is None:
            return
        popup.close()
