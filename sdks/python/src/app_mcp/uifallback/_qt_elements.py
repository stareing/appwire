"""Qt Widgets 控件 → 兜底节点（spec/ui-fallback.md 8.4 Qt 列）。所有方法只在 Qt GUI 线程调用。

PySide6 / PyQt6 为可选依赖：首次调用 :func:`qt` 时才导入（按此顺序尝试）。
"""

from __future__ import annotations

import functools
import re
from collections.abc import Callable, Hashable, Iterator, Sequence
from types import SimpleNamespace
from typing import Any

from . import _format as fmt
from . import _input as inp
from ._format import UiEntryKind
from ._input import UiKey, UiScrollDirection
from ._tree import TRANSPARENT, UiDescription, UiElement, UiWindow

__all__ = ["DECLARED_PROPERTY", "qt", "is_alive", "QtWidgetElement", "QtWindowElement", "declared_of"]

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
# 虚拟子项：标签页、列表 / 树 / 表格项、菜单项
# ---------------------------------------------------------------------------


class TabElement(UiElement):
    """``QTabBar`` 的一个标签页（键为下标；改名或换位后按指纹作废）。"""

    def __init__(self, bar: Any, index: int) -> None:
        self.bar, self.index = bar, index

    @property
    def identity(self) -> Any:
        return self.bar

    @property
    def key(self) -> Hashable:
        return ("tab", self.index)

    @property
    def reusable(self) -> bool:
        return True

    def is_hidden(self) -> bool:
        b, i = self.bar, self.index
        return i >= b.count() or not b.isTabVisible(i) or not b.tabRect(i).intersects(b.rect())

    def describe(self) -> UiDescription:
        b, i = self.bar, self.index
        states = (("selected",) if b.currentIndex() == i else ()) + (() if b.isTabEnabled(i) else ("disabled",))
        return UiDescription(UiEntryKind.ITEM, "tab", strip_mnemonic(b.tabText(i)), states=states)

    def read_text(self) -> list[str]:
        return [strip_mnemonic(self.bar.tabText(self.index))]

    def is_enabled(self) -> bool:
        return bool(self.bar.isEnabled() and self.bar.isTabEnabled(self.index))

    def click(self) -> bool:
        self.bar.setCurrentIndex(self.index)
        return True


def _index_path(index: Any) -> tuple[tuple[int, int], ...]:
    path: list[tuple[int, int]] = []
    while index.isValid():
        path.append((index.row(), index.column()))
        index = index.parent()
    return tuple(reversed(path))


class ItemElement(UiElement):
    """列表 / 树 / 表格中的一项（键为行列路径；模型变化后按指纹核对）。"""

    def __init__(self, view: Any, index: Any, role: str) -> None:
        self.view = view
        self.index = qt().core.QPersistentModelIndex(index)
        self.role = role
        self._key = _index_path(index)

    @property
    def identity(self) -> Any:
        return self.view

    @property
    def key(self) -> Hashable:
        return self._key

    @property
    def reusable(self) -> bool:
        return True

    def _idx(self) -> Any:
        return self.view.model().index(self.index.row(), self.index.column(), self.index.parent())

    def _flags(self) -> Any:
        return self.view.model().flags(self._idx())

    def _checkable(self) -> bool:
        # @why QListWidgetItem 缺省带 ItemIsUserCheckable：只有设置了勾选状态的项才是复选项。
        q = qt().core.Qt
        return bool(self._flags() & q.ItemFlag.ItemIsUserCheckable) and self._idx().data(q.ItemDataRole.CheckStateRole) is not None

    def _text(self) -> str:
        value = self._idx().data(qt().core.Qt.ItemDataRole.DisplayRole)
        return "" if value is None else str(value)

    def _has_children(self) -> bool:
        return self.role == "treeitem" and self.view.model().hasChildren(self._idx())

    def is_hidden(self) -> bool:
        if not self.index.isValid():
            return True
        rect = self.view.visualRect(self._idx())
        return rect.isEmpty() or not rect.intersects(self.view.viewport().rect())

    def describe(self) -> UiDescription:
        q = qt()
        idx = self._idx()
        states: list[str] = []
        role = self.role
        if self._checkable():
            role = "checkbox"
            states.extend(_check_states(q.core.Qt.CheckState(idx.data(q.core.Qt.ItemDataRole.CheckStateRole) or 0)))
        selection = self.view.selectionModel()
        if selection is not None and selection.isSelected(idx):
            states.append("selected")
        if self._has_children():
            states.append("expanded" if self.view.isExpanded(idx) else "collapsed")
        if not self._flags() & q.core.Qt.ItemFlag.ItemIsEnabled:
            states.append("disabled")
        return UiDescription(UiEntryKind.ITEM, role, self._text(), states=tuple(states))

    def read_text(self) -> list[str]:
        return [self._text()]

    def is_enabled(self) -> bool:
        return bool(self.view.isEnabled() and self._flags() & qt().core.Qt.ItemFlag.ItemIsEnabled)

    def click(self) -> bool:
        q = qt()
        idx = self._idx()
        if self._checkable():
            role = q.core.Qt.ItemDataRole.CheckStateRole
            current = q.core.Qt.CheckState(idx.data(role) or 0)
            target = q.core.Qt.CheckState.Unchecked if current == q.core.Qt.CheckState.Checked else q.core.Qt.CheckState.Checked
            return bool(self.view.model().setData(idx, target, role))
        self.view.setCurrentIndex(idx)
        if self._has_children():
            self.view.setExpanded(idx, not self.view.isExpanded(idx))
        # @why 与鼠标点击一致：应用常连接 clicked / activated 处理"选中某项"。
        self.view.clicked.emit(idx)
        return True

    def scroll_into_view(self) -> bool:
        self.view.scrollTo(self._idx())
        return True

    def scroll_page(self, direction: UiScrollDirection) -> bool:
        return _scroll_page(self.view, direction)


def _item_role(view: Any) -> str:
    w = qt().widgets
    if isinstance(view, w.QTreeView):
        return "treeitem"
    if isinstance(view, w.QTableView):
        return "gridcell"
    return "option"


def _visible_indexes(view: Any) -> Iterator[Any]:
    """视口内的项（树按展开顺序、列表 / 表格按行），最多 :data:`ITEM_SCAN_MAX` 个。"""
    q = qt()
    model = view.model()
    if model is None:
        return
    root = view.rootIndex()
    height = view.viewport().height()
    first = view.indexAt(q.core.QPoint(1, 1))
    if isinstance(view, q.widgets.QTreeView):
        idx = first if first.isValid() else model.index(0, 0, root)
        idx = idx.sibling(idx.row(), 0) if idx.isValid() else idx
        for _ in range(ITEM_SCAN_MAX):
            if not idx.isValid() or view.visualRect(idx).top() > height:
                return
            yield idx
            idx = view.indexBelow(idx)
        return
    # @why 列数取表头而不是 model.columnCount()：列表模型（QAbstractListModel）在绑定中把 columnCount 设为私有。
    if isinstance(view, q.widgets.QTableView):
        columns = [c for c in range(view.horizontalHeader().count()) if not view.isColumnHidden(c)]
    else:
        columns = [view.modelColumn() if isinstance(view, q.widgets.QListView) else 0]
    count = 0
    for row in range(first.row() if first.isValid() else 0, model.rowCount(root)):
        row_visible = False
        for c in columns:
            idx = model.index(row, c, root)
            rect = view.visualRect(idx)
            if rect.isEmpty() or not rect.intersects(view.viewport().rect()):
                continue
            row_visible = True
            yield idx
            count += 1
            if count >= ITEM_SCAN_MAX:
                return
        if not row_visible and view.visualRect(model.index(row, columns[0] if columns else 0, root)).top() > height:
            return


def item_at(view: Any, path: tuple[Any, ...]) -> ItemElement | None:
    """按行列路径重建项（可能在视口外）；路径已不存在时 ``None``。"""
    model = view.model()
    if model is None or not path:
        return None
    index = qt().core.QModelIndex()  # 路径从模型根算起（_index_path）
    for step in path:
        if not (isinstance(step, tuple) and len(step) == 2):
            return None
        index = model.index(step[0], step[1], index)
        if not index.isValid():
            return None
    return ItemElement(view, index, _item_role(view))


def item_children(view: Any) -> list[UiElement]:
    role = _item_role(view)
    return [ItemElement(view, idx, role) for idx in _visible_indexes(view)]


class ActionElement(UiElement):
    """菜单栏 / 菜单中的一个 ``QAction``。"""

    def __init__(self, owner: Any, action: Any) -> None:
        self.owner, self.action = owner, action

    @property
    def identity(self) -> Any:
        return self.action

    def is_hidden(self) -> bool:
        a = self.action
        return not a.isVisible() or a.isSeparator() or self.owner.actionGeometry(a).isEmpty()

    def describe(self) -> UiDescription:
        a = self.action
        states: list[str] = []
        if a.isCheckable():
            states.append("checked" if a.isChecked() else "unchecked")
        submenu = a.menu()
        if submenu is not None:
            states.append("expanded" if submenu.isVisible() else "collapsed")
        if not a.isEnabled():
            states.append("disabled")
        return UiDescription(
            UiEntryKind.ITEM, "menuitem", strip_mnemonic(a.text()), states=tuple(states), declared=declared_of(a)
        )

    def read_text(self) -> list[str]:
        return [strip_mnemonic(self.action.text())]

    def is_enabled(self) -> bool:
        return bool(self.action.isEnabled())

    def click(self) -> bool:
        a = self.action
        if a.menu() is not None:
            self.owner.setActiveAction(a)  # 打开子菜单（菜单栏 / 级联菜单）
            return True
        close_popups()
        a.trigger()
        return True


def action_children(owner: Any) -> list[UiElement]:
    return [ActionElement(owner, a) for a in owner.actions()]


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
