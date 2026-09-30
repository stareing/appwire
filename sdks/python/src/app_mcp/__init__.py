"""app-mcp Python SDK：把 App 内的业务动作以 MCP 工具暴露给模型。

需要先运行 ``bindings/uniffi/scripts/generate.sh`` 生成 ``app_mcp_uniffi.py`` 与原生库。

依赖 App 端原生库的符号（``AppMcp``、``Risk`` 等）在首次访问时才加载，因此只用 Hub SDK
（``import app_mcp.hub``）或纯 Python 辅助（``app_mcp.linux``、``app_mcp.single_instance``）时
不需要 ``app_mcp_uniffi`` 生成物。
"""

from __future__ import annotations

import importlib
from typing import TYPE_CHECKING, Any

from ._lifecycle import LifecyclePolicy, WakeDescriptor
from ._schema import schema_from_function
from .dispatchers import qt_dispatcher, tk_dispatcher

if TYPE_CHECKING:  # pragma: no cover
    from . import app_mcp_uniffi as ffi
    from ._client import (
        ERROR_KINDS,
        AppMcp,
        Dispatcher,
        Hold,
        ResourceHandle,
        Scope,
        ToolCallError,
        ToolContext,
        ToolHandle,
        ToolResult,
    )

    AppMcpError = ffi.AppMcpError
    AppOverview = ffi.AppOverview
    CancelReason = ffi.CancelReason
    Risk = ffi.Risk
    Activation = ffi.Activation
    Visibility = ffi.Visibility
    StateInfo = ffi.StateInfo
    StateStatus = ffi.StateStatus
    WakeReason = ffi.WakeReason
    SleepReason = ffi.SleepReason

# 名称 → (模块, 属性)
_LAZY: dict[str, tuple[str, str]] = {
    **{
        name: ("._client", name)
        for name in (
            "ERROR_KINDS",
            "AppMcp",
            "Dispatcher",
            "Hold",
            "ResourceHandle",
            "Scope",
            "ToolCallError",
            "ToolContext",
            "ToolHandle",
            "ToolResult",
        )
    },
    **{
        name: (".app_mcp_uniffi", name)
        for name in (
            "AppMcpError",
            "AppOverview",
            "CancelReason",
            "Risk",
            "Activation",
            "Visibility",
            "StateInfo",
            "StateStatus",
            "WakeReason",
            "SleepReason",
        )
    },
    "ffi": (".app_mcp_uniffi", ""),
}


def __getattr__(name: str) -> Any:
    try:
        module_name, attr = _LAZY[name]
    except KeyError:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}") from None
    module = importlib.import_module(module_name, __name__)
    value = getattr(module, attr) if attr else module
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(_LAZY))


__all__ = [
    "ERROR_KINDS",
    "Activation",
    "AppMcp",
    "AppMcpError",
    "AppOverview",
    "CancelReason",
    "Dispatcher",
    "Hold",
    "LifecyclePolicy",
    "ResourceHandle",
    "Risk",
    "Scope",
    "SleepReason",
    "StateInfo",
    "StateStatus",
    "ToolCallError",
    "ToolContext",
    "ToolHandle",
    "ToolResult",
    "Visibility",
    "WakeDescriptor",
    "WakeReason",
    "qt_dispatcher",
    "schema_from_function",
    "tk_dispatcher",
]
