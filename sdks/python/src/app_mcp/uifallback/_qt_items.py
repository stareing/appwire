"""Qt 虚拟子项：标签页、列表 / 树 / 表格项、菜单项（纯移动自 ``_qt_elements.py``）。"""

from __future__ import annotations

from collections.abc import Hashable, Iterator
from typing import Any

from ._format import UiEntryKind
from ._input import UiScrollDirection
from ._tree import UiDescription, UiElement
from ._qt_support import (
    ITEM_SCAN_MAX,
    _check_states,
    _scroll_page,
    close_popups,
    declared_of,
    qt,
    strip_mnemonic,
)


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
