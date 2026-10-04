"""Hub 配置的字典形式（与 app-mcp-host 配置文件 / policy.json 相同的 JSON 键，也接受 snake_case）→ 生成的记录类型。

未知键 / 取值抛 ``ValueError``；取值本身的校验由 Hub 完成（不合法时 ``HubError.InvalidConfig`` / ``INVALID_INPUT``）。
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any, Union

from app_mcp_hub import app_mcp_hub_uniffi as ffi

Risk = ffi.Risk
LimitsConfig = ffi.LimitsConfig
EventLimitOverrides = ffi.EventLimitOverrides
CacheLimitOverrides = ffi.CacheLimitOverrides
OutputValidation = ffi.OutputValidation
PolicyConfig = ffi.PolicyConfig
PolicyRule = ffi.PolicyRule
PolicyAction = ffi.PolicyAction
PolicyHook = ffi.PolicyHook
AnnotationMatch = ffi.AnnotationMatch
AgentCredential = ffi.AgentCredential

RiskLike = Union[Risk, str]
LimitsLike = Union[LimitsConfig, dict[str, int]]
EventLimitsLike = Union[EventLimitOverrides, dict[str, int]]
CacheLimitsLike = Union[CacheLimitOverrides, dict[str, int]]
OutputValidationLike = Union[OutputValidation, str]
PolicyLike = Union[PolicyConfig, dict[str, Any]]
AgentsLike = list[Union[AgentCredential, dict[str, str]]]
IntentDefaultsLike = Mapping[str, str]

# LimitsConfig 字段 ← JSON 配置键（与 app-mcp-host 配置文件 ``limits`` 相同；也接受 snake_case）。
_LIMIT_KEYS = {
    "toolRatePerMinute": "tool_rate_per_minute",
    "toolRateBurst": "tool_rate_burst",
    "appRatePerMinute": "app_rate_per_minute",
    "appRateBurst": "app_rate_burst",
    "agentRatePerMinute": "agent_rate_per_minute",
    "agentRateBurst": "agent_rate_burst",
    "maxArgumentsBytes": "max_arguments_bytes",
    "maxResultBytes": "max_result_bytes",
    "maxResourceBytes": "max_resource_bytes",
}
# EventLimitOverrides 字段 ← JSON 配置键（与 hub-c / @app-mcp/hub 的 ``eventLimits`` 相同；也接受 snake_case）。
_EVENT_LIMIT_KEYS = {
    "maxSubscriptions": "max_subscriptions",
    "maxInboxEvents": "max_inbox_events",
    "inboxTtlMs": "inbox_ttl_ms",
    "perSubscriptionPerMinute": "per_subscription_per_minute",
}
# CacheLimitOverrides 字段 ← JSON 配置键（与 app-mcp-host 配置文件 ``resultCache`` 相同；也接受 snake_case）。
_CACHE_LIMIT_KEYS = {"maxEntries": "max_entries", "maxBytes": "max_bytes", "maxEntryBytes": "max_entry_bytes"}
# 策略规则的 JSON 键 → PolicyRule / AnnotationMatch 字段（与 app-mcp-host 的 policy.json 相同；也接受 snake_case）。
_RULE_KEYS = {"id": "id", "action": "action", "app": "app", "tool": "tool", "annotations": "annotations", "agent": "agent", "hooks": "hooks"}
_ANNOTATION_KEYS = {
    "readOnlyHint": "read_only_hint",
    "destructiveHint": "destructive_hint",
    "idempotentHint": "idempotent_hint",
    "openWorldHint": "open_world_hint",
}


def _risk(value: RiskLike | None) -> Risk | None:
    if value is None or isinstance(value, Risk):
        return value
    return Risk[value.strip().upper().replace("-", "_")]


def _limits(value: LimitsLike | None) -> LimitsConfig | None:
    """``LimitsConfig`` 或字典（JSON 配置键 ``toolRatePerMinute`` 等，或 snake_case）；未知键抛 ``ValueError``。"""
    if value is None or isinstance(value, LimitsConfig):
        return value
    fields = set(_LIMIT_KEYS.values())
    kwargs: dict[str, int] = {}
    for key, v in value.items():
        name = _LIMIT_KEYS.get(key, key)
        if name not in fields:
            raise ValueError(f"未知的 limits 字段：{key!r}（可选 {sorted(_LIMIT_KEYS)}）")
        kwargs[name] = v
    return LimitsConfig(**kwargs)


def _event_limits(value: EventLimitsLike | None) -> EventLimitOverrides | None:
    """``EventLimitOverrides`` 或字典（JSON 配置键 ``maxInboxEvents`` 等，或 snake_case）；未知键抛 ``ValueError``。"""
    if value is None or isinstance(value, EventLimitOverrides):
        return value
    return EventLimitOverrides(**_fields(value, _EVENT_LIMIT_KEYS, "event_limits"))


def _result_cache(value: CacheLimitsLike | None) -> CacheLimitOverrides | None:
    """``CacheLimitOverrides`` 或字典（JSON 配置键 ``maxEntries`` 等，或 snake_case）；未知键抛 ``ValueError``。"""
    if value is None or isinstance(value, CacheLimitOverrides):
        return value
    return CacheLimitOverrides(**_fields(value, _CACHE_LIMIT_KEYS, "result_cache"))


def _output_validation(value: OutputValidationLike | None) -> OutputValidation | None:
    if value is None or isinstance(value, OutputValidation):
        return value
    try:
        return OutputValidation[value.strip().upper()]
    except KeyError:
        raise ValueError(f"未知的 output_validation：{value!r}（可选 off、log、reject）") from None


def _enum(cls: Any, value: Any, what: str) -> Any:
    if isinstance(value, cls):
        return value
    try:
        return cls[str(value).strip().upper()]
    except KeyError:
        raise ValueError(f"未知的 {what}：{value!r}（可选 {', '.join(m.name.lower() for m in cls)}）") from None


def _fields(value: dict[str, Any], keys: dict[str, str], what: str) -> dict[str, Any]:
    """JSON 键（或 snake_case 字段名）→ 字段名；未知键抛 ``ValueError``。"""
    fields = set(keys.values())
    out: dict[str, Any] = {}
    for key, v in value.items():
        name = keys.get(key, key)
        if name not in fields:
            raise ValueError(f"未知的 {what} 字段：{key!r}（可选 {sorted(keys)}）")
        out[name] = v
    return out


def _policy_rule(value: PolicyRule | dict[str, Any]) -> PolicyRule:
    if isinstance(value, PolicyRule):
        return value
    kw = _fields(value, _RULE_KEYS, "策略规则")
    kw["action"] = _enum(PolicyAction, kw.get("action"), "action")
    ann = kw.get("annotations")
    if isinstance(ann, dict):
        kw["annotations"] = AnnotationMatch(**_fields(ann, _ANNOTATION_KEYS, "annotations"))
    if kw.get("hooks") is not None:
        kw["hooks"] = [_enum(PolicyHook, h, "hook") for h in kw["hooks"]]
    return PolicyRule(**kw)


def _policy(value: PolicyLike) -> PolicyConfig:
    """``PolicyConfig`` 或字典（JSON 形式 ``{"rules": [{"id", "action": "hide"|"deny", "app", ...}]}``）；
    未知键 / 取值抛 ``ValueError``（规则本身的校验由 Hub 完成）。"""
    if isinstance(value, PolicyConfig):
        return value
    kw = _fields(value, {"rules": "rules"}, "policy")
    return PolicyConfig(rules=[_policy_rule(r) for r in kw.get("rules", [])])


def _agents(value: AgentsLike) -> list[AgentCredential]:
    """``AgentCredential`` 或字典 ``{"name", "token"}`` 的列表；未知键抛 ``ValueError``（名字与令牌的校验由 Hub 完成）。"""
    return [
        a if isinstance(a, AgentCredential) else AgentCredential(**_fields(a, {"name": "name", "token": "token"}, "agent"))
        for a in value
    ]


def _intent_defaults(value: IntentDefaultsLike) -> dict[str, str]:
    """意图默认表（动词或 ``动词@主版本`` → 工具全名，与 ``intents.json`` 的 ``defaults`` 相同）；键或值不是字符串抛
    ``TypeError``（格式校验由 Hub 完成）。"""
    if not isinstance(value, Mapping):
        raise TypeError(f"intent_defaults 应为字典，实际为 {type(value).__name__}")
    for k, v in value.items():
        if not isinstance(k, str) or not isinstance(v, str):
            raise TypeError(f"intent_defaults 的键和值都应为字符串：{k!r}: {v!r}")
    return dict(value)
