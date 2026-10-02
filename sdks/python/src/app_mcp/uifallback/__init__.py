"""进程内控件兜底（``ui.*`` 工具，spec/ui-fallback.md）：与 UI 框架无关的引擎。

框架适配：Qt Widgets 见 :mod:`app_mcp.uifallback.qt`（PySide6 / PyQt6，可选依赖，导入本包不需要 Qt）。
其他框架实现 :class:`UiElement` / :class:`UiWindow` / :class:`UiPlatform`，交给 :class:`UiInspector` 与
:class:`UiFallbackTools` 即可。
"""

from __future__ import annotations

from ._format import UiActionResult, UiEntry, UiEntryKind, UiOutline, UiReadResult
from ._input import FillBool, FillNumber, FillText, UiFillValue, UiKey, UiScrollDirection
from ._inspector import UiInspector, UiPlatform
from ._tools import UiFallbackTools
from ._tree import TRANSPARENT, UiDescription, UiElement, UiWindow

__all__ = [
    "TRANSPARENT",
    "FillBool",
    "FillNumber",
    "FillText",
    "UiActionResult",
    "UiDescription",
    "UiElement",
    "UiEntry",
    "UiEntryKind",
    "UiFallbackTools",
    "UiFillValue",
    "UiInspector",
    "UiKey",
    "UiOutline",
    "UiPlatform",
    "UiReadResult",
    "UiScrollDirection",
    "UiWindow",
]
