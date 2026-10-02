"""Qt Widgets 进程内控件兜底（spec/ui-fallback.md，第 4c 项 H）。

开发者显式开启后注册 ``<prefix>.outline`` / ``click`` / ``fill`` / ``press`` / ``scroll`` / ``read``，经 QWidget 树
（名称取自 Qt 无障碍接口）与控件的公开接口直接操作控件，不截图、不按坐标。PySide6 / PyQt6 为可选依赖。

>>> from app_mcp.uifallback.qt import QtUiFallback, declare_mcp_tools
>>> fallback = QtUiFallback.enable(client)          # 必须在 Qt GUI 线程、创建 QApplication 之后调用
>>> declare_mcp_tools(clear_button, "cart.clear")   # 大纲中标出 [已声明：cart.clear]
>>> fallback.close()                                # 注销全部兜底工具

- 工具以 ``surface="view"`` 注册，只在有可见（未最小化）的顶层窗口时启用；
- 引擎在 SDK 的 asyncio 循环上运行，每一步对控件的访问经排队信号切到 GUI 线程（与 :func:`app_mcp.qt_dispatcher` 同一机制）；
  动作打开模态对话框（``exec()`` 进入嵌套事件循环）时不等它返回；
- ``QLineEdit`` 的密码回显模式只显示 ``••••``，拒绝填写与按键。
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import logging
import threading
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any, TypeVar

from .._client import ToolCallError
from ..dispatchers import qt_dispatcher
from ._inspector import UiInspector
from ._qt_elements import DECLARED_PROPERTY, QtWindowElement, is_alive, qt
from ._tools import UiFallbackTools

__all__ = ["QtUiFallback", "QtUiFallbackOptions", "QtUiPlatform", "declare_mcp_tools", "undeclare_mcp_tools"]

logger = logging.getLogger("app_mcp.uifallback")

T = TypeVar("T")


@dataclass(frozen=True)
class QtUiFallbackOptions:
    """:meth:`QtUiFallback.enable` 的选项。

    @input prefix 工具名前缀；max_items ``outline`` 缺省列出的控件数；settle_delay 动作后再等的秒数（再取变化摘要）；
        call_timeout GUI 线程迟迟不执行时的上限（秒，超时以 ``APP_NOT_RESPONDING`` 失败）；
        block_grace 动作已开始执行但这么久仍未返回（进入模态对话框的嵌套事件循环）时视为已执行（秒）。
    """

    prefix: str = "ui"
    max_items: int = 60
    settle_delay: float = 0.05
    call_timeout: float = 10.0
    block_grace: float = 0.2

    def __post_init__(self) -> None:
        if not self.prefix:
            raise ValueError("prefix 不能为空")
        if self.max_items < 1:
            raise ValueError("max_items 必须为正整数")
        if self.settle_delay < 0 or self.call_timeout <= 0 or self.block_grace <= 0:
            raise ValueError("settle_delay 不能为负，call_timeout / block_grace 必须为正")


def declare_mcp_tools(obj: Any, *tools: str) -> None:
    """标出控件（``QWidget``）或菜单项（``QAction``）已有声明的工具：大纲中显示 ``[已声明：…]``。必须在 GUI 线程调用。

    标在外层容器上时给其子树中唯一的控件。
    """
    current = obj.property(DECLARED_PROPERTY)
    names = [n for n in (current.split(", ") if isinstance(current, str) and current else []) if n]
    names.extend(t for t in tools if t and t not in names)
    obj.setProperty(DECLARED_PROPERTY, ", ".join(names))


def undeclare_mcp_tools(obj: Any, *tools: str) -> None:
    """撤销 :func:`declare_mcp_tools`。"""
    current = obj.property(DECLARED_PROPERTY)
    names = [n for n in (current.split(", ") if isinstance(current, str) else []) if n and n not in tools]
    obj.setProperty(DECLARED_PROPERTY, ", ".join(names))


def _top_level_windows() -> list[Any]:
    """可见、未最小化的顶层窗口（不含提示气泡、桌面）。"""
    q = qt()
    excluded = (q.core.Qt.WindowType.ToolTip, q.core.Qt.WindowType.Desktop, q.core.Qt.WindowType.SplashScreen)
    return [
        w
        for w in q.widgets.QApplication.topLevelWidgets()
        if w.isVisible() and not w.isMinimized() and w.windowType() not in excluded
    ]


def _modal_blocked(modal: Any) -> set[int]:
    """窗口模态对话框遮挡的窗口：其父窗口链（应用模态遮挡全部，由调用方处理）。"""
    blocked: set[int] = set()
    parent = modal.parentWidget()
    while parent is not None:
        top = parent.window()
        blocked.add(id(top))
        parent = top.parentWidget()
    return blocked


class QtUiPlatform:
    """Qt 平台：窗口与模态、GUI 线程切换、等界面稳定。

    @invariant ``windows`` / ``all_windows`` / ``is_live`` 只在 GUI 线程调用；``run`` / ``perform`` / ``settle`` 在任意 asyncio 循环上调用。
    """

    def __init__(self, options: QtUiFallbackOptions) -> None:
        q = qt()
        app = q.widgets.QApplication.instance()
        if app is None:
            raise RuntimeError("请先创建 QApplication 再开启 Qt 控件兜底")
        self._app = app
        self._options = options
        self._dispatch = qt_dispatcher()

    def windows(self) -> Sequence[QtWindowElement]:
        """最上层的弹出层（菜单、下拉列表）遮挡全部窗口；模态对话框遮挡其下（应用模态：全部；窗口模态：父窗口链）。"""
        q = qt()
        popup = q.widgets.QApplication.activePopupWidget()
        if popup is not None:
            return [QtWindowElement(popup)]
        tops = _top_level_windows()
        modal = q.widgets.QApplication.activeModalWidget()
        if modal is not None:
            if modal.windowModality() == q.core.Qt.WindowModality.WindowModal:
                blocked = _modal_blocked(modal)
                rest = [w for w in tops if id(w) not in blocked and w is not modal]
                return [QtWindowElement(w) for w in [*rest, modal]]
            return [QtWindowElement(modal)]
        active = q.widgets.QApplication.activeWindow()
        ordered = [w for w in tops if w is not active] + ([active] if active in tops else [])
        return [QtWindowElement(w) for w in ordered]

    def all_windows(self) -> Sequence[QtWindowElement]:
        q = qt()
        tops = _top_level_windows()
        popup = q.widgets.QApplication.activePopupWidget()
        return [QtWindowElement(w) for w in tops + ([popup] if popup is not None and popup not in tops else [])]

    def is_live(self, identity: Any) -> bool:
        if not is_alive(identity):
            return False
        window = getattr(identity, "window", None)
        return bool(window().isVisible()) if callable(window) else True

    def has_visible_window(self) -> bool:
        return bool(_top_level_windows())

    def _on_gui_thread(self) -> bool:
        return qt().core.QThread.currentThread() == self._app.thread()

    def _post(self, fn: Callable[[], T]) -> tuple[concurrent.futures.Future[T], threading.Event]:
        future: concurrent.futures.Future[T] = concurrent.futures.Future()
        started = threading.Event()

        def job() -> None:
            if not future.set_running_or_notify_cancel():
                return
            started.set()
            try:
                future.set_result(fn())
            except BaseException as e:  # noqa: BLE001 - 原样交给等待方
                future.set_exception(e)

        self._dispatch(job)
        return future, started

    def _not_responding(self) -> ToolCallError:
        return ToolCallError("APP_NOT_RESPONDING", f"Qt GUI 线程 {self._options.call_timeout:g} 秒内未能执行兜底操作")

    async def run(self, fn: Callable[[], T]) -> T:
        if self._on_gui_thread():
            return fn()
        future, _ = self._post(fn)
        try:
            return await asyncio.wait_for(asyncio.wrap_future(future), self._options.call_timeout)
        except asyncio.TimeoutError:
            raise self._not_responding() from None

    async def perform(self, fn: Callable[[], bool]) -> bool:
        # @why 总是排队执行（即使已在 GUI 线程）：动作可能进入模态对话框的嵌套事件循环，不能让调用方跟着阻塞。
        future, started = self._post(fn)
        waiting = asyncio.wrap_future(future)
        loop = asyncio.get_running_loop()
        deadline = loop.time() + self._options.call_timeout
        while True:
            try:
                return bool(await asyncio.wait_for(asyncio.shield(waiting), self._options.block_grace))
            except asyncio.TimeoutError:
                if started.is_set():
                    future.add_done_callback(_log_late_failure)
                    return True
                if loop.time() >= deadline:
                    future.cancel()
                    raise self._not_responding() from None

    async def settle(self) -> None:
        await asyncio.sleep(self._options.settle_delay)
        await self.run(lambda: None)  # 让 GUI 线程处理完此前排队的事件


def _log_late_failure(future: concurrent.futures.Future[Any]) -> None:
    if not future.cancelled() and future.exception() is not None:
        logger.warning("兜底动作在嵌套事件循环结束后失败：%s", future.exception())


class QtUiFallback:
    """Qt Widgets 进程内控件兜底。用 :meth:`enable` 创建。

    @invariant 状态监听只用顶层 ``QWindow`` 的 ``visibleChanged`` / ``windowStateChanged`` 与应用的
        ``focusWindowChanged`` / ``applicationStateChanged`` 信号：空闲时没有定时器、线程或全局事件过滤器。
    """

    def __init__(self, tools: UiFallbackTools, platform: QtUiPlatform) -> None:
        self._tools = tools
        self._platform = platform
        self.inspector = tools.inspector
        self._hooked: dict[int, Any] = {}
        self._connections: list[tuple[Any, Callable[..., None]]] = []
        self._closed = False

    @property
    def enabled(self) -> bool:
        """兜底工具当前是否启用（有可见窗口）。"""
        return self._tools.enabled

    @classmethod
    def enable(cls, registrar: Any, options: QtUiFallbackOptions | None = None) -> QtUiFallback:
        """在 ``registrar``（客户端或作用域）下建子作用域 ``<prefix>-fallback`` 并注册兜底工具。

        必须在 Qt GUI 线程、创建 ``QApplication`` 之后调用。
        """
        opts = options or QtUiFallbackOptions()
        platform = QtUiPlatform(opts)
        tools = UiFallbackTools(registrar, UiInspector(platform, opts.prefix, opts.max_items))
        fallback = cls(tools, platform)
        try:
            fallback._watch()
            fallback.refresh()
        except BaseException:
            fallback.close()
            raise
        return fallback

    def _connect(self, signal: Any, slot: Callable[..., None]) -> None:
        signal.connect(slot)
        self._connections.append((signal, slot))

    def _watch(self) -> None:
        gui = qt().gui.QGuiApplication
        self._connect(gui.instance().focusWindowChanged, self._on_change)
        self._connect(gui.instance().applicationStateChanged, self._on_change)

    def _on_change(self, *_: Any) -> None:
        # @why QWindow.visibleChanged 在 QWidget::hide() 清除可见标志之前发出：推迟到本轮事件处理之后再判定。
        qt().core.QTimer.singleShot(0, self.refresh)

    def _hook_windows(self) -> None:
        for window in qt().gui.QGuiApplication.topLevelWindows():
            if id(window) in self._hooked and self._hooked[id(window)] is window:
                continue
            self._hooked[id(window)] = window
            self._connect(window.visibleChanged, self._on_change)
            self._connect(window.windowStateChanged, self._on_change)
        for key in [k for k, w in self._hooked.items() if not is_alive(w)]:
            del self._hooked[key]

    def refresh(self) -> None:
        """重新判定"有可见窗口"并启用 / 禁用工具。窗口显示 / 隐藏 / 最小化时自动调用；
        应用不经焦点变化就显示首个窗口时（无激活的 ``show()``）可手动调用。必须在 GUI 线程调用。"""
        if self._closed:
            return
        self._hook_windows()
        visible = self._platform.has_visible_window()
        if self._tools.set_visible(visible) and not visible:
            self.inspector.clear()

    def close(self) -> None:
        """注销全部兜底工具并断开监听（幂等）。必须在 GUI 线程调用。"""
        if self._closed:
            return
        self._closed = True
        for signal, slot in self._connections:
            try:
                signal.disconnect(slot)
            except (RuntimeError, TypeError):  # 窗口已销毁 / 已断开
                pass
        self._connections.clear()
        self._hooked.clear()
        self.inspector.clear()
        self._tools.close()
