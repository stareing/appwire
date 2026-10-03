"""Hub SDK（Agent 端）：把"连接本机所有 App"的能力嵌进自己的助手 / Agent。

列工具、调用、导出 / 分派 LLM 工具格式（OpenAI、Anthropic、Gemini、MCP）、事件、审批与配对回调。
需要先运行 ``bindings/hub-uniffi/scripts/generate.sh`` 生成 ``app_mcp_hub`` 包里的绑定与原生库。

>>> from app_mcp.hub import Hub
>>> hub = Hub(approval_min_risk="destructive")
>>> hub.set_approval_handler(lambda req: ask_user(req))        # 同步或 async 均可
>>> tools = hub.export_tools("anthropic")                      # 放进 LLM 请求
>>> result = await hub.dispatch("anthropic", tool_use_block)   # 回填给 LLM
>>> hub.close()

线程模型：

- async 方法（``call_tool``、``dispatch``、``read_resource``、``serve_http``）在调用方的事件循环上等待，
  不阻塞；对应的 ``*_sync`` 版本供非 asyncio 代码使用（以 ``asyncio.run`` 执行，不要在事件循环线程上调用）。
- 事件回调在 Hub 的分发线程上同步执行（可传 ``dispatcher`` 切到 UI 线程，见 ``app_mcp.dispatchers``）；
  ``events()`` 提供 async 迭代器。
- 审批 / 配对 / 唤醒回调：原生层以同步回调 + 完成句柄交给本模块（不需要回调线程上有事件循环）。
  同步函数在线程池（或 ``dispatcher``）中执行，可以阻塞等待用户；async 函数在设置回调时所在的事件循环
  （或 ``loop`` 参数）上执行，没有循环时在线程池中以 ``asyncio.run`` 执行。

休眠与唤醒（spec/hub-api.md 3.5）：App 休眠后其工具仍列出（``Availability.DORMANT``），
``AppInfo.dormant_instances`` 列出休眠实例，并收到 ``HubEvent.APP_DORMANT``；调用这些工具时 Hub 生成一次性令牌、
发 ``HubEvent.APP_WAKING`` 并调用唤醒实现（默认按平台执行系统命令；可用 :meth:`Hub.set_waker` 替换）。
"""

from __future__ import annotations

import asyncio
import inspect
import json
import logging
import threading
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any, Union

try:
    from app_mcp_hub import app_mcp_hub_uniffi as ffi
except ImportError as e:  # pragma: no cover - 取决于是否已生成
    raise ImportError(
        "找不到 app_mcp_hub 绑定，请先运行 bash bindings/hub-uniffi/scripts/generate.sh"
    ) from e

_log = logging.getLogger("app_mcp.hub")

from ._hub_config import (  # noqa: E402  字典形式的配置 → 生成的记录类型
    AgentsLike,
    LimitsLike as LimitsLike,
    OutputValidationLike as OutputValidationLike,
    PolicyLike,
    RiskLike,
    _agents,
    _enum,
    _limits,
    _output_validation,
    _policy,
    _risk,
)
from ._hub_callbacks import (  # noqa: E402  用户回调的执行与适配器
    Dispatcher as Dispatcher,
    WakeFailed,
    _ApprovalAdapter,
    _await,
    _PairingAdapter,
    _ProgressAdapter,
    _WakerAdapter,
)

# 直接复用生成的数据类型。
HubConfig = ffi.HubConfig
UpstreamSpec = ffi.UpstreamSpec
ToolFilter = ffi.ToolFilter
ToolFormat = ffi.ToolFormat
Risk = ffi.Risk
#: 调用优先级（:meth:`Hub.call_tool` 的 ``priority``，第 16 项 P6，spec/hub-api.md 3.15）：``INTERACTIVE`` / ``NORMAL`` / ``BACKGROUND``。
CallPriority = ffi.CallPriority
Activation = ffi.Activation
Availability = ffi.Availability
Visibility = ffi.Visibility
AppKind = ffi.AppKind
AppInfo = ffi.AppInfo
InstanceInfo = ffi.InstanceInfo
AppOverviewInfo = ffi.AppOverviewInfo
HubTool = ffi.HubTool
#: App 工具对界面的依赖（``HubTool.surface``：``ToolSurface.APP`` / ``VIEW``，spec/protocol.md 3.4；内置与上游工具为 ``None``）。
ToolSurface = ffi.ToolSurface
HubResource = ffi.HubResource
ResourceContent = ffi.ResourceContent
#: 调用进度（``progress``、``total``、``message``；:meth:`Hub.call_tool` 的 ``on_progress``，spec/hub-api.md 3.12）。
ProgressUpdate = ffi.ProgressUpdate
ToolErrorInfo = ffi.ToolErrorInfo
ApprovalRequest = ffi.ApprovalRequest
PairingRequest = ffi.PairingRequest
#: 事件（``HubEvent.APP_CONNECTED`` 等变体；SDK 诊断上报为 ``HubEvent.APP_DIAGNOSTIC(app_id, instance_id,
#: code, message, count)``，spec/protocol.md 10.2；未单独映射的新事件为 ``HubEvent.OTHER(kind, json)``）。
HubEvent = ffi.HubEvent
#: Hub 操作错误（``HubError.Tool``、``InvalidJson``、``InvalidConfig``、``Io``、``Shutdown``、``Unsupported``）。
#: ``HubError.Unsupported``：本构建未包含所需能力（``mcp_http``、``upstreams``、``serve_http``），``detail`` 说明缺哪个
#: cargo feature（spec/hub-api.md 3.10）；重试无效。
HubError = ffi.HubError
WakeKind = ffi.WakeKind
WakeDescriptor = ffi.WakeDescriptor
#: 交给唤醒回调的请求（``app_id``、``instance_id``、``descriptor``、``token``、``activation_arg``）。
WakeRequest = ffi.WakeRequest
#: 工具暴露方式（``ToolExposure.AUTO`` / ``PROGRESSIVE`` / ``ALL``，spec/hub-api.md 3.7）。
ToolExposure = ffi.ToolExposure
#: MCP 出口协商的协议版本范围（``McpProtocolMode.AUTO`` / ``LEGACY_ONLY``，spec/hub-api.md 3.6「协议版本」）。
McpProtocolMode = ffi.McpProtocolMode
#: 唤醒器配置（``WakerConfig.SYSTEM()`` / ``DISABLED()`` / ``EXEC(argv=[...])``）。
WakerConfig = ffi.WakerConfig
# 运行状态（:meth:`Hub.status`，spec/hub-api.md 3.9）。
HubStatus = ffi.HubStatus
AuthStatus = ffi.AuthStatus
AppStatus = ffi.AppStatus
AppState = ffi.AppState
InstanceStatus = ffi.InstanceStatus
InstanceState = ffi.InstanceState
LastError = ffi.LastError
DiagnosticReport = ffi.DiagnosticReport
#: 休眠记录持久化状态（``HubStatus.dormant_store``，配置了 ``HubConfig.state_dir`` 时）。
DormantStoreStatus = ffi.DormantStoreStatus
StoreIssue = ffi.StoreIssue
#: Agent 任务（``HubStatus.tasks``，spec/hub-api.md 3.6）：调用方的跨请求状态。
AgentTaskStatus = ffi.AgentTaskStatus
#: 调用方的种类：``CallerKind.MCP_SESSION``（legacy MCP 会话）/ ``PRINCIPAL``（无会话 MCP 请求的主体）/ ``API``（Hub API 会话）。
CallerKind = ffi.CallerKind
TaskSelectionStatus = ffi.TaskSelectionStatus
TaskLeaseStatus = ffi.TaskLeaseStatus
#: 未到期的对象锁（``HubStatus.locks``，spec/hub-api.md 3.6「对象锁」）。
LockStatus = ffi.LockStatus
#: 进行中的调用（``HubStatus.calls``，spec/hub-api.md 3.6「调用对象」）：进度展开为 ``progress`` / ``progress_total`` /
#: ``progress_message``，``caller`` 只在 ``status()`` 中给出，``platform_state`` 为执行实例最近上报的可见性。
CallStatus = ffi.CallStatus
#: 调用阶段：``CallState.CREATED`` → ``APPROVING`` → ``ACTIVATING`` → ``RUNNING``（只前进，不需要的阶段跳过）。
CallState = ffi.CallState
# 资源保护与工具声明（spec/hub-api.md 3.11）。
#: 限流与大小上限（``HubConfig.limits``；``HubStatus.limits`` 为全部字段给出的生效值）。为空的字段取默认值。
LimitsConfig = ffi.LimitsConfig
#: 结果与其 ``outputSchema`` 不符时的处理：``OutputValidation.OFF`` / ``LOG``（默认）/ ``REJECT``。
OutputValidation = ffi.OutputValidation
#: 一个工具的声明（``AppStatus.tools``）。
ToolDeclaration = ffi.ToolDeclaration
#: 标准 MCP 工具注解（``HubTool.annotations``、``ApprovalRequest.annotations``）。
ToolAnnotations = ffi.ToolAnnotations
#: 内容标注（``CallResult.annotations``、``HubResource.annotations``）。
ContentAnnotations = ffi.ContentAnnotations
Audience = ffi.Audience
#: 调用结果的业务状态：``ResultStatus.DONE`` / ``PENDING`` / ``PARTIAL`` / ``NOOP``。
ResultStatus = ffi.ResultStatus
# 策略挂点（spec/hub-api.md 3.13）。
#: 策略规则集（``HubConfig.policy``、:meth:`Hub.set_policy`）；空规则集 = 不做任何限制。
PolicyConfig = ffi.PolicyConfig
#: 一条规则：``id``、``action``、``app``（可带末尾 ``*``）、``tool``、``annotations``、``hooks``。
PolicyRule = ffi.PolicyRule
#: ``PolicyAction.HIDE``（不出现在列表中，调用为 ``TOOL_NOT_FOUND``）/ ``DENY``（以 ``POLICY_DENIED`` 拒绝）。
PolicyAction = ffi.PolicyAction
#: 执行点；规则的 ``hooks`` 只能写 ``CALL`` / ``WAKE``。
PolicyHook = ffi.PolicyHook
#: 按 App 声明的 MCP 注解匹配。
AnnotationMatch = ffi.AnnotationMatch
#: 策略状态（:meth:`Hub.policy`、``HubStatus.policy``）：规则与命中次数、生效时刻、最近的加载错误。
PolicyStatus = ffi.PolicyStatus
PolicyRuleStatus = ffi.PolicyRuleStatus
PolicyLoadError = ffi.PolicyLoadError
#: Agent 访问令牌（``HubConfig.agents``、:meth:`Hub.set_agents`，spec/hub-api.md 3.6「Agent 身份」）；``repr`` 不含令牌。
AgentCredential = ffi.AgentCredential

FormatLike = Union[ToolFormat, str]
#: :data:`CallPriority` 或其名称（``"interactive"`` / ``"normal"`` / ``"background"``，不区分大小写）。
CallPriorityLike = Union[CallPriority, str]

__all__ = [
    "AgentCredential",
    "AgentTaskStatus",
    "AnnotationMatch",
    "Activation",
    "AppInfo",
    "AppKind",
    "AppOverviewInfo",
    "AppState",
    "AppStatus",
    "ApprovalRequest",
    "Audience",
    "AuthStatus",
    "Availability",
    "CallPriority",
    "CallPriorityLike",
    "CallResult",
    "CallerKind",
    "ContentAnnotations",
    "DiagnosticReport",
    "DormantStoreStatus",
    "EventStream",
    "Hub",
    "ProgressUpdate",
    "HubConfig",
    "HubError",
    "HubEvent",
    "HubResource",
    "HubStatus",
    "HubTool",
    "InstanceInfo",
    "InstanceState",
    "InstanceStatus",
    "LastError",
    "LimitsConfig",
    "LockStatus",
    "CallState",
    "CallStatus",
    "McpProtocolMode",
    "OutputValidation",
    "PairingRequest",
    "PolicyAction",
    "PolicyConfig",
    "PolicyHook",
    "PolicyLoadError",
    "PolicyRule",
    "PolicyRuleStatus",
    "PolicyStatus",
    "ResourceContent",
    "ResultStatus",
    "Risk",
    "StoreIssue",
    "TaskLeaseStatus",
    "TaskSelectionStatus",
    "ToolAnnotations",
    "ToolDeclaration",
    "ToolError",
    "ToolErrorInfo",
    "ToolExposure",
    "ToolFilter",
    "ToolFormat",
    "ToolSurface",
    "UpstreamSpec",
    "Visibility",
    "WakeDescriptor",
    "WakeFailed",
    "WakeKind",
    "WakeRequest",
    "WakerConfig",
    "init_logging",
    "parse_format",
]


# ---------------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------------


def parse_format(value: FormatLike) -> ToolFormat:
    """``ToolFormat`` 或格式名（``mcp``、``openai-chat``/``openai``、``openai-responses``、``anthropic``、``gemini``）。"""
    if isinstance(value, ToolFormat):
        return value
    return ffi.parse_tool_format(value)


def init_logging(filter: str | None = None) -> bool:
    """Hub 日志输出到 stderr（``RUST_LOG`` 语法）。只有第一次调用生效。"""
    return ffi.init_logging(filter)


def _loads(text: str | None) -> Any:
    return None if text is None else json.loads(text)


def _millis(seconds: float | None) -> int | None:
    return None if seconds is None else max(0, int(seconds * 1000))


def _filter(
    apps: list[str] | None = None,
    max_risk: RiskLike | None = None,
    only_available: bool = False,
    include_builtin: bool = True,
    session: str | None = None,
) -> ToolFilter:
    return ToolFilter(
        apps=apps,
        max_risk=_risk(max_risk),
        only_available=only_available,
        include_builtin=include_builtin,
        session=session,
    )


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

    @property
    def ok(self) -> bool:
        return self.error is None

    def unwrap(self) -> Any:
        """成功时返回数据，失败时抛出 :class:`ToolError`。"""
        if self.error is not None:
            raise ToolError(self.error.kind, self.error.message, _loads(self.error.details_json))
        return self.data


class _Listener(ffi.HubEventListener):
    def __init__(self, hub: Hub) -> None:
        self._hub = hub

    def on_event(self, event: HubEvent) -> None:  # 分发线程
        self._hub._emit(event)

    def on_lagged(self, skipped: int) -> None:  # 分发线程
        self._hub._emit_lagged(skipped)


class EventStream:
    """事件的 async 迭代器（创建时即开始接收）。``async for e in hub.events(): ...``"""

    def __init__(self, hub: Hub, loop: asyncio.AbstractEventLoop) -> None:
        self._loop = loop
        self._queue: asyncio.Queue[HubEvent | None] = asyncio.Queue()
        self._unsubscribe = hub.on_event(self._push)

    def _push(self, event: HubEvent) -> None:
        try:
            self._loop.call_soon_threadsafe(self._queue.put_nowait, event)
        except RuntimeError:  # 循环已关闭
            pass

    def __aiter__(self) -> EventStream:
        return self

    async def __anext__(self) -> HubEvent:
        item = await self._queue.get()
        if item is None:
            raise StopAsyncIteration
        return item

    async def next(self, timeout: float | None = None) -> HubEvent:
        """取下一个事件；超时抛出 ``TimeoutError``。"""
        return await asyncio.wait_for(self.__anext__(), timeout)

    async def wait_for(self, pred: Callable[[HubEvent], bool], timeout: float | None = None) -> HubEvent:
        """等待第一个满足 ``pred`` 的事件（其间的事件被丢弃）。"""

        async def loop() -> HubEvent:
            async for e in self:
                if pred(e):
                    return e
            raise StopAsyncIteration

        return await asyncio.wait_for(loop(), timeout)

    def close(self) -> None:
        self._unsubscribe()
        self._loop.call_soon_threadsafe(self._queue.put_nowait, None)

    def __enter__(self) -> EventStream:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


# ---------------------------------------------------------------------------
# Hub
# ---------------------------------------------------------------------------


class Hub:
    """嵌入式 Hub。

    参数与 :class:`HubConfig` 字段一致（``approval_min_risk`` 可用字符串，如 ``"destructive"``；``limits`` 可用字典，
    键同 JSON 配置，如 ``{"toolRatePerMinute": 60}``；``output_validation`` 可用 ``"off"`` / ``"log"`` / ``"reject"``；
    ``policy`` 可用 JSON 形式的字典，如 ``{"rules": [{"id": "no-pay", "action": "deny", "app": "shop", "tool": "order.*"}]}``）；
    也可直接传 ``config=HubConfig(...)``。
    """

    def __init__(self, config: HubConfig | None = None, **kwargs: Any) -> None:
        if config is None:
            if "approval_min_risk" in kwargs:
                kwargs["approval_min_risk"] = _risk(kwargs["approval_min_risk"])
            if "limits" in kwargs:
                kwargs["limits"] = _limits(kwargs["limits"])
            if "output_validation" in kwargs:
                kwargs["output_validation"] = _output_validation(kwargs["output_validation"])
            if kwargs.get("policy") is not None:
                kwargs["policy"] = _policy(kwargs["policy"])
            if kwargs.get("agents") is not None:
                kwargs["agents"] = _agents(kwargs["agents"])
            config = HubConfig(**kwargs)
        elif kwargs:
            raise TypeError("config 与关键字参数不能同时使用")
        self._inner = ffi.AppMcpHub.start(config)
        self._subs: list[Callable[[HubEvent], None]] = []
        self._lagged_subs: list[Callable[[int], None]] = []
        self._subs_lock = threading.Lock()
        self._closed = False
        self._inner.set_event_listener(_Listener(self))

    # -- 生命周期 ---------------------------------------------------------------

    def close(self) -> None:
        """停止 Hub（断开所有 App、结束后台任务）。可重复调用。"""
        if self._closed:
            return
        self._closed = True
        try:
            self._inner.set_event_listener(None)
        finally:
            self._inner.shutdown()

    def __enter__(self) -> Hub:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    async def __aenter__(self) -> Hub:
        return self

    async def __aexit__(self, *exc: object) -> None:
        self.close()

    @property
    def listen_addr(self) -> str | None:
        """HTTP 服务（``/app``、``/healthz``）的实际地址（端口 0 时为随机端口；App 端点为 ``ws://<地址>/app``）；未开启时为 ``None``。"""
        return self._inner.listen_addr()

    @property
    def ipc_endpoint(self) -> str | None:
        """本地 IPC 连接服务的端点（``unix:…`` / ``pipe:…``，可直接作为 App 端 SDK 的 ``host_url``）；
        未开启时为 ``None``。"""
        return self._inner.ipc_endpoint()

    # -- 查询 -------------------------------------------------------------------

    def apps(self) -> list[AppInfo]:
        return self._inner.apps()

    def tools(
        self,
        *,
        apps: list[str] | None = None,
        max_risk: RiskLike | None = None,
        only_available: bool = False,
        include_builtin: bool = True,
        session: str | None = None,
    ) -> list[HubTool]:
        """工具列表。渐进暴露生效且未给 ``apps`` 时，只含内置工具与 ``session`` 会话已展开 / 调用过 /
        选定了实例的 App 的工具（spec/hub-api.md 3.7）。"""
        return self._inner.tools(_filter(apps, max_risk, only_available, include_builtin, session))

    def resources(self) -> list[HubResource]:
        return self._inner.resources()

    def overview(self, app_id: str) -> AppOverviewInfo | None:
        return self._inner.overview(app_id)

    def status(self) -> HubStatus:
        """运行状态（spec/hub-api.md 3.9，与 ``GET /status`` 相同）：身份、监听位置、令牌策略、各 App 与实例的状态
        （实例带 ``info.connection_id``）、最近错误、最近的 SDK 诊断上报。已关闭时抛出 ``HubError.Shutdown``。"""
        return self._inner.status()

    def policy(self) -> PolicyStatus:
        """生效的策略规则、各规则命中次数与最近的加载错误（spec/hub-api.md 3.13）。"""
        return self._inner.policy()

    def set_policy(self, policy: PolicyLike) -> None:
        """替换策略规则集（命中计数清零；``{}`` 清空）。规则不合法时抛 ``HubError.Tool``（``kind == "INVALID_INPUT"``），
        之前的规则继续生效，原因记入 ``policy().last_error``；字典中有未知键 / 取值时抛 ``ValueError``。"""
        self._inner.set_policy(_policy(policy))

    def set_agents(self, agents: AgentsLike) -> None:
        """替换 Agent 登记（``[{"name", "token"}]`` 或 ``AgentCredential`` 列表；``[]`` 清空），只影响之后到达的 MCP 请求。
        不合法时抛 ``HubError.Tool``（``kind == "INVALID_INPUT"``），之前的登记继续生效。"""
        self._inner.set_agents(_agents(agents))

    def select_instance(self, app_id: str, instance_id: str | None) -> None:
        """设置全局默认实例（``None`` 恢复按规则路由）。"""
        self._inner.select_instance(app_id, instance_id)

    # -- 调用 -------------------------------------------------------------------

    async def call_tool(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        *,
        instance_id: str | None = None,
        timeout: float | None = None,
        session: str | None = None,
        call_id: str | None = None,
        idempotency_key: str | None = None,
        priority: CallPriorityLike | None = None,
        on_progress: Callable[[ProgressUpdate], Any] | None = None,
    ) -> CallResult:
        """调用工具（全名 ``<appId>.<tool>``，``timeout`` 单位秒）。

        工具层面的失败（用户拒绝、超时、App 报错……）放在 ``CallResult.error``；
        名称无法解析时抛出 :data:`HubError`。任务被取消时自动取消调用。
        ``on_progress``：接收调用进度（:data:`ProgressUpdate`，Hub 合并后），在调用方的事件循环线程上按顺序调用，
        都在结果返回之前；回调抛出的异常记日志后忽略。
        ``idempotency_key``：Agent 的幂等键（1..=256 个字符），原样转交 App（spec/hub-api.md 3.15）；不合法时
        ``CallResult.error.kind`` 为 ``INVALID_INPUT``。
        ``priority``：调用优先级（:data:`CallPriority` 或其名称；``None`` = normal），原样转交 App，App 的调用队列
        先交互、后后台（第 16 项 P6）；未知名称抛 ``ValueError``。
        """
        req = ffi.CallRequest(
            name=name,
            arguments_json=None if arguments is None else json.dumps(arguments, ensure_ascii=False),
            instance_id=instance_id,
            timeout_ms=_millis(timeout),
            call_id=call_id,
            session=session,
            idempotency_key=idempotency_key,
            priority=None if priority is None else _enum(CallPriority, priority, "priority"),
        )
        if on_progress is None:
            out = await self._inner.call_tool(req)
        else:
            listener = _ProgressAdapter(on_progress, asyncio.get_running_loop())
            out = await self._inner.call_tool_with_progress(req, listener)
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
        )

    def call_tool_sync(self, name: str, arguments: dict[str, Any] | None = None, **kwargs: Any) -> CallResult:
        """:meth:`call_tool` 的阻塞版本（不要在事件循环线程上调用）。"""
        return self._run_sync(self.call_tool(name, arguments, **kwargs))

    def cancel_call(self, call_id: str) -> None:
        self._inner.cancel_call(call_id)

    async def read_resource(self, uri: str) -> ResourceContent:
        return await self._inner.read_resource(uri)

    def subscribe(self, uri: str) -> None:
        self._inner.subscribe(uri)

    def unsubscribe(self, uri: str) -> None:
        self._inner.unsubscribe(uri)

    # -- LLM 工具格式 -------------------------------------------------------------

    def export_tools_json(
        self,
        format: FormatLike,
        *,
        apps: list[str] | None = None,
        max_risk: RiskLike | None = None,
        only_available: bool = False,
        include_builtin: bool = True,
        session: str | None = None,
    ) -> str:
        """导出工具定义（JSON 文本）。``session`` 与 :meth:`dispatch` 的会话对应（渐进暴露按会话计算）。"""
        return self._inner.export_tools(
            parse_format(format), _filter(apps, max_risk, only_available, include_builtin, session)
        )

    def export_tools(self, format: FormatLike, **filter: Any) -> Any:
        """导出工具定义（已解析），直接放进 LLM 请求的 ``tools``。

        Gemini 格式为 ``{"functionDeclarations": [...]}``，其余为列表。
        """
        return json.loads(self.export_tools_json(format, **filter))

    async def dispatch(self, format: FormatLike, tool_call: Any, session: str | None = None) -> Any:
        """执行模型发出的一个工具调用（dict，或该格式的 JSON 文本），返回应回填给模型的 dict。

        例如 Anthropic：``{"type": "tool_use", "id", "name", "input"}`` →
        ``{"type": "tool_result", "tool_use_id", "content", "is_error"?}``。
        """
        text = tool_call if isinstance(tool_call, str) else json.dumps(tool_call, ensure_ascii=False)
        return json.loads(await self._inner.dispatch(parse_format(format), text, session))

    def dispatch_sync(self, format: FormatLike, tool_call: Any, session: str | None = None) -> Any:
        """:meth:`dispatch` 的阻塞版本（不要在事件循环线程上调用）。"""
        return self._run_sync(self.dispatch(format, tool_call, session))

    def reset_session(self, session: str | None = None) -> None:
        """清除会话状态（首次接触总览、会话内 ``apps.select``）。"""
        self._inner.reset_session(session)

    async def serve_http(self, addr: str, allow_remote: bool = False) -> str:
        """同时以 MCP Streamable HTTP 对外提供，返回实际地址。"""
        return await self._inner.serve_http(addr, allow_remote)

    # -- 回调 -------------------------------------------------------------------

    def set_approval_handler(
        self,
        handler: Callable[[ApprovalRequest], bool | Awaitable[bool]],
        *,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatcher: Dispatcher | None = None,
    ) -> None:
        """调用确认（风险不低于 ``approval_min_risk`` 时询问）。返回 False 或抛出异常 → ``USER_REJECTED``。

        同步 handler 默认在线程池中执行（可阻塞等待用户），传 ``dispatcher`` 则切到 UI 线程；
        async handler 在 ``loop``（缺省为调用本方法时正在运行的循环）上执行。
        """
        self._inner.set_approval_handler(_ApprovalAdapter(handler, loop or _running_loop(), dispatcher))

    def set_pairing_handler(
        self,
        handler: Callable[[PairingRequest], bool | Awaitable[bool]],
        *,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatcher: Dispatcher | None = None,
    ) -> None:
        """App 配对确认。返回 False 或抛出异常 → 拒绝。执行上下文同 :meth:`set_approval_handler`。"""
        self._inner.set_pairing_handler(_PairingAdapter(handler, loop or _running_loop(), dispatcher))

    def set_waker(
        self,
        handler: Callable[[WakeRequest], None | Awaitable[None]] | None,
        *,
        loop: asyncio.AbstractEventLoop | None = None,
        dispatcher: Dispatcher | None = None,
    ) -> None:
        """自定义唤醒（如 Android 厂商发送显式广播），替换默认的系统唤醒实现；``None`` 恢复默认。

        正常返回表示已发出激活，Hub 随后等待 App 回连（``wake_timeout_ms``）；抛出 :class:`WakeFailed`
        以指定类别（如 ``APP_NOT_INSTALLED``）结束调用，其他异常按 ``LAUNCH_FAILED``。执行上下文同
        :meth:`set_approval_handler`。
        """
        self._inner.set_waker(
            None if handler is None else _WakerAdapter(handler, loop or _running_loop(), dispatcher)
        )

    def on_event(
        self, callback: Callable[[HubEvent], None], *, dispatcher: Dispatcher | None = None
    ) -> Callable[[], None]:
        """订阅事件，返回取消订阅函数。回调默认在 Hub 分发线程上执行，须尽快返回。"""
        cb = callback if dispatcher is None else (lambda e: dispatcher(lambda: callback(e)))
        with self._subs_lock:
            self._subs.append(cb)

        def unsubscribe() -> None:
            with self._subs_lock:
                if cb in self._subs:
                    self._subs.remove(cb)

        return unsubscribe

    def on_lagged(self, callback: Callable[[int], None]) -> Callable[[], None]:
        """事件处理过慢被跳过时回调（参数为跳过的数量）；收到后应重新拉取 ``apps()`` / ``tools()``。"""
        with self._subs_lock:
            self._lagged_subs.append(callback)

        def unsubscribe() -> None:
            with self._subs_lock:
                if callback in self._lagged_subs:
                    self._lagged_subs.remove(callback)

        return unsubscribe

    def events(self, loop: asyncio.AbstractEventLoop | None = None) -> EventStream:
        """事件的 async 迭代器（需要在事件循环中调用，或传 ``loop``）。"""
        return EventStream(self, loop or asyncio.get_running_loop())

    # -- 内部 -------------------------------------------------------------------

    def _emit(self, event: HubEvent) -> None:
        with self._subs_lock:
            subs = list(self._subs)
        for cb in subs:
            try:
                cb(event)
            except Exception:
                _log.exception("事件回调抛出异常")

    def _emit_lagged(self, skipped: int) -> None:
        with self._subs_lock:
            subs = list(self._lagged_subs)
        for cb in subs:
            try:
                cb(skipped)
            except Exception:
                _log.exception("lagged 回调抛出异常")

    @staticmethod
    def _run_sync(coro: Awaitable[Any]) -> Any:
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            pass
        else:
            if inspect.iscoroutine(coro):
                coro.close()
            raise RuntimeError("事件循环线程上请直接 await async 版本")
        return asyncio.run(_await(coro))


def _running_loop() -> asyncio.AbstractEventLoop | None:
    try:
        return asyncio.get_running_loop()
    except RuntimeError:
        return None
