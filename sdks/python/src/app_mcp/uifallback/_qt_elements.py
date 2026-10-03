"""Qt Widgets 控件 → 兜底节点（spec/ui-fallback.md 8.4 Qt 列）。所有方法只在 Qt GUI 线程调用。

PySide6 / PyQt6 为可选依赖：首次调用 :func:`qt` 时才导入（按此顺序尝试）。
"""

from __future__ import annotations

from collections.abc import Callable, Hashable, Sequence
from typing import Any

from . import _format as fmt
from . import _input as inp
from ._format import UiEntryKind
from ._input import UiKey, UiScrollDirection
from ._tree import TRANSPARENT, UiDescription, UiElement, UiWindow

# @compat 以下名称已拆分到子模块，经本模块再导出以保持原导入路径
from ._qt_support import (  # noqa: F401
    DECLARED_PROPERTY, ITEM_SCAN_MAX, _FOCUS_CHAIN_MAX, _POPUP_CLOSE_MAX, _OPTIONS_LISTED, _RICH_TAG, qt,
    is_alive, strip_mnemonic, declared_of, _accessible_name, _label_name, _label_text, _check_states,
    _scroll_area_of, _scroll_page, _KEY_MAP, send_key, close_popups,
)
from ._qt_items import (  # noqa: F401
    TabElement, _index_path, ItemElement, _item_role, _visible_indexes, item_at, item_children, ActionElement,
    action_children,
)

__all__ = ["DECLARED_PROPERTY", "qt", "is_alive", "QtWidgetElement", "QtWindowElement", "declared_of"]


# ---------------------------------------------------------------------------
# 控件
# ---------------------------------------------------------------------------


class QtWidgetElement(UiElement):
    """一个 ``QWidget``。分类与动作按控件类型查规则表（:data:`_RULES`）。"""

    def __init__(self, widget: Any) -> None:
        self.w = widget

    @property
    def identity(self) -> Any:
        return self.w

    def _rule(self) -> _Rule:
        return _rule_of(self.w)

    def is_hidden(self) -> bool:
        w = self.w
        return self._rule().ignored or not w.isVisible() or w.visibleRegion().isEmpty()

    def describe(self) -> UiDescription:
        return self._rule().describe(self.w)

    def children(self) -> Sequence[UiElement]:
        rule = self._rule()
        if rule.children is not None:
            return rule.children(self.w)
        return widget_children(self.w)

    def locate(self, identity: Any, key: Hashable) -> UiElement | None:
        if identity is not self.w or not isinstance(key, tuple) or not isinstance(self.w, qt().widgets.QAbstractItemView):
            return None
        return item_at(self.w, key)

    def read_text(self) -> list[str]:
        return self._rule().read(self.w)

    def read_children(self) -> bool:
        return self._rule().read_children

    def is_enabled(self) -> bool:
        return bool(self.w.isEnabled())

    def click(self) -> bool:
        return self._rule().click(self.w)

    def set_text(self, text: str) -> bool:
        return self._rule().set_text(self.w, text)

    def select_option(self, text: str) -> bool:
        return self._rule().select_option(self.w, text)

    def set_range(self, value: float) -> bool:
        return self._rule().set_range(self.w, value)

    def focus(self) -> bool:
        w = self.w
        if w.focusPolicy() == qt().core.Qt.FocusPolicy.NoFocus:
            return False
        w.setFocus(qt().core.Qt.FocusReason.OtherFocusReason)
        return True

    def ime_action(self) -> bool:
        # @why QLineEdit 处理 Return 时发 returnPressed / editingFinished 后 ignore 事件，交给对话框的默认按钮：
        #      事件是否被接受不代表是否已提交，因此对单行输入框视为已处理。
        if isinstance(self.w, qt().widgets.QLineEdit):
            send_key(self.w, UiKey.ENTER)
            return True
        return False

    def scroll_into_view(self) -> bool:
        area_type = qt().widgets.QScrollArea
        parent = self.w.parentWidget()
        while parent is not None:
            if isinstance(parent, area_type) and parent.widget() is not None and parent.widget().isAncestorOf(self.w):
                parent.ensureWidgetVisible(self.w)
                return True
            parent = parent.parentWidget()
        return False

    def scroll_page(self, direction: UiScrollDirection) -> bool:
        return _scroll_page(_scroll_area_of(self.w), direction)


class QtWindowElement(QtWidgetElement, UiWindow):
    """顶层窗口：主窗口 / 对话框 / 弹出层（菜单、下拉框弹出列表）。"""

    def is_hidden(self) -> bool:
        return not self.w.isVisible() or self.w.isMinimized()

    def _focus_target(self) -> Any:
        focus = qt().widgets.QApplication.focusWidget()
        if focus is not None and focus.window() is self.w:
            return focus
        return self.w.focusWidget() or self.w

    def send_key(self, key: UiKey) -> bool:
        return send_key(self._focus_target(), key)

    def move_focus(self, forward: bool) -> bool:
        """沿焦点链找下一个可用 Tab 聚焦的控件（与 ``QWidget::focusNextPrevChild`` 的选择条件一致，只用公开接口）。"""
        q = qt()
        start = self._focus_target()
        tab = q.core.Qt.FocusPolicy.TabFocus
        current = start
        for _ in range(_FOCUS_CHAIN_MAX):
            current = current.nextInFocusChain() if forward else current.previousInFocusChain()
            if current is None or current is start:
                return False
            if (
                current.window() is self.w
                and current.isVisible()
                and current.isEnabled()
                and (current.focusPolicy() & tab) == tab
                and current.focusProxy() is None
            ):
                current.setFocus(q.core.Qt.FocusReason.TabFocusReason if forward else q.core.Qt.FocusReason.BacktabFocusReason)
                return True
        return False

    def dismiss(self) -> bool:
        q = qt()
        w = self.w
        if isinstance(w, q.widgets.QDialog):
            w.reject()
            return True
        if w.windowType() == q.core.Qt.WindowType.Popup:
            w.close()
            return True
        return False


def window_element(widget: Any) -> QtWindowElement:
    return QtWindowElement(widget)


def widget_children(w: Any) -> list[UiElement]:
    """子控件（不含独立窗口），按位置从上到下、从左到右（阅读顺序）。"""
    widget_type = qt().widgets.QWidget
    kids = [c for c in w.children() if isinstance(c, widget_type) and not c.isWindow()]
    kids.sort(key=lambda c: (c.y(), c.x()))
    return [QtWidgetElement(c) for c in kids]


# ---------------------------------------------------------------------------
# 规则表：控件类型 → 描述 / 子节点 / 动作
# ---------------------------------------------------------------------------


def _no(*_: Any) -> bool:
    return False


class _Rule:
    """一类控件的映射。字段缺省为"不支持"。"""

    def __init__(
        self,
        matches: Callable[[Any], bool],
        describe: Callable[[Any], UiDescription] = lambda _w: TRANSPARENT,
        *,
        ignored: bool = False,
        children: Callable[[Any], list[UiElement]] | None = None,
        read: Callable[[Any], list[str]] = lambda _w: [],
        read_children: bool = True,
        click: Callable[[Any], bool] = _no,
        set_text: Callable[[Any, str], bool] = _no,
        select_option: Callable[[Any, str], bool] = _no,
        set_range: Callable[[Any, float], bool] = _no,
    ) -> None:
        self.matches = matches
        self._describe = describe
        self.ignored = ignored
        self.children = children
        self.read = read
        self.read_children = read_children
        self.click = click
        self.set_text = set_text
        self.select_option = select_option
        self.set_range = set_range

    def describe(self, w: Any) -> UiDescription:
        d = self._describe(w)
        declared = declared_of(w)
        if declared is None or d.declared is not None:
            return d
        return UiDescription(**{**d.__dict__, "declared": declared})


def _is(type_name: str, *excluding: str) -> Callable[[Any], bool]:
    def check(w: Any) -> bool:
        widgets = qt().widgets
        return isinstance(w, getattr(widgets, type_name)) and not any(isinstance(w, getattr(widgets, x)) for x in excluding)

    return check


def _disabled(w: Any) -> tuple[str, ...]:
    return () if w.isEnabled() else ("disabled",)


def _focused(w: Any) -> tuple[str, ...]:
    return ("focused",) if w.window().focusWidget() is w else ()


def _button_name(w: Any) -> str:
    return strip_mnemonic(w.text()) or _accessible_name(w) or w.toolTip()


def _item(role: str, name: str, w: Any, *states: str, value: str | None = None, **extra: Any) -> UiDescription:
    return UiDescription(
        UiEntryKind.ITEM, role, name, value=value, states=(*states, *_disabled(w), *_focused(w)), **extra
    )


def _describe_check(w: Any) -> UiDescription:
    return _item("checkbox", _button_name(w), w, *_check_states(w.checkState()))


def _describe_radio(w: Any) -> UiDescription:
    return _item("radio", _button_name(w), w, "checked" if w.isChecked() else "unchecked")


def _button_menu(w: Any) -> Any | None:
    menu = getattr(w, "menu", None)
    return menu() if callable(menu) else None


def _describe_button(w: Any) -> UiDescription:
    states: list[str] = []
    if w.isCheckable() and w.isChecked():
        states.append("pressed")
    menu = _button_menu(w)
    if menu is not None:
        states.append("expanded" if menu.isVisible() else "collapsed")
    return _item("button", _button_name(w), w, *states)


def _click_button(w: Any) -> bool:
    if _button_menu(w) is not None and hasattr(w, "showMenu"):
        w.showMenu()
        return True
    w.click()
    return True


def _text_name(w: Any) -> str:
    placeholder = getattr(w, "placeholderText", None)
    return _accessible_name(w) or (placeholder() if callable(placeholder) else "") or w.toolTip()


def _secure_echo(w: Any) -> bool:
    mode = qt().widgets.QLineEdit.EchoMode
    return w.echoMode() in (mode.Password, mode.PasswordEchoOnEdit, mode.NoEcho)


def _describe_line_edit(w: Any) -> UiDescription:
    states = ("readonly",) if w.isReadOnly() else ()
    if w.validator() is not None and w.text() and not w.hasAcceptableInput():
        states = (*states, "invalid")
    if _secure_echo(w):
        return _item("textbox", _text_name(w), w, *states, secure=True, has_secure_value=bool(w.text()))
    return _item("textbox", _text_name(w), w, *states, value=w.text())


def _fill_line_edit(w: Any, text: str) -> bool:
    w.setFocus(qt().core.Qt.FocusReason.OtherFocusReason)
    # @why selectAll + insert 与用户输入等价：经过校验器与最大长度，发出 textEdited（setText 不发）。
    w.selectAll()
    w.insert(text)
    if w.text() != text:
        raise inp.invalid(f"输入框没有接受「{fmt.truncate(text, fmt.VALUE_MAX)}」（校验器或长度限制），当前值为「{w.text()}」")
    return True


def _describe_text_edit(w: Any) -> UiDescription:
    return _item("textbox", _text_name(w), w, *(("readonly",) if w.isReadOnly() else ()), value=w.toPlainText())


def _fill_text_edit(w: Any, text: str) -> bool:
    w.setFocus(qt().core.Qt.FocusReason.OtherFocusReason)
    w.setPlainText(text)
    return True


def _describe_combo(w: Any) -> UiDescription:
    expanded = "expanded" if w.view().isVisible() else "collapsed"
    return _item("combobox", _label_name(w), w, expanded, value=w.currentText())


def _click_combo(w: Any) -> bool:
    w.showPopup()
    return True


def _select_combo(w: Any, text: str) -> bool:
    flags = qt().core.Qt.MatchFlag
    index = w.findText(text, flags.MatchExactly | flags.MatchCaseSensitive)
    if index < 0:
        index = w.findText(text, flags.MatchFixedString)  # 不区分大小写
    if index < 0:
        if w.isEditable():
            w.setEditText(text)
            return True
        options = "、".join(w.itemText(i) for i in range(min(w.count(), _OPTIONS_LISTED)))
        raise inp.invalid(f"下拉框没有选项「{text}」；可选：{options}")
    w.setCurrentIndex(index)
    # @why 与用户在弹出列表中选择一致：应用常连接 activated / textActivated（setCurrentIndex 只发 currentIndexChanged）。
    w.activated.emit(index)
    w.textActivated.emit(w.itemText(index))
    return True


def _describe_spin(w: Any) -> UiDescription:
    return _item("spinbutton", _label_name(w), w, *(("readonly",) if w.isReadOnly() else ()), value=w.text())


def _set_spin(w: Any, value: float) -> bool:
    widgets = qt().widgets
    if not isinstance(w, (widgets.QSpinBox, widgets.QDoubleSpinBox)):
        return False
    if not w.minimum() <= value <= w.maximum():
        raise inp.invalid(f"数字框取值范围为 {w.minimum()}–{w.maximum()}")
    w.setValue(value if isinstance(w, widgets.QDoubleSpinBox) else int(round(value)))
    return True


def _describe_slider(w: Any) -> UiDescription:
    return _item("slider", _label_name(w), w, value=str(w.value()))


def _set_slider(w: Any, value: float) -> bool:
    if not w.minimum() <= value <= w.maximum():
        raise inp.invalid(f"滑块取值范围为 {w.minimum()}–{w.maximum()}")
    w.setValue(int(round(value)))
    return True


def _container(role: str, name: Callable[[Any], str]) -> Callable[[Any], UiDescription]:
    return lambda w: UiDescription(UiEntryKind.CONTAINER, role, name(w))


def _describe_status(w: Any) -> UiDescription:
    message = w.currentMessage()
    return UiDescription(UiEntryKind.ITEM, "status", message, descend=True) if message else TRANSPARENT


def _window_role(w: Any) -> str:
    q = qt()
    if isinstance(w, q.widgets.QMenu):
        return "list"
    if isinstance(w, q.widgets.QMessageBox):
        return "alertdialog"
    if isinstance(w, q.widgets.QDialog) or w.windowType() == q.core.Qt.WindowType.Popup:
        return "dialog"
    return "window"


def _window_name(w: Any) -> str:
    q = qt()
    if isinstance(w, q.widgets.QMenu):
        return strip_mnemonic(w.title())
    return w.windowTitle() or _accessible_name(w)


def _window_children(w: Any) -> list[UiElement]:
    if isinstance(w, qt().widgets.QMenu):
        return action_children(w)
    return widget_children(w)


def _read_label(w: Any) -> list[str]:
    return [_label_text(w)]


_RULES: tuple[_Rule, ...] = (
    _Rule(_is("QScrollBar"), ignored=True),
    _Rule(_is("QHeaderView"), ignored=True),
    _Rule(_is("QSizeGrip"), ignored=True),
    _Rule(_is("QRubberBand"), ignored=True),
    _Rule(_is("QFocusFrame"), ignored=True),
    _Rule(lambda w: w.isWindow(), lambda w: UiDescription(UiEntryKind.CONTAINER, _window_role(w), _window_name(w)),
          children=_window_children, read=lambda w: [_window_name(w)]),
    _Rule(_is("QCheckBox"), _describe_check, read=lambda w: [_button_name(w)], click=lambda w: w.click() or True),
    _Rule(_is("QRadioButton"), _describe_radio, read=lambda w: [_button_name(w)], click=lambda w: w.click() or True),
    _Rule(_is("QAbstractButton"), _describe_button, read=lambda w: [_button_name(w)], click=_click_button),
    _Rule(_is("QLineEdit"), _describe_line_edit, read_children=False, set_text=_fill_line_edit,
          read=lambda w: ([fmt.SECURE_MASK] if w.text() else []) if _secure_echo(w) else [w.text()],
          click=lambda w: w.setFocus(qt().core.Qt.FocusReason.OtherFocusReason) or True),
    _Rule(_is("QTextEdit"), _describe_text_edit, read_children=False, set_text=_fill_text_edit,
          read=lambda w: [w.toPlainText()]),
    _Rule(_is("QPlainTextEdit"), _describe_text_edit, read_children=False, set_text=_fill_text_edit,
          read=lambda w: [w.toPlainText()]),
    _Rule(_is("QComboBox"), _describe_combo, read_children=False, click=_click_combo, select_option=_select_combo,
          read=lambda w: [w.currentText()]),
    _Rule(_is("QAbstractSpinBox"), _describe_spin, read_children=False, set_range=_set_spin, read=lambda w: [w.text()]),
    _Rule(_is("QAbstractSlider"), _describe_slider, set_range=_set_slider),
    _Rule(_is("QTabBar"), children=lambda w: [TabElement(w, i) for i in range(w.count())]),
    _Rule(_is("QAbstractItemView"), _container("list", _accessible_name), children=item_children),
    _Rule(_is("QMenuBar"), _container("navigation", lambda w: _accessible_name(w) or "菜单栏"), children=action_children),
    _Rule(_is("QGroupBox"), _container("group", lambda w: strip_mnemonic(w.title())), read=lambda w: [w.title()]),
    _Rule(_is("QToolBar"), _container("group", lambda w: w.windowTitle() or _accessible_name(w))),
    _Rule(_is("QStatusBar"), _describe_status, read=lambda w: [w.currentMessage()]),
    _Rule(_is("QAbstractScrollArea"), _container("scrollable", _accessible_name)),
    _Rule(_is("QLabel"), read=_read_label),
)  # fmt: skip

_GENERIC = _Rule(lambda _w: True)


def _rule_of(w: Any) -> _Rule:
    return next((r for r in _RULES if r.matches(w)), _GENERIC)
