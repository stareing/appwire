"""常用 GUI 框架的调度函数，传给 ``AppMcp(dispatcher=...)``，让同步 handler 在 UI 线程执行。"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

__all__ = ["tk_dispatcher", "qt_dispatcher"]


def tk_dispatcher(root: Any) -> Callable[[Callable[[], None]], None]:
    """Tk：用 ``root.after(0, fn)`` 切到 Tk 主循环（需要线程化的 Tcl，CPython 默认如此）。"""

    def dispatch(fn: Callable[[], None]) -> None:
        root.after(0, fn)

    return dispatch


def qt_dispatcher() -> Callable[[Callable[[], None]], None]:
    """Qt（PySide6 / PyQt6）：通过排队的信号连接切到 ``QCoreApplication`` 所在线程。

    必须在创建 ``QApplication`` 之后调用。
    """
    try:
        from PySide6.QtCore import QCoreApplication, QObject, Signal, Slot  # type: ignore
    except ImportError:
        from PyQt6.QtCore import QCoreApplication, QObject  # type: ignore
        from PyQt6.QtCore import pyqtSignal as Signal  # type: ignore
        from PyQt6.QtCore import pyqtSlot as Slot  # type: ignore

    app = QCoreApplication.instance()
    if app is None:
        raise RuntimeError("请先创建 QApplication 再调用 qt_dispatcher()")

    class _Bridge(QObject):  # type: ignore[misc]
        run = Signal(object)

        @Slot(object)
        def _exec(self, fn: Callable[[], None]) -> None:
            fn()

    bridge = _Bridge()
    bridge.moveToThread(app.thread())
    bridge.run.connect(bridge._exec)  # 接收者在主线程 → 自动为排队连接

    def dispatch(fn: Callable[[], None]) -> None:
        bridge.run.emit(fn)

    dispatch._bridge = bridge  # type: ignore[attr-defined]  # 保持引用
    return dispatch
