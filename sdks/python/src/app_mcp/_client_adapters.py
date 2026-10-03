"""``_client`` 的完成辅助与 uniffi 回调适配器（纯移动自 ``_client.py``）。"""

from __future__ import annotations

import inspect
import json
import logging
import threading
from collections.abc import Callable
from typing import TYPE_CHECKING, Any

from . import app_mcp_uniffi as ffi
from ._schema import ArgumentBinder, to_jsonable
from ._client_convert import logger
from ._client_types import NavigationDenied, ToolCallError, ToolContext, ToolResult, _CancelListener

if TYPE_CHECKING:  # pragma: no cover
    from ._client import AppMcp


# ---------------------------------------------------------------------------
# 完成辅助
# ---------------------------------------------------------------------------


def _error_of(exc: BaseException) -> tuple[str, str, Any]:
    if isinstance(exc, NavigationDenied):
        return "NAVIGATION_DENIED", exc.message, None
    if isinstance(exc, ToolCallError):
        return exc.kind, exc.message, exc.details
    if isinstance(exc, ValueError) and getattr(exc, "_app_mcp_invalid_input", False):
        return "INVALID_INPUT", str(exc), None
    return "HANDLER_ERROR", str(exc) or type(exc).__name__, None


def _complete_ok(target: ffi.Call | ffi.Read, value: Any, hints: list[str] | None) -> None:
    """``hints`` 为 ``None`` 表示资源读取，否则为工具调用；``value`` 为 :class:`ToolResult` 时以完整结果提交。"""
    data = value.data if isinstance(value, ToolResult) else value
    try:
        text = json.dumps(data, default=to_jsonable, ensure_ascii=False)
    except (TypeError, ValueError) as e:
        _complete_err(target, "HANDLER_ERROR", f"返回值无法序列化为 JSON：{e}")
        return
    try:
        if hints is None:  # 资源读取
            target.complete(text)
        elif isinstance(value, ToolResult):
            target.complete_with(value._ffi(text, hints))
        else:
            target.complete(text, hints)
    except ffi.AppMcpError.AlreadyCompleted:
        pass  # 已取消或超时
    except ffi.AppMcpError as e:
        logger.warning("提交结果失败：%r", e)
        _complete_err(target, "HANDLER_ERROR", f"提交结果失败：{e}")


def _complete_err(target: ffi.Call | ffi.Read, kind: str, message: str, details: Any = None) -> None:
    try:
        if details is not None and hasattr(target, "fail_with_details"):
            try:
                text = json.dumps(details, default=to_jsonable, ensure_ascii=False)
            except (TypeError, ValueError) as e:
                logger.warning("错误详情无法序列化为 JSON，已忽略：%s", e)
                target.fail(kind, message)
            else:
                target.fail_with_details(kind, message, text)
        else:
            target.fail(kind, message)
    except ffi.AppMcpError.AlreadyCompleted:
        pass
    except ffi.AppMcpError.UnknownErrorKind:
        _complete_err(target, "HANDLER_ERROR", message, details)
    except ffi.AppMcpError as e:
        logger.warning("提交错误失败：%r", e)


class _Gate:
    """保证一次调用只“开始”或“超时”其中之一。"""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._state = "queued"  # queued → running | abandoned

    def begin(self) -> bool:
        with self._lock:
            if self._state != "queued":
                return False
            self._state = "running"
            return True

    def abandon(self) -> bool:
        with self._lock:
            if self._state != "queued":
                return False
            self._state = "abandoned"
            return True


# ---------------------------------------------------------------------------
# 适配器：uniffi 回调接口 → Python 函数
# ---------------------------------------------------------------------------


class _Registration:
    """一个已注册函数的执行方式。"""

    def __init__(self, owner: AppMcp, fn: Callable[..., Any], binder: ArgumentBinder | None) -> None:
        self.owner = owner
        self.fn = fn
        self.binder = binder
        self.is_async = inspect.iscoroutinefunction(fn)


class _ToolAdapter(ffi.ToolHandler):
    def __init__(self, reg: _Registration) -> None:
        self._reg = reg

    def invoke(self, call: ffi.Call) -> None:  # 分发线程
        reg = self._reg
        try:
            raw = call.arguments_json()
            args = json.loads(raw) if raw else {}
            if not isinstance(args, dict):
                raise ValueError("参数必须是 JSON 对象")
        except ValueError as e:
            _complete_err(call, "INVALID_INPUT", f"参数不是合法的 JSON 对象：{e}")
            return
        ctx = ToolContext(call, args)
        call.set_cancel_listener(_CancelListener(ctx))
        if ctx.cancelled:
            return

        def prepare() -> dict[str, Any]:
            assert reg.binder is not None
            try:
                return reg.binder.bind(args, ctx)
            except (ValueError, TypeError) as e:
                err = ValueError(str(e))
                err._app_mcp_invalid_input = True  # type: ignore[attr-defined]
                raise err from e

        def finish(result: Any) -> None:
            hints = list(ctx.state_hints)
            if isinstance(result, ToolResult):
                hints = result.state_hints + hints
            _complete_ok(call, result, hints)

        reg.owner._execute(reg, prepare, finish, lambda k, m, d=None: _complete_err(call, k, m, d), ctx)


class _ResourceAdapter(ffi.ResourceReader):
    def __init__(self, reg: _Registration) -> None:
        self._reg = reg

    def read(self, read: ffi.Read) -> None:  # 分发线程
        self._reg.owner._execute(
            self._reg,
            dict,
            lambda value: _complete_ok(read, value, None),
            lambda k, m, d=None: _complete_err(read, k, m, d),
            None,
        )


NavigateFunction = Callable[[str, "dict[str, Any] | None"], Any]


def _finish_navigate(request: ffi.Navigate, deny: str | None = None, fail: str | None = None) -> None:
    try:
        if deny is not None:
            request.deny(deny)
        elif fail is not None:
            request.fail(fail)
        else:
            request.complete()
    except ffi.AppMcpError:
        pass  # 已完成或连接已断开：回复被丢弃（spec/protocol.md 3.4）


def _user_action_fields(details: Any) -> tuple[str | None, str | None]:
    """``USER_ACTION_REQUIRED`` 详情中的字符串 ``reason`` / ``uri``（缺失或不是字符串时为 ``None``）。"""
    if not isinstance(details, dict):
        return None, None
    reason, uri = details.get("reason"), details.get("uri")
    return (reason if isinstance(reason, str) else None, uri if isinstance(uri, str) else None)


def _fail_navigate(request: ffi.Navigate, kind: str, message: str, details: Any = None) -> None:
    """导航回调抛出的异常（已映射为错误类别）→ 回复：``NAVIGATION_DENIED`` 拒绝，``USER_ACTION_REQUIRED`` 需要用户操作
    （带 reason / uri），其他按失败（``NAVIGATION_FAILED``）。"""
    if kind == "NAVIGATION_DENIED":
        _finish_navigate(request, deny=message)
    elif kind == "USER_ACTION_REQUIRED":
        reason, uri = _user_action_fields(details)
        try:
            request.fail_user_action(message, reason, uri)
        except ffi.AppMcpError:
            pass  # 已完成或连接已断开：回复被丢弃（spec/protocol.md 3.4）
    else:
        _finish_navigate(request, fail=message)


class _NavigationAdapter(ffi.NavigationHandler):
    def __init__(self, owner: AppMcp, fn: NavigateFunction) -> None:
        self._owner = owner
        self._fn = fn
        self._is_async = inspect.iscoroutinefunction(fn)

    def navigate(self, request: ffi.Navigate) -> None:  # 分发线程
        try:
            raw = request.params_json()
            params = None if raw is None else json.loads(raw)
            if params is not None and not isinstance(params, dict):
                raise ValueError("页面参数必须是 JSON 对象")
        except ValueError as e:
            _finish_navigate(request, fail=f"页面参数不合法：{e}")
            return
        page, fn = request.page(), self._fn
        if self._is_async:

            async def call() -> Any:
                return await fn(page, params)

        else:

            def call() -> Any:
                return fn(page, params)

        self._owner._execute(
            _Registration(self._owner, call, None),
            dict,
            lambda _result: _finish_navigate(request),
            lambda k, m, d=None: _fail_navigate(request, k, m, d),
            None,
        )


class _ClientListener(ffi.ClientListener):
    def __init__(self, owner: AppMcp) -> None:
        self._owner = owner

    def on_state_changed(self, state: ffi.StateInfo) -> None:
        self._owner._state_changed(state)

    def on_paired(self, token: str) -> None:
        cb = self._owner._on_paired
        if cb is not None:
            try:
                cb(token)
            except Exception:
                logger.exception("on_paired 回调抛出异常")

    def on_log(self, level: ffi.LogLevel, message: str) -> None:
        py_level = {
            ffi.LogLevel.DEBUG: logging.DEBUG,
            ffi.LogLevel.INFO: logging.INFO,
            ffi.LogLevel.WARN: logging.WARNING,
            ffi.LogLevel.ERROR: logging.ERROR,
        }.get(level, logging.INFO)
        logger.log(py_level, "%s", message)

    def on_idle_exit(self) -> None:
        self._owner._idle_exit()
