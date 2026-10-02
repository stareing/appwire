"""控件节点抽象、引用登记与大纲收集（spec/ui-fallback.md 第 3–6 节），与 UI 框架无关。"""

from __future__ import annotations

import weakref
from collections.abc import Callable, Hashable, Sequence
from dataclasses import dataclass, field
from typing import Any

from . import _format as fmt
from ._format import UiEntry, UiEntryKind
from ._input import UiKey, UiScrollDirection

__all__ = [
    "UiDescription",
    "UiElement",
    "UiWindow",
    "UiNode",
    "UiSnapshot",
    "UiRefRegistry",
    "collect",
    "find_attached",
]


@dataclass(frozen=True)
class UiDescription:
    """控件节点的描述（spec/ui-fallback.md 第 4–6 节）。

    @invariant ``kind`` 为 ``None`` = 不列出、只遍历子节点（布局容器等），此时只有 ``declared`` 有意义；
        ``value`` 为原始值（未截断），``secure`` 时不读取、只看 ``has_secure_value``；
        ``descend``：控件（``ITEM``）是否仍遍历子节点。
    """

    kind: UiEntryKind | None
    role: str = "generic"
    name: str = ""
    value: str | None = None
    states: tuple[str, ...] = ()
    required: bool = False
    secure: bool = False
    has_secure_value: bool = False
    declared: str | None = None
    level: int | None = None
    descend: bool = False


TRANSPARENT = UiDescription(None)
"""不列出、只遍历子节点。"""


class UiElement:
    """兜底遍历的控件节点（平台适配：Qt 控件、虚拟子项）。所有方法只在 UI 线程调用。

    动作方法返回 ``False`` 表示该控件不支持此动作；需要更具体的说明时抛 ``ToolCallError``（用 ``_input`` 构造）。
    """

    @property
    def identity(self) -> Any:
        """弱引用的对象：同一控件多次遍历得到的对象相同（Qt 控件本身；虚拟子项为其所属控件）。"""
        raise NotImplementedError

    @property
    def key(self) -> Hashable:
        """``identity`` 内的子键（标签页下标、列表行路径）；控件本身为 ``None``。"""
        return None

    @property
    def generation(self) -> int:
        """同一 ``identity`` 被框架复用给另一个控件时变化；没有复用概念时为 0。"""
        return 0

    @property
    def reusable(self) -> bool:
        """框架会把同一位置复用给另一个数据项（列表行、标签页下标）：引用按指纹核对，指纹变了即作废。"""
        return False

    def is_hidden(self) -> bool:
        """本身（连同子树）不可见：隐藏、折叠、零尺寸、被裁剪或滚出视口。"""
        raise NotImplementedError

    def describe(self) -> UiDescription:
        raise NotImplementedError

    def children(self) -> Sequence[UiElement]:
        return ()

    def locate(self, identity: Any, key: Hashable) -> UiElement | None:
        """按引用记录重建本节点名下当前不在 :meth:`children` 中的虚拟子项（滚出视口的列表行）；没有时 ``None``。"""
        return None

    def read_text(self) -> list[str]:
        """``ui.read`` 中本节点自身的文本（名称、值；密码类只给掩码）。"""
        return []

    def read_children(self) -> bool:
        """``ui.read`` 是否继续读子节点（文本框内部不再读）。"""
        return True

    def is_enabled(self) -> bool:
        return True

    def click(self) -> bool:
        """激活：点击、切换、选中、展开 / 收起。"""
        return False

    def set_text(self, text: str) -> bool:
        return False

    def select_option(self, text: str) -> bool:
        """下拉框按选项文本选中（先精确后忽略大小写）；没有该选项时抛 INVALID_INPUT 并列出可选项。"""
        return False

    def set_range(self, value: float) -> bool:
        return False

    def focus(self) -> bool:
        return False

    def ime_action(self) -> bool:
        """文本框的"完成 / 提交"动作。"""
        return False

    def scroll_into_view(self) -> bool:
        return False

    def scroll_page(self, direction: UiScrollDirection) -> bool:
        """本控件或最近的可滚动祖先按查看方向滚动一页；已到尽头或不可滚动时 ``False``。"""
        return False


class UiWindow(UiElement):
    """顶层窗口（主窗口 / 对话框 / 弹出层）：作为 ``window`` / ``dialog`` 分组，并承接按键。"""

    def send_key(self, key: UiKey) -> bool:
        """把按键（按下 + 抬起）交给窗口的焦点控件；返回是否被处理。"""
        return False

    def move_focus(self, forward: bool) -> bool:
        return False

    def dismiss(self) -> bool:
        """Escape 未被处理时的关闭 / 取消（对话框取消、弹出层关闭；主窗口不处理）。"""
        return False


@dataclass(eq=False)
class UiNode:
    """一个可见节点及其条目。"""

    element: UiElement
    entry: UiEntry
    secure: bool
    window: UiWindow

    @property
    def described(self) -> str:
        return f"{self.entry.ref} {self.entry.described}"


@dataclass(frozen=True)
class UiSnapshot:
    """一次收集的结果：条目（按树序）与引用 → 节点。"""

    entries: list[UiEntry]
    by_ref: dict[str, UiNode]


def _weak(obj: Any) -> Callable[[], Any]:
    return weakref.ref(obj)


@dataclass(eq=False)
class _Record:
    owner: Callable[[], Any]
    key: Hashable
    generation: int
    fingerprint: str

    def points_to(self, identity: Any, key: Hashable, generation: int) -> bool:
        return self.owner() is identity and self.key == key and self.generation == generation


class UiRefRegistry:
    """引用 ``eN`` ↔ 控件（spec/ui-fallback.md 第 3 节）：弱引用 + 指纹（角色 + 名称 + 所在分组）。

    控件被框架重建（列表行复用）时，按指纹在当前界面中唯一匹配到的新控件沿用原引用；原位置还在但指纹变了时旧引用作废。

    @invariant 只在 UI 线程访问。
    """

    def __init__(self) -> None:
        self._by_ref: dict[str, _Record] = {}
        self._next = 1

    def _find(self, identity: Any, key: Hashable, generation: int) -> str | None:
        return next((r for r, rec in self._by_ref.items() if rec.points_to(identity, key, generation)), None)

    def ref_of(
        self,
        element: UiElement,
        fingerprint: str,
        taken: set[str],
        fingerprint_count: dict[str, int],
        verify: bool,
        live: Callable[[Any], bool],
    ) -> str:
        """取得（必要时分配或按指纹沿用）控件的引用。

        @input taken 本轮已分配的引用；fingerprint_count 本轮各指纹出现次数；verify 为假（子树收集分组路径不完整，或控件
            不会被复用）时不核对指纹；live 判断旧对象是否仍在界面上。
        """
        identity, key, generation = element.identity, element.key, element.generation
        existing = self._find(identity, key, generation)
        if existing is not None:
            if not verify or self._by_ref[existing].fingerprint == fingerprint:
                return existing
            # @why 框架把同一位置复用给了另一个控件（列表行、标签页改名）：旧引用作废，不能让它指向新控件。
            del self._by_ref[existing]
        if fingerprint_count.get(fingerprint) == 1:
            orphans = [
                r
                for r, rec in self._by_ref.items()
                if r not in taken
                and rec.fingerprint == fingerprint
                and ((old := rec.owner()) is None or (old is identity and rec.key == key) or not live(old))
            ]
            if len(orphans) == 1:
                rec = self._by_ref[orphans[0]]
                rec.owner, rec.key, rec.generation = _weak(identity), key, generation
                return orphans[0]
        reference = f"e{self._next}"
        self._next += 1
        self._by_ref[reference] = _Record(_weak(identity), key, generation, fingerprint)
        return reference

    def target(self, reference: str) -> tuple[Any, Hashable, int] | None:
        """引用记录的控件（可能已不在界面上）：``(identity, key, generation)``。"""
        rec = self._by_ref.get(reference)
        if rec is None:
            return None
        owner = rec.owner()
        return None if owner is None else (owner, rec.key, rec.generation)

    def prune(self) -> None:
        """丢弃控件已被回收的记录。"""
        for r in [r for r, rec in self._by_ref.items() if rec.owner() is None]:
            del self._by_ref[r]

    def clear(self) -> None:
        self._by_ref.clear()

    def __len__(self) -> int:
        return len(self._by_ref)


@dataclass(eq=False)
class _Raw:
    element: UiElement
    window: UiWindow
    desc: UiDescription
    name: str
    path: str
    depth: int
    chain: tuple[int, ...]
    containers: tuple[int, ...]

    @property
    def kind(self) -> UiEntryKind:
        assert self.desc.kind is not None
        return self.desc.kind


@dataclass(eq=False)
class _Frame:
    container: int
    headings: list[tuple[int, int]] = field(default_factory=list)  # (level, index)


def _name_of(d: UiDescription) -> str:
    limit = fmt.STATUS_NAME_MAX if d.role in ("status", "alert") else fmt.NAME_MAX
    return fmt.truncate(fmt.collapse(d.name), limit)


def _value_of(d: UiDescription) -> str | None:
    if d.secure:
        return fmt.SECURE_MASK if d.has_secure_value else None
    v = fmt.collapse(d.value)
    return fmt.truncate(v, fmt.VALUE_MAX) if v else None


def collect(
    refs: UiRefRegistry,
    windows: Sequence[UiWindow],
    start: tuple[UiElement, UiWindow] | None = None,
    live: Callable[[Any], bool] = lambda _: True,
) -> UiSnapshot:
    """收集 ``windows``（或 ``start`` 子树）下可见的条目，并为控件与分组分配引用。只能在 UI 线程上调用。

    ``start`` 给出时不按指纹沿用 / 核对引用（分组路径不完整）。
    """
    full = start is None
    raw: list[_Raw] = []
    declared_groups: list[tuple[str, int, int]] = []
    frames = [_Frame(-1)]
    container_stack: list[int] = []

    def chain() -> tuple[int, ...]:
        out: list[int] = []
        for f in frames:
            if f.container >= 0:
                out.append(f.container)
            out.extend(i for _, i in f.headings)
        return tuple(out)

    def path(c: tuple[int, ...]) -> str:
        return " › ".join(
            raw[i].name if raw[i].kind is UiEntryKind.HEADING else f"{fmt.label(raw[i].desc.role)}「{raw[i].name}」"
            for i in c
        )

    def walk(e: UiElement, window: UiWindow) -> None:
        if e.is_hidden():
            return
        d = e.describe()
        kind = d.kind
        pushed = False
        declared_from = len(raw)
        if kind is UiEntryKind.HEADING:
            name = _name_of(d)
            if name:
                frame = frames[-1]
                level = min(max(d.level or 2, 1), 3)
                while frame.headings and frame.headings[-1][0] >= level:
                    frame.headings.pop()
                c = chain()
                heading = UiDescription(UiEntryKind.HEADING, "heading", name, level=level)
                raw.append(_Raw(e, window, heading, name, path(c), len(frames) - 1, c, tuple(container_stack)))
                frame.headings.append((level, len(raw) - 1))
        elif kind is not None:
            c = chain()
            raw.append(_Raw(e, window, d, _name_of(d), path(c), len(frames) - 1, c, tuple(container_stack)))
            if kind is UiEntryKind.CONTAINER:
                frames.append(_Frame(len(raw) - 1))
                container_stack.append(len(raw) - 1)
                pushed = True
        if kind is not UiEntryKind.ITEM or d.descend:
            for child in e.children():
                walk(child, window)
        if pushed:
            frames.pop()
            container_stack.pop()
        if kind is None and d.declared is not None:
            declared_groups.append((d.declared, declared_from, len(raw)))

    if start is not None:
        walk(start[0], start[1])
    else:
        for w in windows:
            walk(w, w)

    # 第二遍：指纹计数 → 分配引用 → 条目。
    def fingerprint_of(r: _Raw) -> str:
        return f"{r.desc.role}|{r.name}|{r.path}"

    counts: dict[str, int] = {}
    for r in raw:
        if r.kind is not UiEntryKind.HEADING:
            fp = fingerprint_of(r)
            counts[fp] = counts.get(fp, 0) + 1
    taken: set[str] = set()
    refs_by_index: list[str | None] = [None] * len(raw)
    for i, r in enumerate(raw):
        if r.kind is UiEntryKind.HEADING:
            continue
        reference = refs.ref_of(
            r.element, fingerprint_of(r), taken, counts if full else {}, full and r.element.reusable, live
        )
        taken.add(reference)
        refs_by_index[i] = reference
    heading_keys: dict[int, int] = {}

    def key_of(i: int) -> str:
        reference = refs_by_index[i]
        if reference is not None:
            return reference
        return f"h{heading_keys.setdefault(id(raw[i].element), len(heading_keys))}"

    entries: list[UiEntry] = []
    by_ref: dict[str, UiNode] = {}
    for i, r in enumerate(raw):
        d = r.desc
        is_item = r.kind is UiEntryKind.ITEM
        entry = UiEntry(
            kind=r.kind,
            key=key_of(i),
            role=d.role,
            label=fmt.SECURE_LABEL if d.secure else fmt.label(d.role),
            ref=refs_by_index[i],
            name=r.name,
            value=_value_of(d) if is_item else None,
            states=d.states if is_item else (),
            required=is_item and d.required,
            declared=None if r.kind is UiEntryKind.HEADING else d.declared,
            level=d.level if r.kind is UiEntryKind.HEADING else None,
            depth=r.depth,
            chain=r.chain,
            containers=tuple(key_of(c) for c in r.containers),
        )
        entries.append(entry)
        if refs_by_index[i] is not None:
            by_ref[refs_by_index[i]] = UiNode(r.element, entry, d.secure, r.window)  # type: ignore[index]
    # 非控件节点上的声明（如按钮外层的容器）：子树中恰好一个控件时标给它。
    for tool, lo, hi in declared_groups:
        items = [e for e in entries[lo:hi] if e.kind is UiEntryKind.ITEM]
        if len(items) == 1 and items[0].declared is None:
            items[0].declared = tool
    refs.prune()
    return UiSnapshot(entries, by_ref)


def find_attached(
    refs: UiRefRegistry, reference: str, windows: Sequence[UiWindow]
) -> tuple[UiElement, UiWindow] | None:
    """按引用在全部窗口中（含不可见的节点）找到仍在界面上的控件。"""
    target = refs.target(reference)
    if target is None:
        return None
    identity, key, generation = target

    def find(e: UiElement, w: UiWindow) -> tuple[UiElement, UiWindow] | None:
        if e.identity is identity and e.key == key and e.generation == generation:
            return e, w
        located = e.locate(identity, key)
        if located is not None and located.generation == generation:
            return located, w
        for child in e.children():
            found = find(child, w)
            if found is not None:
                return found
        return None

    for w in windows:
        found = find(w, w)
        if found is not None:
            return found
    return None
