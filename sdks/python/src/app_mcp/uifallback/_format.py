"""大纲与变化摘要格式：唯一定义见 spec/ui-fallback.md 第 4–7 节，本文件只是 Python 实现（与 UI 框架无关）。"""

from __future__ import annotations

import enum
import re
from dataclasses import dataclass
from typing import Any

__all__ = [
    "UiActionResult",
    "UiEntry",
    "UiEntryKind",
    "UiOutline",
    "UiReadResult",
    "ROLE_LABELS",
    "SECURE_LABEL",
    "SECURE_MASK",
    "NAME_MAX",
    "STATUS_NAME_MAX",
    "VALUE_MAX",
    "MAX_CHANGES",
    "LIMIT_MAX",
    "READ_DEFAULT",
    "READ_MAX",
    "REF_PATTERN",
    "label",
    "collapse",
    "truncate",
    "render",
    "diff",
    "read_text",
]


class UiEntryKind(enum.Enum):
    """条目类别：控件、分组（带引用，可用于 within）、标题（只作分组）。"""

    ITEM = "item"
    CONTAINER = "container"
    HEADING = "heading"


ROLE_LABELS: dict[str, str] = {
    "button": "按钮", "link": "链接", "textbox": "输入框", "searchbox": "搜索框",
    "checkbox": "复选框", "radio": "单选框", "switch": "开关", "combobox": "下拉框",
    "listbox": "列表框", "option": "选项", "slider": "滑块", "spinbutton": "数字框",
    "tab": "标签页", "menuitem": "菜单项", "treeitem": "树节点", "gridcell": "单元格",
    "status": "提示", "alert": "警告", "generic": "元素",
    "window": "窗口", "dialog": "对话框", "alertdialog": "警告框", "navigation": "导航",
    "main": "主区域", "form": "表单", "search": "搜索区", "group": "分组", "list": "列表",
    "scrollable": "滚动区", "heading": "标题",
}  # fmt: skip

SECURE_LABEL = "密码框"
SECURE_MASK = "••••"
"""密码类控件的固定掩码（spec/ui-fallback.md 8.1：不泄露长度）。"""

NAME_MAX = 40
STATUS_NAME_MAX = 80
VALUE_MAX = 30
MAX_CHANGES = 15
LIMIT_MAX = 500
READ_DEFAULT = 1000
READ_MAX = 20000

REF_PATTERN = re.compile(r"^e[1-9]\d*$")
"""引用格式（spec/ui-fallback.md 第 3 节）。"""

_WHITESPACE = re.compile(r"\s+")
_IGNORED_STATES = frozenset({"focused"})
_EXCLUSIVE_STATES = (frozenset({"checked", "unchecked", "mixed"}), frozenset({"expanded", "collapsed"}))


@dataclass(eq=False)
class UiEntry:
    """大纲中的一个条目。

    @invariant ``key`` 在前后两次快照中对应同一控件（控件与分组为引用）；``chain`` 为祖先分组（分组与标题）在条目列表中的
        下标、外层在前；``containers`` 为祖先分组的键、外层在前；``depth`` 为所在分组层数（缩进）。
    """

    kind: UiEntryKind
    key: str
    role: str
    label: str
    ref: str | None = None
    name: str = ""
    value: str | None = None
    states: tuple[str, ...] = ()
    required: bool = False
    declared: str | None = None
    level: int | None = None
    depth: int = 0
    chain: tuple[int, ...] = ()
    containers: tuple[str, ...] = ()

    @property
    def described(self) -> str:
        """``按钮「结算」``。"""
        return f"{self.label}「{self.name}」" if self.name else self.label


@dataclass(frozen=True)
class UiOutline:
    """``ui.outline`` 的结果（spec/ui-fallback.md 4.1 / 4.3）。"""

    text: str
    items: tuple[UiEntry, ...]
    groups: tuple[str | None, ...]
    total: int
    remaining: int
    hint: str | None

    def to_json(self) -> dict[str, Any]:
        items: list[dict[str, Any]] = []
        for e, group in zip(self.items, self.groups):
            item: dict[str, Any] = {"ref": e.ref, "role": e.role, "name": e.name}
            if e.value is not None:
                item["value"] = e.value
            if e.states:
                item["states"] = list(e.states)
            if e.required:
                item["required"] = True
            if e.declared is not None:
                item["declared"] = e.declared
            if group is not None:
                item["group"] = group
            items.append(item)
        out: dict[str, Any] = {"text": self.text, "items": items, "total": self.total}
        if self.remaining > 0:
            out["remaining"] = self.remaining
        if self.hint is not None:
            out["hint"] = self.hint
        return out


@dataclass(frozen=True)
class UiActionResult:
    """操作类工具的结果（spec/ui-fallback.md 7.2）。"""

    changes: tuple[str, ...]
    hint: str | None = None

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {"ok": True, "changes": list(self.changes)}
        if self.hint is not None:
            out["hint"] = self.hint
        return out


@dataclass(frozen=True)
class UiReadResult:
    """``ui.read`` 的结果。"""

    ref: str
    text: str
    truncated: bool

    def to_json(self) -> dict[str, Any]:
        return {"ref": self.ref, "text": self.text, "truncated": self.truncated}


def label(role: str) -> str:
    return ROLE_LABELS.get(role, "元素")


def collapse(s: object) -> str:
    return _WHITESPACE.sub(" ", str(s)).strip() if s is not None else ""


def truncate(s: str, max_chars: int) -> str:
    """截断到 ``max_chars`` 个码点（保留前 max − 1 个、去掉尾部空白再加 ``…``）。"""
    if len(s) <= max_chars:
        return s
    return s[: max(max_chars - 1, 0)].rstrip() + "…"


def _group_label(e: UiEntry) -> str:
    return e.name if e.kind is UiEntryKind.HEADING else e.described


def _haystack(e: UiEntry, entries: list[UiEntry]) -> str:
    parts = [e.label, e.role, e.name, e.value or "", e.declared or "", *(_group_label(entries[i]) for i in e.chain)]
    return " ".join(parts).lower()


def _item_line(e: UiEntry) -> str:
    line = f"{e.ref} {e.described}"
    if e.value is not None:
        line += f'= "{e.value}"'
    if e.states:
        line += " " + " ".join(e.states)
    if e.required:
        line += " (必填)"
    if e.declared is not None:
        line += f" [已声明：{e.declared}]"
    return line


def _container_line(e: UiEntry) -> str:
    return f"» {e.ref} {e.described}" + (f" [已声明：{e.declared}]" if e.declared is not None else "")


def _heading_line(e: UiEntry) -> str:
    return f"{'#' * (e.level or 2)} {e.name}"


def render(entries: list[UiEntry], query: str | None, limit: int) -> UiOutline:
    """过滤、截断、分组并渲染大纲（spec/ui-fallback.md 4.2 / 4.3）。"""
    tokens = [t for t in _WHITESPACE.split((query or "").lower()) if t]
    matched = [
        i
        for i, e in enumerate(entries)
        if e.kind is UiEntryKind.ITEM and all(t in _haystack(e, entries) for t in tokens)
    ]
    shown = set(matched[:limit])
    kept_groups = {g for i in shown for g in entries[i].chain}
    lines: list[str] = []
    items: list[UiEntry] = []
    groups: list[str | None] = []
    declared = False
    for i, e in enumerate(entries):
        indent = "  " * e.depth
        if e.kind is UiEntryKind.ITEM:
            if i in shown:
                lines.append(indent + _item_line(e))
                declared = declared or e.declared is not None
                items.append(e)
                groups.append(" › ".join(_group_label(entries[g]) for g in e.chain) or None)
        elif i not in kept_groups:
            continue
        elif e.kind is UiEntryKind.CONTAINER:
            lines.append(indent + _container_line(e))
            declared = declared or e.declared is not None
        else:
            lines.append(indent + _heading_line(e))
    remaining = len(matched) - len(shown)
    if not lines:
        lines.append(f"（没有与「{query}」匹配的可交互元素）" if tokens else "（没有可见的可交互元素）")
    if remaining > 0:
        lines.append(f"…另有 {remaining} 个元素未列出，可用 query 或 within 缩小范围")
    hint = "标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具" if declared else None
    return UiOutline("\n".join(lines), tuple(items), tuple(groups), len(matched), remaining, hint)


def _state_change(before: tuple[str, ...], after: tuple[str, ...]) -> list[str]:
    b = [s for s in before if s not in _IGNORED_STATES]
    a = [s for s in after if s not in _IGNORED_STATES]
    added = [s for s in a if s not in b]
    removed = [
        s
        for s in b
        if s not in a and not any(s in g and any(x in g for x in added) for g in _EXCLUSIVE_STATES)
    ]
    parts = []
    if added:
        parts.append("变为 " + "、".join(added))
    if removed:
        parts.append("不再 " + "、".join(removed))
    return parts


def _visible_states(e: UiEntry) -> str:
    states = [s for s in e.states if s not in _IGNORED_STATES]
    return " " + " ".join(states) if states else ""


def _change_label(e: UiEntry) -> str:
    return f"标题「{e.name}」" if e.kind is UiEntryKind.HEADING else f"{e.described}({e.ref})"


def _outer_changed(e: UiEntry, present: dict[str, UiEntry], other: dict[str, UiEntry]) -> str | None:
    """外层最先出现的"同样是新增 / 同样被移除"的祖先分组。"""
    return next((c for c in e.containers if c in present and c not in other), None)


def _child_counts(entries: list[UiEntry], present: dict[str, UiEntry], other: dict[str, UiEntry]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for e in entries:
        if e.key in other:
            continue
        outer = _outer_changed(e, present, other)
        if outer is None:
            continue
        counts[outer] = counts.get(outer, 0) + (1 if e.kind is UiEntryKind.ITEM else 0)
    return counts


def diff(before: list[UiEntry], after: list[UiEntry], prefix: str) -> list[str]:
    """操作前后的变化摘要（spec/ui-fallback.md 7.2）。"""
    by_before = {e.key: e for e in before}
    by_after = {e.key: e for e in after}
    added = _child_counts(after, by_after, by_before)
    removed = _child_counts(before, by_before, by_after)
    output: list[str] = []
    for e in after:
        old = by_before.get(e.key)
        if old is None:
            if _outer_changed(e, by_after, by_before) is not None:
                continue
            line = f"新增{_change_label(e)}"
            if e.kind is UiEntryKind.ITEM:
                if e.value is not None:
                    line += f' = "{e.value}"'
                line += _visible_states(e)
            n = added.get(e.key, 0)
            if n > 0:
                line += f'，含 {n} 个可交互元素（可用 {prefix}.outline({{ within: "{e.ref}" }}) 查看）'
            output.append(line)
            continue
        if e.kind is UiEntryKind.HEADING:
            if old.name != e.name:
                output.append(f"标题「{old.name}」变为「{e.name}」")
            continue
        parts: list[str] = []
        if old.name != e.name:
            parts.append(f"名称变为「{e.name}」")
        if old.value != e.value:
            parts.append(f'值变为 "{e.value}"' if e.value is not None else "值已清空")
        parts.extend(_state_change(old.states, e.states))
        if parts:
            output.append(f"{e.ref} {old.name or old.label} {'，'.join(parts)}")
    for e in before:
        if e.key in by_after or _outer_changed(e, by_before, by_after) is not None:
            continue
        n = removed.get(e.key, 0)
        output.append(f"{_change_label(e)} 已消失" + (f"（含 {n} 个可交互元素）" if n > 0 else ""))
    if len(output) <= MAX_CHANGES:
        return output
    rest = len(output) - MAX_CHANGES
    return [*output[:MAX_CHANGES], f"…另有 {rest} 项变化，请调用 {prefix}.outline 查看"]


def read_text(parts: list[str], reference: str | None, max_chars: int) -> UiReadResult:
    """``ui.read`` 的文本：折叠空白、相邻重复只保留一次（按钮名称与其内部文本相同）、截断到 ``max_chars`` 个码点。"""
    texts = [t for t in (collapse(p) for p in parts) if t]
    full = " ".join(t for i, t in enumerate(texts) if i == 0 or t != texts[i - 1])
    truncated = len(full) > max_chars
    return UiReadResult(reference or "root", truncate(full, max_chars) if truncated else full, truncated)

