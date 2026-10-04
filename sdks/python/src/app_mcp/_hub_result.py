"""Hub SDK 的调用结果（自 ``hub.py`` 移出）：:class:`CallResult`、:class:`ToolError` 与 ``CallOutcome`` 的转换。"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any

from app_mcp_hub import app_mcp_hub_uniffi as ffi

ToolErrorInfo = ffi.ToolErrorInfo
AppOverviewInfo = ffi.AppOverviewInfo
ResultStatus = ffi.ResultStatus
ContentAnnotations = ffi.ContentAnnotations


def _loads(text: str | None) -> Any:
    return None if text is None else json.loads(text)


class ToolError(Exception):
    """工具调用以错误结束（``CallResult.unwrap()``）。``kind`` 为协议错误类别，如 ``USER_REJECTED``。"""

    def __init__(self, kind: str, message: str, details: Any = None) -> None:
        super().__init__(f"{kind}: {message}")
        self.kind = kind
        self.message = message
        self.details = details


@dataclass
class CallResult:
    """一次工具调用的结果。"""

    call_id: str
    #: 成功时的结果数据（已解析的 JSON）。
    data: Any = None
    #: 失败时的错误（``error.kind`` 如 ``USER_REJECTED``、``TIMEOUT``）。
    error: ToolErrorInfo | None = None
    state_hints: list[str] = field(default_factory=list)
    #: 实际执行的实例。
    instance_id: str | None = None
    #: 本会话首次接触该 App 时附带的总览。
    overview: AppOverviewInfo | None = None
    #: App 声明的业务状态（缺省 ``DONE``；``PENDING`` 时后续状态见 ``state_resource``）。
    status: ResultStatus = ResultStatus.DONE
    #: ``PENDING`` 时可读取后续状态的资源 URI（``app-mcp://<appId>/<名>``）。
    state_resource: str | None = None
    #: App 给出的一句结论。
    summary: str | None = None
    #: App 对结果内容的标注，原样。
    annotations: ContentAnnotations | None = None
    #: App 在后台、Hub 改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则为 ``None``。
    routed_to: str | None = None
    #: 从 Hub 收到调用到得出结果的毫秒数（spec/hub-api.md 3.15 ``dev.appwire/durationMs``）。
    duration_ms: int = 0
    #: 本次调用是否唤醒了 App（``dev.appwire/woke``）。
    woke: bool = False
    #: 结果来自只读结果缓存（未转发给 App）时距 App 产出的毫秒数（``dev.appwire/cached.ageMs``，spec/hub-api.md 3.20）；
    #: 未命中为 ``None``。
    cached_age_ms: int | None = None

    @property
    def ok(self) -> bool:
        return self.error is None

    def unwrap(self) -> Any:
        """成功时返回数据，失败时抛出 :class:`ToolError`。"""
        if self.error is not None:
            raise ToolError(self.error.kind, self.error.message, _loads(self.error.details_json))
        return self.data


def _call_result(out: ffi.CallOutcome) -> CallResult:
    """``CallOutcome``（生成的记录）→ :class:`CallResult`；结果数据解析为 JSON 值。"""
    return CallResult(
        call_id=out.call_id,
        data=_loads(out.data_json),
        error=out.error,
        state_hints=list(out.state_hints),
        instance_id=out.instance_id,
        overview=out.overview,
        status=out.status,
        state_resource=out.state_resource,
        summary=out.summary,
        annotations=out.annotations,
        routed_to=out.routed_to,
        duration_ms=out.duration_ms,
        woke=out.woke,
        cached_age_ms=out.cached_age_ms,
    )
