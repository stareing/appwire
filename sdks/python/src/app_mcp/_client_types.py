"""``_client`` 的公开类型：去重、错误、调用上下文与结果（纯移动自 ``_client.py``）。"""

from __future__ import annotations

import threading
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, ClassVar

from . import app_mcp_uniffi as ffi
from ._client_convert import (
    ERROR_KINDS,
    _RESULT_STATUSES,
    ContentAnnotationsLike,
    ResultStatusLike,
    _content_annotations,
    _enum_arg,
    _ms,
    logger,
)


# ---------------------------------------------------------------------------
# 公开类型
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class CallDedup:
    """调用去重（spec/protocol.md 3.3）：已开始执行的 ``callId`` 的首次结果在 ``ttl`` 秒内重放，最多保留 ``max_entries`` 条。

    任一为 0 关闭去重（:attr:`OFF`）。缺省（``AppMcp(call_dedup=None)``）为 300 秒、64 条。命中时记一条 warning 日志。
    """

    ttl: float = 300.0
    max_entries: int = 64

    OFF: ClassVar[CallDedup]

    def __post_init__(self) -> None:
        if self.ttl < 0 or self.max_entries < 0:
            raise ValueError(f"call_dedup 的 ttl / max_entries 不能为负：{self!r}")

    def _ffi(self) -> ffi.CallDedupPolicy:
        return ffi.CallDedupPolicy(ttl_ms=_ms(self.ttl), max_entries=self.max_entries)


CallDedup.OFF = CallDedup(0, 0)


class ToolCallError(Exception):
    """handler 抛出此异常以指定错误类别，如 ``ToolCallError("INVALID_INPUT", "数量必须为正")``。

    其他异常一律映射为 ``HANDLER_ERROR``。
    ``details`` 为可 JSON 序列化的结构化详情：对象的字段合并进错误的 ``data``，其他值放在 ``data.details``。
    """

    def __init__(self, kind: str, message: str, details: Any = None) -> None:
        super().__init__(message)
        if kind not in ERROR_KINDS:
            raise ValueError(f"未知的错误类别：{kind!r}")
        self.kind = kind
        self.message = message
        self.details = details

    @classmethod
    def user_action_required(cls, message: str, reason: str | None = None, uri: str | None = None) -> ToolCallError:
        """``USER_ACTION_REQUIRED``（spec/protocol.md 第 4 节）：需要用户本人操作后才能继续（登录过期、系统权限未授予、
        需切到前台、需在 App 内确认），如
        ``raise ToolCallError.user_action_required("登录已过期，请重新登录", UserActionReason.LOGIN, "shop://login")``。

        @input message 面向用户的说明（Agent 应转告用户）
        @input reason 可选类别：``UserActionReason`` 中的值或其他字符串；``None`` 时不出现在 ``data`` 中
        @input uri 可选的 App 内入口（深链接等）；``None`` 时不出现在 ``data`` 中
        """
        fields = {k: v for k, v in (("reason", reason), ("uri", uri)) if v is not None}
        return cls("USER_ACTION_REQUIRED", message, fields or None)


class UserActionReason:
    """``USER_ACTION_REQUIRED`` 的 ``data.reason`` 建议取值（接收方遇到其他值按原样展示）。"""

    LOGIN = "login"
    """登录已过期 / 未登录。"""
    PERMISSION = "permission"
    """系统权限未授予（相机、位置、通知等）。"""
    FOREGROUND = "foreground"
    """需要把 App 切到前台。"""
    CONFIRM = "confirm"
    """需要用户在 App 内确认。"""


class Hold:
    """阻止自动休眠的持有（``client.hold()`` / ``ctx.hold()``）。``release()`` 幂等；可用作上下文管理器::

        with client.hold():
            long_task()
    """

    __slots__ = ("_inner", "_released")

    def __init__(self, inner: ffi.Hold) -> None:
        self._inner = inner
        self._released = False

    @property
    def released(self) -> bool:
        return self._released

    def release(self) -> None:
        if not self._released:
            self._released = True
            self._inner.release()

    def __enter__(self) -> Hold:
        return self

    def __exit__(self, *exc: object) -> None:
        self.release()


class ToolResult:
    """结构化调用结果（spec/protocol.md 3.2），作为 handler 返回值。直接返回普通值 = ``done`` 且无附加信息。

    - ``data``：返回值；``None`` 表示无返回值（Hub 对模型输出"已完成"）。
    - ``state_hints``：调用后内容可能变化的资源名（与 ``ctx.add_state_hint`` 合并）。
    - ``status``：``"pending"``（已受理、待 App 内确认或异步完成）/ ``"partial"`` / ``"noop"``；缺省 ``"done"``。
    - ``state_resource``：``pending`` 时可读取后续状态的资源名。
    - ``summary``：一句面向模型 / 用户的结论（``partial`` 时说明完成了哪部分）。
    - ``annotations``：结果内容的标注（``{"audience": ["user"], "priority": 0.5, "last_modified": "…"}``
      或 ``ContentAnnotations``），Hub 原样转发。

    ``status`` / ``annotations`` 非法时构造即抛 ``ValueError``（handler 内抛出 → ``HANDLER_ERROR``）。
    """

    __slots__ = ("data", "state_hints", "status", "state_resource", "summary", "annotations")

    def __init__(
        self,
        data: Any = None,
        state_hints: list[str] | None = None,
        *,
        status: ResultStatusLike = "done",
        state_resource: str | None = None,
        summary: str | None = None,
        annotations: ContentAnnotationsLike | None = None,
    ) -> None:
        self.data = data
        self.state_hints = list(state_hints or [])
        self.status: ffi.ResultStatus = _enum_arg(status, _RESULT_STATUSES, " status")
        self.state_resource = state_resource
        self.summary = summary
        self.annotations = _content_annotations(annotations)

    def _ffi(self, data_json: str, state_hints: list[str]) -> ffi.CallResult:
        return ffi.CallResult(
            data_json=data_json,
            state_hints=state_hints,
            status=self.status,
            state_resource=self.state_resource,
            summary=self.summary,
            annotations=self.annotations,
        )


class ToolContext:
    """一次工具调用的上下文。handler 可声明 ``ctx: ToolContext`` 参数（或名为 ``ctx`` 的参数）获取。"""

    def __init__(self, call: ffi.Call, arguments: dict[str, Any]) -> None:
        self._call = call
        self.call_id: str = call.call_id()
        self.tool_name: str = call.tool_name()
        # Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 None。App 自行决定如何使用（如作为业务去重键）。
        self.idempotency_key: str | None = call.idempotency_key()
        self.arguments = arguments
        self.state_hints: list[str] = []
        self.cancel_reason: ffi.CancelReason | None = None
        self._cancelled = threading.Event()
        self._lock = threading.Lock()
        self._on_cancel: list[Callable[[], None]] = []

    @property
    def cancelled(self) -> bool:
        return self._cancelled.is_set()

    def wait_cancelled(self, timeout: float | None = None) -> bool:
        """阻塞直到调用被取消或超时，返回是否已取消。"""
        return self._cancelled.wait(timeout)

    def add_state_hint(self, resource_name: str) -> None:
        """声明调用后内容可能变化的资源，提示模型重新读取。"""
        self.state_hints.append(resource_name)

    def hold(self) -> Hold:
        """延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的 :class:`Hold` 被释放。

        调用已结束（完成、取消）时抛 ``AppMcpError.AlreadyCompleted``。
        """
        return Hold(self._call.hold())

    def progress(self, progress: float, total: float | None = None, message: str | None = None) -> None:
        """报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP ``notifications/progress``）。

        ``progress`` 应递增（不递增的值被 Host 丢弃），``total`` 未知时省略。调用已结束、已取消或未连接时无副作用。
        """
        if self._cancelled.is_set():
            return
        try:
            self._call.report_progress(float(progress), None if total is None else float(total), message)
        except ffi.AppMcpError.AlreadyCompleted:
            # @why 调用刚结束：进度只是提示，不影响结果。
            pass

    def on_cancel(self, fn: Callable[[], None]) -> None:
        """注册取消回调（在原生分发线程上调用）；已取消时立即调用。"""
        with self._lock:
            if not self._cancelled.is_set():
                self._on_cancel.append(fn)
                return
        fn()

    def _fire_cancel(self, reason: ffi.CancelReason) -> None:
        with self._lock:
            if self._cancelled.is_set():
                return
            self.cancel_reason = reason
            self._cancelled.set()
            callbacks, self._on_cancel = self._on_cancel, []
        for fn in callbacks:
            try:
                fn()
            except Exception:  # 回调异常不能影响分发线程
                logger.exception("取消回调抛出异常")


class _CancelListener(ffi.CancelListener):
    def __init__(self, ctx: ToolContext) -> None:
        self._ctx = ctx

    def on_cancel(self, reason: ffi.CancelReason) -> None:
        self._ctx._fire_cancel(reason)


class NavigationDenied(Exception):
    """导航回调抛出此异常拒绝本次导航（``NAVIGATION_DENIED``，spec/protocol.md 3.4），如用户正在输入、页面需要登录。

    ``message`` 面向模型 / 用户。其他异常按导航失败（``NAVIGATION_FAILED``）回复。
    """

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message
