"""兜底工具的执行引擎（spec/ui-fallback.md 第 2–7 节）：大纲、点击、填写、按键、滚动、读取。

与客户端、具体 UI 框架无关。引擎本身是协程（在 SDK 的 asyncio 循环上运行），每一步对控件树的访问都经
:class:`UiPlatform` 切到 UI 线程执行；动作与"等界面稳定"之间把 UI 线程交还事件循环。
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from typing import Any, Protocol, TypeVar

from .._client import ToolCallError
from . import _format as fmt
from . import _input as inp
from ._format import UiActionResult, UiOutline, UiReadResult
from ._input import UiFillValue, UiKey
from ._tree import UiElement, UiNode, UiRefRegistry, UiSnapshot, UiWindow, collect, find_attached

__all__ = ["UiPlatform", "UiInspector"]

T = TypeVar("T")

_TEXT_ROLES = frozenset({"textbox", "searchbox"})
_STABLE_ROUNDS = 10


class UiPlatform(Protocol):
    """平台：窗口、线程切换与"等界面稳定"（Qt 见 ``app_mcp.uifallback.qt``；测试用假实现）。"""

    def windows(self) -> Sequence[UiWindow]:
        """当前可见、未被模态层遮挡的窗口，按 Z 序（最上层在后）。只在 UI 线程调用。"""
        ...

    def all_windows(self) -> Sequence[UiWindow]:
        """全部仍打开的窗口（含被遮挡的），用于区分"已失效"与"不可见"。只在 UI 线程调用。"""
        ...

    def is_live(self, identity: Any) -> bool:
        """控件对象仍在界面上（可能不可见）。只在 UI 线程调用。"""
        ...

    async def run(self, fn: Callable[[], T]) -> T:
        """在 UI 线程上执行 ``fn`` 并返回其结果（异常原样抛出）。"""
        ...

    async def perform(self, fn: Callable[[], bool]) -> bool:
        """在 UI 线程上执行动作 ``fn``。动作进入嵌套事件循环（打开模态对话框）而迟迟不返回时视为已执行，返回 ``True``。"""
        ...

    async def settle(self) -> None:
        """执行动作后等界面稳定（一帧 + 短暂延迟）。"""
        ...


class UiInspector:
    """兜底工具的执行引擎。

    @invariant 引用登记表只在 UI 线程上（``platform.run`` / ``perform`` 内）访问。
    """

    def __init__(self, platform: UiPlatform, prefix: str = "ui", max_items: int = 60) -> None:
        self.platform = platform
        self.prefix = prefix
        self.max_items = max(1, max_items)
        self._refs = UiRefRegistry()

    def clear(self) -> None:
        """丢弃全部引用（之后旧引用一律失效）。只在 UI 线程调用。"""
        self._refs.clear()

    # -- UI 线程上的步骤 ------------------------------------------------------

    def _snapshot(self) -> UiSnapshot:
        return collect(self._refs, self.platform.windows(), live=self.platform.is_live)

    def _resolve(self, reference: str, snap: UiSnapshot) -> UiNode:
        """按引用在当前界面中取控件（spec/ui-fallback.md 7.1）。"""
        node = snap.by_ref.get(reference)
        if node is not None:
            return node
        if find_attached(self._refs, reference, self.platform.all_windows()) is not None:
            raise inp.hidden(f"引用 {reference} 对应的控件", reference, self.prefix)
        raise inp.stale(reference, self.prefix)

    def _actionable(self, reference: str, snap: UiSnapshot) -> UiNode:
        node = self._resolve(reference, snap)
        if not node.element.is_enabled():
            raise inp.disabled(node.described, reference)
        return node

    @staticmethod
    def _refuse_secure(node: UiNode) -> None:
        if node.secure:
            raise inp.secure(node.described, node.entry.ref)

    @staticmethod
    def _unsupported(node: UiNode, why: str | None = None) -> ToolCallError:
        return inp.unsupported(node.described, node.entry.ref, why)

    def _prepare(self, reference: str) -> tuple[UiSnapshot, UiNode]:
        snap = self._snapshot()
        return snap, self._actionable(reference, snap)

    # -- 协程 ------------------------------------------------------------------

    async def _act(
        self, before: UiSnapshot, target: UiNode | None, action: Callable[[], bool] | None, until_stable: bool = False
    ) -> tuple[UiActionResult, bool]:
        """执行动作、等界面稳定、返回变化摘要与动作结果。``until_stable``：滚动等带动画的动作再等到相邻两次快照不变。"""
        done = True if action is None else await self.platform.perform(action)
        await self.platform.settle()
        after = await self.platform.run(self._snapshot)
        for _ in range(_STABLE_ROUNDS if until_stable else 0):
            await self.platform.settle()
            following = await self.platform.run(self._snapshot)
            moving = bool(fmt.diff(after.entries, following.entries, self.prefix))
            after = following
            if not moving:
                break
        declared = target.entry.declared if target is not None else None
        hint = f"该元素已声明为工具 {declared}，下次可直接调用" if declared is not None else None
        return UiActionResult(tuple(fmt.diff(before.entries, after.entries, self.prefix)), hint), done

    async def _act_required(self, before: UiSnapshot, node: UiNode, action: Callable[[], bool], why: str | None = None) -> UiActionResult:
        """动作必须被控件支持：不支持时以 ``unsupported`` 失败（不返回变化）。"""
        if not await self.platform.perform(action):
            raise self._unsupported(node, why)
        return (await self._act(before, node, None))[0]

    async def outline(self, query: str | None, within: str | None, limit: int | None) -> UiOutline:
        """``ui.outline``（spec/ui-fallback.md 第 4 节）。"""
        n = min(max(limit if limit is not None else self.max_items, 1), fmt.LIMIT_MAX)

        def step() -> UiOutline:
            snap = self._snapshot()
            if within is None:
                return fmt.render(snap.entries, query, n)
            scope = self._resolve(within, snap)
            sub = collect(self._refs, (), (scope.element, scope.window), self.platform.is_live)
            return fmt.render(sub.entries, query, n)

        return await self.platform.run(step)

    async def click(self, reference: str) -> UiActionResult:
        """``ui.click``：激活控件。"""
        before, node = await self.platform.run(lambda: self._prepare(reference))
        return await self._act_required(before, node, node.element.click)

    async def fill(self, reference: str, value: UiFillValue) -> UiActionResult:
        """``ui.fill``：文本框写入文本；复选框 / 开关 / 单选框按布尔值切换；下拉框按选项文本；滑块 / 数字框按数字。"""

        def step() -> tuple[UiSnapshot, UiNode, Callable[[], bool] | None, str | None]:
            before, node = self._prepare(reference)
            self._refuse_secure(node)
            e, el, d = node.entry, node.element, node.described
            if e.role in _TEXT_ROLES:
                text = inp.as_text(value, d, reference)
                if "readonly" in e.states:
                    raise self._unsupported(node, "只读")
                return before, node, lambda: el.set_text(text), None
            if e.role in ("checkbox", "switch", "radio"):
                want = inp.as_bool(value, d, reference)
                if ("checked" in e.states) == want:
                    return before, node, None, None
                if e.role == "radio" and not want:
                    raise self._unsupported(node, "单选框不能直接取消选中，请选中同组的其他项")
                return before, node, el.click, None
            if e.role == "combobox":
                text = inp.as_text(value, d, reference)
                return before, node, lambda: el.select_option(text), "请先 click 展开再 click 选项"
            if e.role in ("slider", "spinbutton"):
                number = inp.as_number(value, d, reference)
                return before, node, lambda: el.set_range(number), None
            raise self._unsupported(node, "只能填写文本框、复选框、开关、单选框、下拉框、滑块与数字框")

        before, node, action, why = await self.platform.run(step)
        if action is None:
            return (await self._act(before, node, None))[0]
        return await self._act_required(before, node, action, why)

    async def press(self, reference: str | None, key: str) -> UiActionResult:
        """``ui.press``（spec/ui-fallback.md 2.1）：``reference`` 给出时先聚焦该控件。"""
        k = inp.parse_key(key)

        def step() -> tuple[UiSnapshot, UiNode | None, Callable[[], bool]]:
            before = self._snapshot()
            target = self._actionable(reference, before) if reference is not None else None
            if target is not None:
                self._refuse_secure(target)
            focused = next((n for n in reversed(list(before.by_ref.values())) if "focused" in n.entry.states), None)
            if target is None and focused is not None and focused.secure:
                raise inp.invalid("当前焦点在密码类控件上，兜底工具不对其按键", reason="secure")
            subject = target or focused
            windows = self.platform.windows()
            window = subject.window if subject is not None else (windows[-1] if windows else None)
            if window is None:
                raise ToolCallError("TOOL_DISABLED", "应用当前没有可见窗口")
            return before, target, lambda: self._press_on_ui(k, target, subject, window)

        before, target, action = await self.platform.run(step)
        result, handled = await self._act(before, target, action)
        if handled:
            return result
        return UiActionResult(result.changes, result.hint or f"按键 {key} 没有被任何控件处理")

    def _press_on_ui(self, k: UiKey, target: UiNode | None, subject: UiNode | None, window: UiWindow) -> bool:
        if target is not None:
            target.element.focus()
        if k is UiKey.TAB:
            return window.move_focus(True)
        if k is UiKey.SHIFT_TAB:
            return window.move_focus(False)
        if k is UiKey.ESCAPE:
            windows = self.platform.windows()
            top = windows[-1] if windows else None
            return top is not None and (top.send_key(k) or top.dismiss())
        is_text = subject is not None and subject.entry.role in _TEXT_ROLES
        if k is UiKey.ENTER and is_text and subject is not None and subject.element.ime_action():
            return True
        if window.send_key(k):
            return True
        return subject is not None and not is_text and subject.element.click()

    async def scroll(self, reference: str, direction: str | None) -> UiActionResult:
        """``ui.scroll``：无 ``direction`` 时把控件滚动到可见，否则滚动控件（或其所在滚动区）一页。只核对引用仍在界面上。"""
        d = inp.parse_direction(direction)

        def step() -> tuple[UiSnapshot, UiNode | None, UiElement]:
            before = self._snapshot()
            visible = before.by_ref.get(reference)
            if visible is not None:
                return before, visible, visible.element
            found = find_attached(self._refs, reference, self.platform.all_windows())
            if found is None:
                raise inp.stale(reference, self.prefix)
            return before, None, found[0]

        before, visible, element = await self.platform.run(step)
        if d is None:
            if visible is not None:
                return UiActionResult(())
            result, moved = await self._act(before, None, element.scroll_into_view, until_stable=True)
            return result if moved else UiActionResult(result.changes, "没有可滚动的祖先，无法滚动到可见")
        result, moved = await self._act(before, None, lambda: element.scroll_page(d), until_stable=True)
        return result if moved else UiActionResult(result.changes, "已到尽头或不可滚动")

    async def read(self, reference: str | None, max_chars: int | None) -> UiReadResult:
        """``ui.read``：控件（缺省为全部窗口）的可见文本，折叠空白；密码类控件只返回掩码。"""
        limit = min(max(max_chars if max_chars is not None else fmt.READ_DEFAULT, 1), fmt.READ_MAX)

        def step() -> UiReadResult:
            parts: list[str] = []

            def walk(e: UiElement) -> None:
                if e.is_hidden():
                    return
                parts.extend(e.read_text())
                if e.read_children():
                    for child in e.children():
                        walk(child)

            if reference is not None:
                walk(self._resolve(reference, self._snapshot()).element)
            else:
                for w in self.platform.windows():
                    walk(w)
            return fmt.read_text(parts, reference, limit)

        return await self.platform.run(step)
