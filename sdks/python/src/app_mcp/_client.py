"""app-mcp Python SDK 的惯用封装（建在 uniffi 生成的 ``app_mcp_uniffi`` 之上）。

线程模型
--------
原生运行时在自己的**分发线程**上同步调用 handler 回调，回调必须尽快返回。本模块在回调里只做
参数解析和登记，然后：

- 同步函数：交给 ``dispatcher``（默认内部线程池；可传入切到 Qt / Tk 主线程的调度函数）执行；
- ``async def``：用 ``asyncio.run_coroutine_threadsafe`` 投递到事件循环（默认内部后台循环线程，
  可用 ``loop=`` 指定应用自己的循环，如 qasync 的 Qt 循环）。

函数执行完后从所在线程调用 ``Call.complete`` / ``Call.fail``。
调度后超过 ``dispatch_timeout`` 秒仍未开始执行（UI 线程卡住）时以 ``APP_NOT_RESPONDING`` 失败。
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import enum
import inspect
import json
import logging
import threading
from collections.abc import Callable, Mapping
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any, ClassVar, Literal, TypeVar, Union

from . import app_mcp_uniffi as ffi
from ._lifecycle import LifecyclePolicy, WakeDescriptor
from ._schema import ArgumentBinder, to_jsonable

__all__ = [
    "AppMcp",
    "Scope",
    "ToolHandle",
    "ResourceHandle",
    "ToolContext",
    "ToolResult",
    "ToolCallError",
    "UserActionReason",
    "Dispatcher",
    "Hold",
    "CallDedup",
]

logger = logging.getLogger("app_mcp")

Dispatcher = Callable[[Callable[[], None]], None]
"""调度函数：接收一个无参可调用对象，并在合适的线程上执行它。"""

F = TypeVar("F", bound=Callable[..., Any])

RiskLike = Union[str, ffi.Risk]
ActivationLike = Union[str, ffi.Activation]
ToolAnnotationsLike = Union[ffi.ToolAnnotations, Mapping[str, Any]]
ContentAnnotationsLike = Union[ffi.ContentAnnotations, Mapping[str, Any]]
ResultStatusLike = Union[str, ffi.ResultStatus]


class _Unset(enum.Enum):
    """``ToolHandle.update`` 参数的缺省标记：区分"未给出"（保持不变）与显式 ``None``（清除）。"""

    UNSET = enum.auto()


_UNSET = _Unset.UNSET

_RISKS = {
    "read": ffi.Risk.READ,
    "write": ffi.Risk.WRITE,
    "destructive": ffi.Risk.DESTRUCTIVE,
    "payment": ffi.Risk.PAYMENT,
    "os-sensitive": ffi.Risk.OS_SENSITIVE,
}
_ACTIVATIONS = {
    "headless": ffi.Activation.HEADLESS,
    "background": ffi.Activation.BACKGROUND,
    "foreground": ffi.Activation.FOREGROUND,
}
_VISIBILITIES = {
    "visible": ffi.Visibility.VISIBLE,
    "hidden": ffi.Visibility.HIDDEN,
    "frozen": ffi.Visibility.FROZEN,
}
_MODES = {
    "persistent": ffi.LifecycleMode.PERSISTENT,
    "idle": ffi.LifecycleMode.IDLE,
    "on-demand": ffi.LifecycleMode.ON_DEMAND,
}
_RESIDENCIES = {
    "keep": ffi.Residency.KEEP,
    "exit-when-idle": ffi.Residency.EXIT_WHEN_IDLE,
    "exit-always": ffi.Residency.EXIT_ALWAYS,
}
_WAKE_KINDS = {
    "uri": ffi.WakeKind.URI,
    "aumid": ffi.WakeKind.AUMID,
    "apple-event": ffi.WakeKind.APPLE_EVENT,
    "dbus": ffi.WakeKind.DBUS,
    "android-intent": ffi.WakeKind.ANDROID_INTENT,
    "web-url": ffi.WakeKind.WEB_URL,
    "none": ffi.WakeKind.NONE,
}
_HEARTBEATS = {
    "auto": ffi.HeartbeatMode.AUTO,
    "always": ffi.HeartbeatMode.ALWAYS,
    "off": ffi.HeartbeatMode.OFF,
}
_WAKE_REASONS = {
    "os-activation": ffi.WakeReason.OS_ACTIVATION,
    "app": ffi.WakeReason.APP,
    "visible": ffi.WakeReason.VISIBLE,
    "cold-start": ffi.WakeReason.COLD_START,
}
_RESULT_STATUSES = {
    "done": ffi.ResultStatus.DONE,
    "pending": ffi.ResultStatus.PENDING,
    "partial": ffi.ResultStatus.PARTIAL,
    "noop": ffi.ResultStatus.NOOP,
}
_AUDIENCES = {
    "user": ffi.Audience.USER,
    "assistant": ffi.Audience.ASSISTANT,
}
_TOOL_ANNOTATION_KEYS = frozenset(
    {"title", "read_only_hint", "destructive_hint", "idempotent_hint", "open_world_hint"}
)
_SLEEP_REASONS = {
    "idle": ffi.SleepReason.IDLE,
    "grace": ffi.SleepReason.GRACE,
    "background": ffi.SleepReason.BACKGROUND,
    "app": ffi.SleepReason.APP,
}
ERROR_KINDS: frozenset[str] = frozenset(ffi.error_kinds())


def _ms(seconds: float) -> int:
    return max(0, int(round(seconds * 1000)))


def _lifecycle_to_ffi(policy: LifecyclePolicy) -> ffi.LifecyclePolicy:
    wake = policy.wake
    return ffi.LifecyclePolicy(
        mode=_MODES[policy.mode],
        idle_timeout_ms=_ms(policy.idle_timeout),
        hidden_idle_timeout_ms=_ms(policy.hidden_idle_timeout),
        grace_ms=_ms(policy.grace),
        residency=_RESIDENCIES[policy.residency],
        wake=None
        if wake is None
        else ffi.WakeDescriptor(kind=_WAKE_KINDS[wake.kind], target=wake.target, background=wake.background),
        host_absent_retries=policy.host_absent_retries,
        legacy_timers=policy.legacy_timers,
        merge_window_ms=_ms(policy.merge_window),
        sleep_on_background=policy.sleep_on_background,
    )


def _enum_arg(value: Any, table: dict[str, Any], what: str) -> Any:
    if not isinstance(value, str):
        return value
    try:
        return table[value.lower().replace("_", "-")]
    except KeyError:
        raise ValueError(f"未知的{what}：{value!r}（可选 {sorted(table)}）") from None


def _risk(value: RiskLike | None) -> ffi.Risk | None:
    if value is None or isinstance(value, ffi.Risk):
        return value
    try:
        return _RISKS[value.lower().replace("_", "-")]
    except KeyError:
        raise ValueError(f"未知的 risk：{value!r}（可选 {sorted(_RISKS)}）") from None


def _tool_annotations(value: ToolAnnotationsLike | None) -> ffi.ToolAnnotations | None:
    """``dict`` 用 snake_case 键（``read_only_hint`` 等）；未知键抛 ``ValueError``。"""
    if value is None or isinstance(value, ffi.ToolAnnotations):
        return value
    unknown = set(value) - _TOOL_ANNOTATION_KEYS
    if unknown:
        raise ValueError(f"未知的工具注解字段：{sorted(unknown)}（可选 {sorted(_TOOL_ANNOTATION_KEYS)}）")
    return ffi.ToolAnnotations(**value)


def _content_annotations(value: ContentAnnotationsLike | None) -> ffi.ContentAnnotations | None:
    """``dict`` 键为 ``audience``（``"user"`` / ``"assistant"`` 列表）、``priority``、``last_modified``。"""
    if value is None or isinstance(value, ffi.ContentAnnotations):
        return value
    unknown = set(value) - {"audience", "priority", "last_modified"}
    if unknown:
        raise ValueError(f"未知的内容注解字段：{sorted(unknown)}（可选 audience、priority、last_modified）")
    audience = value.get("audience")
    return ffi.ContentAnnotations(
        audience=None if audience is None else [_enum_arg(a, _AUDIENCES, " audience") for a in audience],
        priority=value.get("priority"),
        last_modified=value.get("last_modified"),
    )


def _schema_json(schema: dict[str, Any] | str | None) -> str | None:
    """字典或 JSON 文本 → 规范化的 JSON 文本；非法 JSON 抛 ``ValueError``。"""
    if schema is None:
        return None
    return json.dumps(json.loads(schema) if isinstance(schema, str) else schema)


def _activation(value: ActivationLike | None) -> ffi.Activation | None:
    if value is None or isinstance(value, ffi.Activation):
        return value
    try:
        return _ACTIVATIONS[value.lower()]
    except KeyError:
        raise ValueError(f"未知的 activation：{value!r}（可选 {sorted(_ACTIVATIONS)}）") from None


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


# ---------------------------------------------------------------------------
# 完成辅助
# ---------------------------------------------------------------------------


def _error_of(exc: BaseException) -> tuple[str, str, Any]:
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


# ---------------------------------------------------------------------------
# 句柄
# ---------------------------------------------------------------------------


class ToolHandle:
    """已注册的工具。"""

    def __init__(self, inner: ffi.Tool, spec: ffi.ToolSpec) -> None:
        self._inner = inner
        self._spec = spec

    @property
    def name(self) -> str:
        return self._inner.name()

    def set_enabled(self, enabled: bool) -> None:
        self._inner.set_enabled(enabled)
        # @why update() 整体替换定义，须记住当前启用状态，否则之后的 update 会把它改回去
        self._spec = _replace_spec(self._spec, enabled=enabled)

    def update(
        self,
        *,
        description: str | _Unset = _UNSET,
        input_schema: dict[str, Any] | str | None | _Unset = _UNSET,
        risk: RiskLike | None | _Unset = _UNSET,
        activation: ActivationLike | None | _Unset = _UNSET,
        title: str | None | _Unset = _UNSET,
        annotations: ToolAnnotationsLike | None | _Unset = _UNSET,
        output_schema: dict[str, Any] | str | None | _Unset = _UNSET,
    ) -> None:
        """修改定义：未给出的字段保持不变；显式传 ``None`` 清除该声明（恢复注册时的缺省）。

        ``input_schema=None`` 为无参数，``risk=None`` 为缺省风险，``title`` / ``activation`` /
        ``annotations`` / ``output_schema`` 为 ``None`` 时清除声明。``description`` 不可清除。
        """
        s = self._spec
        spec = _replace_spec(
            s,
            description=s.description if description is _UNSET else description,
            input_schema_json=s.input_schema_json if input_schema is _UNSET else _schema_json(input_schema),
            risk=s.risk if risk is _UNSET else _risk(risk),
            activation=s.activation if activation is _UNSET else _activation(activation),
            title=s.title if title is _UNSET else title,
            annotations=s.annotations if annotations is _UNSET else _tool_annotations(annotations),
            output_schema_json=s.output_schema_json if output_schema is _UNSET else _schema_json(output_schema),
        )
        self._inner.update(spec)
        self._spec = spec

    def dispose(self) -> None:
        self._inner.dispose()


def _replace_spec(spec: ffi.ToolSpec, **changes: Any) -> ffi.ToolSpec:
    """复制 ``ToolSpec`` 并替换给出的字段（uniffi 记录不是 dataclass）。"""
    fields = {
        "name": spec.name,
        "description": spec.description,
        "input_schema_json": spec.input_schema_json,
        "risk": spec.risk,
        "activation": spec.activation,
        "title": spec.title,
        "enabled": spec.enabled,
        "annotations": spec.annotations,
        "output_schema_json": spec.output_schema_json,
    }
    fields.update(changes)
    return ffi.ToolSpec(**fields)


class ResourceHandle:
    """已注册的资源。"""

    def __init__(self, inner: ffi.Resource) -> None:
        self._inner = inner

    @property
    def name(self) -> str:
        return self._inner.name()

    def notify_changed(self) -> None:
        self._inner.notify_changed()

    def dispose(self) -> None:
        self._inner.dispose()


# ---------------------------------------------------------------------------
# 注册入口（客户端与 Scope 共用）
# ---------------------------------------------------------------------------


class _Registrar:
    _owner: AppMcp

    def _raw(self) -> ffi.AppMcpClient | ffi.Scope:
        raise NotImplementedError

    def add_tool(
        self,
        fn: Callable[..., Any],
        name: str | None = None,
        description: str | None = None,
        *,
        input_schema: dict[str, Any] | str | None = None,
        risk: RiskLike | None = None,
        activation: ActivationLike | None = None,
        title: str | None = None,
        enabled: bool = True,
        annotations: ToolAnnotationsLike | None = None,
        output_schema: dict[str, Any] | str | None = None,
    ) -> ToolHandle:
        """注册函数为工具，返回句柄。``input_schema`` 缺省时从函数签名生成。

        ``risk`` 为旧写法，优先用 ``annotations``：标准 MCP 工具注解（``{"read_only_hint": True}`` 或
        ``ToolAnnotations``），原样转发给 Agent；为空时 Hub 按 ``risk`` 推导。``output_schema`` 为结果的
        JSON Schema（字典或 JSON 文本，MCP ``outputSchema``）。
        """
        binder = ArgumentBinder(fn, ToolContext)
        if input_schema is None:
            schema = binder.schema()
        else:
            schema = json.loads(input_schema) if isinstance(input_schema, str) else input_schema
        spec = ffi.ToolSpec(
            name=name or fn.__name__,
            description=description if description is not None else inspect.getdoc(fn) or "",
            input_schema_json=json.dumps(schema) if schema is not None else None,
            risk=_risk(risk),
            activation=_activation(activation),
            title=title,
            enabled=enabled,
            annotations=_tool_annotations(annotations),
            output_schema_json=_schema_json(output_schema),
        )
        adapter = _ToolAdapter(_Registration(self._owner, fn, binder))
        return ToolHandle(self._raw().register_tool(spec, adapter), spec)

    def tool(
        self,
        name: str | None = None,
        description: str | None = None,
        *,
        input_schema: dict[str, Any] | str | None = None,
        risk: RiskLike | None = None,
        activation: ActivationLike | None = None,
        title: str | None = None,
        enabled: bool = True,
        annotations: ToolAnnotationsLike | None = None,
        output_schema: dict[str, Any] | str | None = None,
    ) -> Callable[[F], F]:
        """装饰器形式的 :meth:`add_tool`。返回原函数；句柄可用 ``client.tools[name]`` 取得。"""

        def decorator(fn: F) -> F:
            handle = self.add_tool(
                fn,
                name,
                description,
                input_schema=input_schema,
                risk=risk,
                activation=activation,
                title=title,
                enabled=enabled,
                annotations=annotations,
                output_schema=output_schema,
            )
            self._owner.tools[handle.name] = handle
            return fn

        return decorator

    def add_resource(
        self,
        fn: Callable[[], Any],
        name: str | None = None,
        description: str | None = None,
        *,
        mime_type: str | None = None,
        realtime: bool = False,
        annotations: ContentAnnotationsLike | None = None,
    ) -> ResourceHandle:
        """注册资源读取函数（无参数，返回可 JSON 序列化的内容）。

        ``realtime``：需实时推送（spec/lifecycle.md 第 13 节 B3）——被订阅时阻止休眠、休眠中变化时回连推送；
        默认 ``False``：订阅不阻止休眠，变化在下次连接时补发。
        ``annotations``：资源内容的标注（``{"audience": ["user"], "priority": 0.5}``），Hub 放到 MCP ``resources/list``
        的资源注解上。读取函数抛出 :class:`ToolCallError`（含 :meth:`ToolCallError.user_action_required`）时，
        类别与详情原样交给 Host。
        """
        spec = ffi.ResourceSpec(
            name=name or fn.__name__,
            description=description if description is not None else inspect.getdoc(fn) or "",
            mime_type=mime_type,
            realtime=realtime,
            annotations=_content_annotations(annotations),
        )
        adapter = _ResourceAdapter(_Registration(self._owner, fn, None))
        return ResourceHandle(self._raw().register_resource(spec, adapter))

    def resource(
        self,
        name: str | None = None,
        description: str | None = None,
        *,
        mime_type: str | None = None,
        realtime: bool = False,
        annotations: ContentAnnotationsLike | None = None,
    ) -> Callable[[F], F]:
        """装饰器形式的 :meth:`add_resource`。句柄可用 ``client.resources[name]`` 取得。"""

        def decorator(fn: F) -> F:
            handle = self.add_resource(
                fn, name, description, mime_type=mime_type, realtime=realtime, annotations=annotations
            )
            self._owner.resources[handle.name] = handle
            return fn

        return decorator

    def scope(self, name: str) -> Scope:
        """创建子作用域。``dispose()``（或 ``with`` 结束）时注销其下全部工具与资源。"""
        return Scope(self._owner, self._raw().create_scope(name))


class Scope(_Registrar):
    def __init__(self, owner: AppMcp, inner: ffi.Scope) -> None:
        self._owner = owner
        self._inner = inner

    def _raw(self) -> ffi.Scope:
        return self._inner

    def dispose(self) -> None:
        self._inner.dispose()

    def __enter__(self) -> Scope:
        return self

    def __exit__(self, *exc: object) -> None:
        self.dispose()


# ---------------------------------------------------------------------------
# 客户端
# ---------------------------------------------------------------------------


class AppMcp(_Registrar):
    """app-mcp 客户端。

    >>> client = AppMcp(app_id="shop", app_name="Shop",
    ...                 overview=AppOverview(summary="网店：浏览商品、管理购物车"))
    >>> @client.tool("cart.add", description="加入购物车", risk="write")
    ... def add(sku: str, qty: int = 1) -> dict:
    ...     return {"ok": True}
    >>> client.start()

    ``lifecycle`` 为空时 ``persistent``（不休眠）；``heartbeat``：``"auto"``（默认，按传输：本地 IPC / 桌面回环不发）/
    ``"always"`` / ``"off"``（spec/lifecycle.md 第 11 节 A3）；``call_dedup``：调用去重（:class:`CallDedup`，
    缺省 300 秒、64 条，``CallDedup.OFF`` 关闭）。
    """

    def __init__(
        self,
        app_id: str,
        app_name: str,
        *,
        host_url: str | None = None,
        instance_id: str | None = None,
        app_version: str | None = None,
        instance_title: str | None = None,
        token: str | None = None,
        launch_token: str | None = None,
        max_concurrent_calls: int = 1,
        overview: ffi.AppOverview | str | None = None,
        dispatcher: Dispatcher | None = None,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatch_timeout: float | None = 10.0,
        on_state: Callable[[ffi.StateInfo], None] | None = None,
        on_paired: Callable[[str], None] | None = None,
        lifecycle: LifecyclePolicy | None = None,
        connect_timeout: float | None = None,
        on_idle_exit: Callable[[], None] | None = None,
        heartbeat: Literal["auto", "always", "off"] | ffi.HeartbeatMode = "auto",
        call_dedup: CallDedup | None = None,
    ) -> None:
        self._owner = self
        self._on_idle_exit = on_idle_exit
        self.lifecycle = lifecycle
        self.tools: dict[str, ToolHandle] = {}
        self.resources: dict[str, ResourceHandle] = {}
        self._on_state = on_state
        self._on_paired = on_paired
        self._dispatch_timeout = dispatch_timeout
        self._state_cond = threading.Condition()
        self._closed = False

        self._executor: concurrent.futures.ThreadPoolExecutor | None = None
        if dispatcher is None:
            self._executor = concurrent.futures.ThreadPoolExecutor(
                max_workers=max(4, max_concurrent_calls), thread_name_prefix="app-mcp"
            )
            executor = self._executor

            def default_dispatch(fn: Callable[[], None]) -> None:
                executor.submit(fn)

            dispatcher = default_dispatch
        self._dispatcher: Dispatcher = dispatcher

        self._loop = loop
        self._own_loop_thread: threading.Thread | None = None
        self._loop_lock = threading.Lock()

        config = ffi.ClientConfig(
            app_id=app_id,
            app_name=app_name,
            instance_id=instance_id,
            host_url=host_url,
            app_version=app_version,
            instance_title=instance_title,
            token=token,
            launch_token=launch_token,
            max_concurrent_calls=max_concurrent_calls,
            overview=ffi.AppOverview(summary=overview) if isinstance(overview, str) else overview,
            lifecycle=None if lifecycle is None else _lifecycle_to_ffi(lifecycle),
            connect_timeout_ms=None if connect_timeout is None else max(1, _ms(connect_timeout)),
            heartbeat=_enum_arg(heartbeat, _HEARTBEATS, "心跳策略"),
            call_dedup=None if call_dedup is None else call_dedup._ffi(),
        )
        self._inner = ffi.AppMcpClient(config, _ClientListener(self))
        self._state: ffi.StateInfo = self._inner.state()

    def _raw(self) -> ffi.AppMcpClient:
        return self._inner

    # -- 生命周期 ------------------------------------------------------------

    def start(self) -> AppMcp:
        """开始连接 Host（重复调用无效果）。"""
        self._inner.start()
        return self

    def stop(self) -> None:
        """停止：取消所有进行中的调用、断开连接、不再重连。"""
        self._inner.stop()

    def handle_wake(self, argv: str | Sequence[str]) -> bool:
        """处理操作系统激活参数：命令行参数列表（如 ``sys.argv[1:]``）、单个参数或 URL。

        识别 ``app-mcp-wake:<token>``、``<scheme>://app-mcp/wake?token=``、``#app-mcp-wake=<token>``；
        是本 SDK 的唤醒时发起回连并返回 ``True``。可以在 ``start()`` 之前调用（冷启动唤醒）。
        """
        if isinstance(argv, str):
            return self._inner.handle_wake(argv)
        for arg in argv:
            if ffi.parse_wake_token(arg) is not None:
                return self._inner.handle_wake(arg)
        return False

    def wake(self, reason: str | ffi.WakeReason | None = None) -> bool:
        """App 主动回连（如用户打开了相关界面）；``reason="visible"`` 表示窗口重新可见。返回是否发起了回连。"""
        if reason is None:
            return self._inner.wake()
        return self._inner.wake_with_reason(_enum_arg(reason, _WAKE_REASONS, "回连原因"))

    def connect_now(self) -> bool:
        """``on-demand`` 模式下主动连接；尚未 ``start()`` 时等同于 ``start()``。"""
        return self._inner.connect_now()

    def sleep(self, reason: str | ffi.SleepReason | None = None) -> bool:
        """App 主动请求休眠（默认原因 ``app``；进入后台时可用 ``"background"``）。返回是否有效果。"""
        if reason is None:
            return self._inner.sleep()
        return self._inner.sleep_with_reason(_enum_arg(reason, _SLEEP_REASONS, "休眠原因"))

    def hold(self) -> Hold:
        """临时阻止自动休眠，直到返回的 :class:`Hold` 被释放（``with client.hold(): ...``）。"""
        return Hold(self._inner.hold())

    @property
    def tools_hash(self) -> str:
        """当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。"""
        return self._inner.tools_hash()

    def _idle_exit(self) -> None:
        cb = self._on_idle_exit
        if cb is None:
            return

        def run() -> None:
            try:
                cb()
            except Exception:
                logger.exception("on_idle_exit 回调抛出异常")

        try:
            self._dispatcher(run)
        except Exception:  # 调度器已关闭：直接在分发线程上调用
            run()

    def close(self) -> None:
        """停止并释放内部线程池 / 事件循环。"""
        if self._closed:
            return
        self._closed = True
        self._inner.stop()
        if self._executor is not None:
            self._executor.shutdown(wait=False, cancel_futures=True)
        if self._own_loop_thread is not None and self._loop is not None:
            self._loop.call_soon_threadsafe(self._loop.stop)
            self._own_loop_thread.join(timeout=2)

    def __enter__(self) -> AppMcp:
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- 状态 ----------------------------------------------------------------

    @property
    def instance_id(self) -> str:
        return self._inner.instance_id()

    @property
    def token(self) -> str | None:
        return self._inner.token()

    @property
    def connection_id(self) -> str | None:
        """Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 cid 对应；未连接时为 None。"""
        return self._inner.connection_id()

    @property
    def state(self) -> ffi.StateInfo:
        return self._inner.state()

    def wait_for_state(self, status: ffi.StateStatus, timeout: float | None = None) -> bool:
        """阻塞直到进入指定状态，返回是否在超时前达到。"""
        with self._state_cond:
            return self._state_cond.wait_for(lambda: self._inner.state().status == status, timeout)

    def set_visibility(self, visibility: str | ffi.Visibility, focused: bool = True) -> None:
        if isinstance(visibility, str):
            visibility = _VISIBILITIES[visibility.lower()]
        self._inner.set_visibility(visibility, focused)

    def _state_changed(self, state: ffi.StateInfo) -> None:
        with self._state_cond:
            self._state = state
            self._state_cond.notify_all()
        if self._on_state is not None:
            try:
                self._on_state(state)
            except Exception:
                logger.exception("on_state 回调抛出异常")

    # -- 执行 ----------------------------------------------------------------

    def _event_loop(self) -> asyncio.AbstractEventLoop:
        with self._loop_lock:
            if self._loop is None:
                loop = asyncio.new_event_loop()
                ready = threading.Event()

                def run() -> None:
                    asyncio.set_event_loop(loop)
                    loop.call_soon(ready.set)
                    loop.run_forever()

                t = threading.Thread(target=run, name="app-mcp-asyncio", daemon=True)
                t.start()
                ready.wait()
                self._loop = loop
                self._own_loop_thread = t
            return self._loop

    def _execute(
        self,
        reg: _Registration,
        prepare: Callable[[], dict[str, Any]],
        finish: Callable[[Any], None],
        fail: Callable[..., None],
        ctx: ToolContext | None,
    ) -> None:
        """在分发线程上调用：把函数投递到目标线程 / 事件循环执行。"""
        fn = reg.fn
        if reg.is_async:
            loop = self._event_loop()

            async def run_async() -> None:
                try:
                    kwargs = prepare()
                    result = await fn(**kwargs)
                except asyncio.CancelledError:
                    fail("CANCELLED", "调用已取消")
                    return
                except BaseException as e:  # noqa: BLE001 - 所有异常都要转为调用失败
                    fail(*_error_of(e))
                    return
                finish(result)

            future = asyncio.run_coroutine_threadsafe(run_async(), loop)
            if ctx is not None:
                ctx.on_cancel(lambda: future.cancel())
            return

        gate = _Gate()
        timer: threading.Timer | None = None
        if self._dispatch_timeout is not None:
            timeout = self._dispatch_timeout

            def on_timeout() -> None:
                if gate.abandon():
                    fail("APP_NOT_RESPONDING", f"{timeout:g} 秒内未能在目标线程上开始执行")

            timer = threading.Timer(timeout, on_timeout)
            timer.daemon = True
            timer.start()

        def job() -> None:
            if timer is not None:
                timer.cancel()
            if not gate.begin():
                return
            if ctx is not None and ctx.cancelled:
                return
            try:
                result = fn(**prepare())
            except BaseException as e:  # noqa: BLE001
                fail(*_error_of(e))
                return
            finish(result)

        try:
            self._dispatcher(job)
        except Exception as e:  # 调度器拒绝（例如线程池已关闭）
            if timer is not None:
                timer.cancel()
            if gate.abandon():
                fail("HANDLER_ERROR", f"无法调度 handler：{e}")
