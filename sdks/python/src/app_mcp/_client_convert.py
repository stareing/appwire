"""``_client`` 的类型别名、枚举映射表与参数转换（纯移动自 ``_client.py``）。"""

from __future__ import annotations

import enum
import json
import logging
from collections.abc import Callable, Iterable, Mapping
from typing import Any, TypeVar, Union

from . import app_mcp_uniffi as ffi
from ._lifecycle import LifecyclePolicy


logger = logging.getLogger("app_mcp")

Dispatcher = Callable[[Callable[[], None]], None]
"""调度函数：接收一个无参可调用对象，并在合适的线程上执行它。"""

F = TypeVar("F", bound=Callable[..., Any])

RiskLike = Union[str, ffi.Risk]
ActivationLike = Union[str, ffi.Activation]
SurfaceLike = Union[str, ffi.ToolSurface]
ToolAnnotationsLike = Union[ffi.ToolAnnotations, Mapping[str, Any]]
ContentAnnotationsLike = Union[ffi.ContentAnnotations, Mapping[str, Any]]
ResultStatusLike = Union[str, ffi.ResultStatus]
CacheLike = Union[ffi.CachePolicy, Mapping[str, Any], int]
DeprecationLike = Union[ffi.Deprecation, Mapping[str, Any], str]


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
_SURFACES = {
    "app": ffi.ToolSurface.APP,
    "view": ffi.ToolSurface.VIEW,
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


def _implements(value: Iterable[str]) -> list[str]:
    """意图名序列 → 列表；单个字符串视为一项（避免被拆成字符）。"""
    if isinstance(value, str):
        return [value]
    return list(value)


_CACHE_SCOPES = {"private": ffi.CacheScope.PRIVATE, "shared": ffi.CacheScope.SHARED}


def _cache(value: CacheLike | None) -> ffi.CachePolicy | None:
    """结果缓存声明（spec/protocol.md 3.6）：``int`` 为 ``ttl_ms``（``private``）；``dict`` 键为 ``ttl_ms``、``scope``
    （``"private"`` / ``"shared"``）。只做形状转换，``ttl_ms`` 越界由原生层在注册 / 更新时拒绝（``AppMcpError.InvalidConfig``）。

    @error 未知键、缺少 ``ttl_ms``、``ttl_ms`` 不是非负整数 → ``ValueError``；未知 ``scope`` → ``ValueError``。
    """
    if value is None or isinstance(value, ffi.CachePolicy):
        return value
    if isinstance(value, bool):
        raise ValueError(f"cache 不能是布尔值：{value!r}")
    if isinstance(value, int):
        value = {"ttl_ms": value}
    unknown = set(value) - {"ttl_ms", "scope"}
    if unknown:
        raise ValueError(f"未知的 cache 字段：{sorted(unknown)}（可选 ttl_ms、scope）")
    ttl = value.get("ttl_ms")
    if isinstance(ttl, bool) or not isinstance(ttl, int) or ttl < 0:
        raise ValueError(f"cache.ttl_ms 须为非负整数：{ttl!r}")
    scope = value.get("scope")
    return ffi.CachePolicy(ttl_ms=ttl, scope=None if scope is None else _enum_arg(scope, _CACHE_SCOPES, " cache scope"))


def _deprecation(value: DeprecationLike | None) -> ffi.Deprecation | None:
    """工具弃用声明（spec/protocol.md 3.7）：``str`` 为 ``message``；``dict`` 键为 ``message``、``replacement``、``until``。
    只做形状转换，格式（``message`` 长度、``replacement`` 局部名且不指向自身、``until`` 为 ``YYYY-MM-DD``）由原生层在
    注册 / 更新时校验（``AppMcpError.InvalidConfig``）。

    @error 未知键、缺少 ``message`` 或字段不是字符串 → ``ValueError``。
    """
    if value is None or isinstance(value, ffi.Deprecation):
        return value
    if isinstance(value, str):
        value = {"message": value}
    if not isinstance(value, Mapping):
        raise ValueError(f"deprecated 须为 Deprecation、dict 或字符串：{value!r}")
    unknown = set(value) - {"message", "replacement", "until"}
    if unknown:
        raise ValueError(f"未知的 deprecated 字段：{sorted(unknown)}（可选 message、replacement、until）")
    message = value.get("message")
    if not isinstance(message, str):
        raise ValueError(f"deprecated.message 须为字符串：{message!r}")
    optional = {k: value.get(k) for k in ("replacement", "until")}
    for key, v in optional.items():
        if v is not None and not isinstance(v, str):
            raise ValueError(f"deprecated.{key} 须为字符串：{v!r}")
    return ffi.Deprecation(message=message, **optional)


def _surface(value: SurfaceLike | None) -> ffi.ToolSurface | None:
    """``"app"`` / ``None`` → ``None``（缺省，不序列化）；``"view"`` → ``VIEW``。"""
    surface = None if value is None else _enum_arg(value, _SURFACES, "surface")
    return None if surface == ffi.ToolSurface.APP else surface


def _activation(value: ActivationLike | None) -> ffi.Activation | None:
    if value is None or isinstance(value, ffi.Activation):
        return value
    try:
        return _ACTIVATIONS[value.lower()]
    except KeyError:
        raise ValueError(f"未知的 activation：{value!r}（可选 {sorted(_ACTIVATIONS)}）") from None
