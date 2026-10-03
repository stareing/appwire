"""回调执行：原生层在自己的线程上同步调用适配器，适配器把用户回调交给线程池 / 事件循环 / dispatcher，
完成后经句柄（``ApprovalResponder`` 等）回传结果。"""

from __future__ import annotations

import asyncio
import concurrent.futures
import inspect
import logging
import threading
from collections.abc import Awaitable, Callable
from typing import Any

from app_mcp_hub import app_mcp_hub_uniffi as ffi

_log = logging.getLogger("app_mcp.hub")

#: 把回调交给指定线程执行（如 UI 线程），见 ``app_mcp.dispatchers``。
Dispatcher = Callable[[Callable[[], None]], None]
ApprovalRequest = ffi.ApprovalRequest
PairingRequest = ffi.PairingRequest
WakeRequest = ffi.WakeRequest
ProgressUpdate = ffi.ProgressUpdate

_executor: concurrent.futures.ThreadPoolExecutor | None = None
_executor_lock = threading.Lock()


def _callback_executor() -> concurrent.futures.ThreadPoolExecutor:
    """执行同步回调（以及没有事件循环可用的 async 回调）的线程池。"""
    global _executor
    with _executor_lock:
        if _executor is None:
            _executor = concurrent.futures.ThreadPoolExecutor(thread_name_prefix="app-mcp-hub-cb")
        return _executor


class WakeFailed(Exception):
    """在唤醒回调中抛出，以指定的协议错误类别结束调用（其他异常按 ``LAUNCH_FAILED``）。"""

    def __init__(self, kind: str, message: str) -> None:
        super().__init__(f"{kind}: {message}")
        self.kind = kind
        self.message = message


async def _await(awaitable: Awaitable[Any]) -> Any:
    return await awaitable


def _schedule(
    fn: Callable[[Any], Any],
    arg: Any,
    loop: asyncio.AbstractEventLoop | None,
    dispatcher: Dispatcher | None,
    done: Callable[[BaseException | None, Any], None],
) -> None:
    """在合适的上下文执行用户回调（同步 / async），结束后调用 ``done(error, result)``。立即返回。

    - async 函数：有 ``loop`` 时在该循环上执行，否则在线程池中以 ``asyncio.run`` 执行；
    - 同步函数：有 ``dispatcher`` 时交给它（如切到 UI 线程），否则在线程池中执行；
      返回值若是 awaitable，在线程池中以 ``asyncio.run`` 等待。
    """

    def finish(error: BaseException | None, result: Any) -> None:
        try:
            done(error, result)
        except Exception:  # noqa: BLE001 - 句柄已失效等，不影响后续回调
            _log.exception("回传回调结果失败")

    def settle(result: Any) -> None:
        if inspect.isawaitable(result):
            _callback_executor().submit(run_awaitable, result)
        else:
            finish(None, result)

    def run_awaitable(awaitable: Awaitable[Any]) -> None:
        try:
            result = asyncio.run(_await(awaitable))
        except BaseException as e:  # noqa: BLE001 - 转交给句柄
            finish(e, None)
        else:
            finish(None, result)

    def run_sync() -> None:
        try:
            result = fn(arg)
        except BaseException as e:  # noqa: BLE001 - 转交给句柄
            finish(e, None)
        else:
            settle(result)

    if inspect.iscoroutinefunction(fn):
        if loop is not None and not loop.is_closed():
            future = asyncio.run_coroutine_threadsafe(fn(arg), loop)

            def on_done(f: concurrent.futures.Future[Any]) -> None:
                if f.cancelled():
                    finish(asyncio.CancelledError(), None)
                elif f.exception() is not None:
                    finish(f.exception(), None)
                else:
                    finish(None, f.result())

            future.add_done_callback(on_done)
        else:
            _callback_executor().submit(run_awaitable, fn(arg))
    elif dispatcher is not None:
        dispatcher(run_sync)
    else:
        _callback_executor().submit(run_sync)


def _decide(responder: Any, what: str) -> Callable[[BaseException | None, Any], None]:
    """审批 / 配对：异常视为拒绝。"""

    def done(error: BaseException | None, result: Any) -> None:
        if error is not None:
            _log.error("%s回调抛出异常，视为拒绝", what, exc_info=error)
            responder.complete(False)
        else:
            responder.complete(bool(result))

    return done


class _ApprovalAdapter(ffi.ApprovalHandler):
    def __init__(self, fn: Any, loop: Any, dispatcher: Any) -> None:
        self._fn, self._loop, self._dispatcher = fn, loop, dispatcher

    def on_request(self, request: ApprovalRequest, responder: ffi.ApprovalResponder) -> None:  # Hub 线程
        _schedule(self._fn, request, self._loop, self._dispatcher, _decide(responder, "审批"))


class _PairingAdapter(ffi.PairingHandler):
    def __init__(self, fn: Any, loop: Any, dispatcher: Any) -> None:
        self._fn, self._loop, self._dispatcher = fn, loop, dispatcher

    def on_request(self, request: PairingRequest, responder: ffi.PairingResponder) -> None:  # Hub 线程
        _schedule(self._fn, request, self._loop, self._dispatcher, _decide(responder, "配对"))


class _WakerAdapter(ffi.HubWaker):
    def __init__(self, fn: Any, loop: Any, dispatcher: Any) -> None:
        self._fn, self._loop, self._dispatcher = fn, loop, dispatcher

    def wake(self, request: WakeRequest, responder: ffi.WakeResponder) -> None:  # Hub 线程
        def done(error: BaseException | None, _result: Any) -> None:
            if error is None:
                responder.succeed()
            elif isinstance(error, WakeFailed):
                responder.fail(error.kind, error.message)
            else:
                _log.error("唤醒回调抛出异常", exc_info=error)
                responder.fail("LAUNCH_FAILED", str(error) or type(error).__name__)

        _schedule(self._fn, request, self._loop, self._dispatcher, done)


class _ProgressAdapter(ffi.ProgressListener):
    """进度回调（Hub 线程）→ 调用方事件循环线程上按顺序执行 ``fn``；返回 awaitable 时作为任务执行。"""

    def __init__(self, fn: Callable[[ProgressUpdate], Any], loop: asyncio.AbstractEventLoop) -> None:
        self._fn, self._loop = fn, loop

    def _deliver(self, update: ProgressUpdate) -> None:
        try:
            result = self._fn(update)
            if inspect.isawaitable(result):
                asyncio.ensure_future(result)
        except Exception:  # noqa: BLE001 - 进度回调异常不影响调用
            _log.exception("进度回调抛出异常，已忽略")

    def on_progress(self, update: ProgressUpdate) -> None:  # Hub 线程
        if not self._loop.is_closed():
            self._loop.call_soon_threadsafe(self._deliver, update)
