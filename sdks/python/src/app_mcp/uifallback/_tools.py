"""兜底工具的注册与启用状态（spec/ui-fallback.md 第 2 节），与 UI 框架无关。"""

from __future__ import annotations

import threading
from collections.abc import Awaitable, Callable
from typing import Any

from .._client import ToolCallError
from . import _format as fmt
from . import _input as inp
from ._inspector import UiInspector

__all__ = ["UiFallbackTools"]

_Run = Callable[[dict[str, Any]], Awaitable[dict[str, Any]]]


class UiFallbackTools:
    """在 ``registrar``（客户端或作用域）下建子作用域 ``<prefix>-fallback`` 并注册 ``<prefix>.outline`` 等六个工具。

    工具以 ``surface="view"`` 注册、初始禁用；平台适配层按"App 有可见窗口"调用 :meth:`set_visible`。

    @invariant ``set_visible`` / ``close`` 可在任意线程调用（内部加锁）；``close`` 之后忽略 ``set_visible``。
    """

    def __init__(self, registrar: Any, inspector: UiInspector) -> None:
        self.inspector = inspector
        self._lock = threading.Lock()
        self._visible = False
        self._closed = False
        self._scope = registrar.scope(f"{inspector.prefix}-fallback")
        self._handles: list[Any] = []
        try:
            self._register(inspector.prefix)
        except BaseException:
            self._scope.dispose()
            raise

    @property
    def enabled(self) -> bool:
        """兜底工具当前是否启用（App 有可见窗口且未关闭）。"""
        return not self._closed and self._visible

    def set_visible(self, visible: bool) -> bool:
        """更新"有可见窗口"状态；返回状态是否变化（变为不可见时调用方应清空引用）。"""
        with self._lock:
            if self._closed or visible == self._visible:
                return False
            self._visible = visible
            for h in self._handles:
                h.set_enabled(visible)
            return True

    def close(self) -> None:
        """注销全部兜底工具（幂等）。"""
        with self._lock:
            if self._closed:
                return
            self._closed = True
            self._handles.clear()
        self._scope.dispose()

    def _guarded(self, run: _Run) -> Callable[..., Awaitable[dict[str, Any]]]:
        async def handler(**args: Any) -> dict[str, Any]:
            if not self.enabled:
                raise ToolCallError("TOOL_DISABLED", "应用当前没有可见窗口，兜底工具不可用")
            return await run(args)

        return handler

    def _add(self, prefix: str, name: str, title: str, description: str, schema: dict[str, Any], read_only: bool, run: _Run) -> None:
        self._handles.append(
            self._scope.add_tool(
                self._guarded(run),
                f"{prefix}.{name}",
                description,
                input_schema=schema,
                risk="read" if read_only else "write",
                title=title,
                enabled=False,
                annotations={"title": title, "read_only_hint": read_only},
                surface="view",
            )
        )

    def _register(self, prefix: str) -> None:
        ui = self.inspector
        schemas = inp.schemas(ui.max_items)

        async def outline(a: dict[str, Any]) -> dict[str, Any]:
            result = await ui.outline(inp.arg_string(a, "query"), inp.arg_ref(a, "within", False), inp.arg_int(a, "limit"))
            return result.to_json()

        async def click(a: dict[str, Any]) -> dict[str, Any]:
            return (await ui.click(_required_ref(a))).to_json()

        async def fill(a: dict[str, Any]) -> dict[str, Any]:
            return (await ui.fill(_required_ref(a), inp.arg_value(a))).to_json()

        async def press(a: dict[str, Any]) -> dict[str, Any]:
            key = inp.arg_string(a, "key")
            if key is None:
                raise inp.invalid("缺少参数 key")
            return (await ui.press(inp.arg_ref(a, "ref", False), key)).to_json()

        async def scroll(a: dict[str, Any]) -> dict[str, Any]:
            return (await ui.scroll(_required_ref(a), inp.arg_string(a, "direction"))).to_json()

        async def read(a: dict[str, Any]) -> dict[str, Any]:
            return (await ui.read(inp.arg_ref(a, "ref", False), inp.arg_int(a, "maxChars"))).to_json()

        self._add(
            prefix, "outline", "界面控件大纲",
            "兜底能力：列出当前窗口可见的可交互控件（按钮、输入框、复选框等），每行一个，带引用 eN，按窗口 / 对话框 / 分组归类。"
            "应用已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / scroll / read。",
            schemas["outline"], True, outline,
        )  # fmt: skip
        self._add(
            prefix, "click", "点击控件",
            f"兜底能力：激活 {prefix}.outline 中的控件（点击按钮、切换复选框、选中选项、展开 / 收起），返回界面变化摘要。",
            schemas["click"], False, click,
        )  # fmt: skip
        self._add(
            prefix, "fill", "填写控件",
            "兜底能力：填写文本框；复选框 / 开关 / 单选框传 true / false；下拉框传选项文本；滑块 / 数字框传数字。密码类控件不支持。"
            "返回界面变化摘要。",
            schemas["fill"], False, fill,
        )  # fmt: skip
        self._add(
            prefix, "press", "按键",
            "兜底能力：按键（ref 缺省为当前焦点控件）：Enter（提交文本框 / 激活）、Escape（关闭对话框）、Tab / Shift+Tab（移动焦点）、"
            "Space（激活）。输入文本请用 fill。返回界面变化摘要。",
            schemas["press"], False, press,
        )  # fmt: skip
        self._add(
            prefix, "scroll", "滚动",
            "兜底能力：无 direction 时把控件滚动到可见；direction 为 up / down / left / right 时滚动该控件所在的滚动区一页"
            "（down = 向下翻看更多内容）。返回界面变化摘要。",
            schemas["scroll"], False, scroll,
        )  # fmt: skip
        self._add(
            prefix, "read", "读取控件文本",
            f"兜底能力：读取控件的可见文本（折叠空白，默认最多 {fmt.READ_DEFAULT} 字）；ref 缺省为全部窗口。",
            schemas["read"], True, read,
        )  # fmt: skip


def _required_ref(args: dict[str, Any]) -> str:
    reference = inp.arg_ref(args, "ref")
    assert reference is not None  # arg_ref(required=True) 缺失时已抛 INVALID_INPUT
    return reference
