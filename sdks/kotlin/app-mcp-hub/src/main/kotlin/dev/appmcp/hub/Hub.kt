package dev.appmcp.hub

import dev.appmcp.hub.ffi.AppMcpHub as FfiHub
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.CoroutineContext
import kotlin.coroutines.EmptyCoroutineContext
import kotlin.time.Duration

// 直接复用 uniffi 生成的数据类型。
typealias HubConfig = dev.appmcp.hub.ffi.HubConfig
typealias UpstreamSpec = dev.appmcp.hub.ffi.UpstreamSpec
typealias ToolFilter = dev.appmcp.hub.ffi.ToolFilter
typealias ToolFormat = dev.appmcp.hub.ffi.ToolFormat
typealias Risk = dev.appmcp.hub.ffi.Risk
/** 调用优先级（[Hub.callTool] 的 `priority`，第 16 项 P6，spec/hub-api.md 3.15）：`INTERACTIVE` / `NORMAL` / `BACKGROUND`。 */
typealias CallPriority = dev.appmcp.hub.ffi.CallPriority
typealias Activation = dev.appmcp.hub.ffi.Activation
typealias Availability = dev.appmcp.hub.ffi.Availability
typealias Visibility = dev.appmcp.hub.ffi.Visibility
typealias AppKind = dev.appmcp.hub.ffi.AppKind
typealias AppInfo = dev.appmcp.hub.ffi.AppInfo
typealias InstanceInfo = dev.appmcp.hub.ffi.InstanceInfo
typealias AppOverviewInfo = dev.appmcp.hub.ffi.AppOverviewInfo
typealias HubTool = dev.appmcp.hub.ffi.HubTool
/** App 工具对界面的依赖（`APP` / `VIEW`，spec/protocol.md 3.4）：[HubTool.surface]；内置与上游工具为 `null`。 */
typealias ToolSurface = dev.appmcp.hub.ffi.ToolSurface
typealias HubResource = dev.appmcp.hub.ffi.HubResource
typealias ResourceContent = dev.appmcp.hub.ffi.ResourceContent
/** 调用进度（`progress`、`total`、`message`；[Hub.callTool] 的 `onProgress`，spec/hub-api.md 3.12）。 */
typealias ProgressUpdate = dev.appmcp.hub.ffi.ProgressUpdate
typealias ToolErrorInfo = dev.appmcp.hub.ffi.ToolErrorInfo
typealias ApprovalRequest = dev.appmcp.hub.ffi.ApprovalRequest
typealias PairingRequest = dev.appmcp.hub.ffi.PairingRequest
typealias WakeKind = dev.appmcp.hub.ffi.WakeKind
typealias WakeDescriptor = dev.appmcp.hub.ffi.WakeDescriptor

// 运行状态（[Hub.status]，spec/hub-api.md 3.9）。
typealias HubStatus = dev.appmcp.hub.ffi.HubStatus
typealias AuthStatus = dev.appmcp.hub.ffi.AuthStatus
typealias AppStatus = dev.appmcp.hub.ffi.AppStatus
typealias AppState = dev.appmcp.hub.ffi.AppState
typealias InstanceStatus = dev.appmcp.hub.ffi.InstanceStatus
typealias InstanceState = dev.appmcp.hub.ffi.InstanceState
typealias LastError = dev.appmcp.hub.ffi.LastError
typealias DiagnosticReport = dev.appmcp.hub.ffi.DiagnosticReport
/** 休眠记录持久化状态（[HubStatus.dormantStore]，配置了 [HubConfig.stateDir] 时）。 */
typealias DormantStoreStatus = dev.appmcp.hub.ffi.DormantStoreStatus
typealias StoreIssue = dev.appmcp.hub.ffi.StoreIssue
/** Agent 任务（[HubStatus.tasks]，spec/hub-api.md 3.6）：调用方的跨请求状态。 */
typealias AgentTaskStatus = dev.appmcp.hub.ffi.AgentTaskStatus
/** 调用方的种类：legacy MCP 会话 / 无会话 MCP 请求的主体 / Hub API 会话。 */
typealias CallerKind = dev.appmcp.hub.ffi.CallerKind
typealias TaskSelectionStatus = dev.appmcp.hub.ffi.TaskSelectionStatus
typealias TaskLeaseStatus = dev.appmcp.hub.ffi.TaskLeaseStatus
/** 未到期的对象锁（`HubStatus.locks`，spec/hub-api.md 3.6「对象锁」）。 */
typealias LockStatus = dev.appmcp.hub.ffi.LockStatus

// 资源保护与工具声明（spec/hub-api.md 3.11）。
/** 限流与大小上限（[HubConfig.limits]；[HubStatus.limits] 为全部字段给出的生效值）。为空的字段取默认值。 */
typealias LimitsConfig = dev.appmcp.hub.ffi.LimitsConfig
/** 结果与其 `outputSchema` 不符时的处理：`OFF` / `LOG`（默认）/ `REJECT`。 */
typealias OutputValidation = dev.appmcp.hub.ffi.OutputValidation
/** 标准 MCP 工具注解（[HubTool.annotations]、[ApprovalRequest.annotations]）。 */
typealias ToolAnnotations = dev.appmcp.hub.ffi.ToolAnnotations
/** 内容标注（MCP 内容注解：audience、priority、lastModified）。 */
typealias ContentAnnotations = dev.appmcp.hub.ffi.ContentAnnotations
typealias Audience = dev.appmcp.hub.ffi.Audience
/** 调用结果的业务状态：`DONE` / `PENDING` / `PARTIAL` / `NOOP`。 */
typealias ResultStatus = dev.appmcp.hub.ffi.ResultStatus
/** 一个工具的声明（[AppStatus.tools]）。 */
typealias ToolDeclaration = dev.appmcp.hub.ffi.ToolDeclaration

// 策略挂点（spec/hub-api.md 3.13）。
/** 策略规则集（[HubConfig.policy]、[Hub.setPolicy]）；空规则集 = 不做任何限制（默认）。 */
typealias PolicyConfig = dev.appmcp.hub.ffi.PolicyConfig
/** 一条策略规则：`HIDE`（不出现在任何列表中、调用为 `TOOL_NOT_FOUND`）或 `DENY`（以 `POLICY_DENIED` 拒绝）。 */
typealias PolicyRule = dev.appmcp.hub.ffi.PolicyRule
typealias PolicyAction = dev.appmcp.hub.ffi.PolicyAction
/** 执行点；规则的 `hooks` 只能写 `CALL` / `WAKE`。 */
typealias PolicyHook = dev.appmcp.hub.ffi.PolicyHook
/** 按 App 声明的 MCP 注解匹配（给出的每一项都相等才命中）。 */
typealias AnnotationMatch = dev.appmcp.hub.ffi.AnnotationMatch
/** 策略状态（[Hub.policy]、[HubStatus.policy]）：生效的规则、命中次数与最近的加载错误。 */
typealias PolicyStatus = dev.appmcp.hub.ffi.PolicyStatus
typealias PolicyRuleStatus = dev.appmcp.hub.ffi.PolicyRuleStatus
typealias PolicyLoadError = dev.appmcp.hub.ffi.PolicyLoadError
/** Agent 访问令牌（[HubConfig.agents]、[Hub.setAgents]，spec/hub-api.md 3.6「Agent 身份」）。 */
typealias AgentCredential = dev.appmcp.hub.ffi.AgentCredential

/** 工具暴露方式（`AUTO` / `PROGRESSIVE` / `ALL`，spec/hub-api.md 3.7）。 */
typealias ToolExposure = dev.appmcp.hub.ffi.ToolExposure
/** MCP 出口协商的协议版本范围（`AUTO` / `LEGACY_ONLY`，spec/hub-api.md 3.6「协议版本」）。 */
typealias McpProtocolMode = dev.appmcp.hub.ffi.McpProtocolMode

/** 唤醒器配置；变体需通过 `dev.appmcp.hub.ffi.WakerConfig.System` / `.Disabled` / `.Exec(argv)` 访问（typealias 不能访问嵌套类）。 */
typealias WakerConfig = dev.appmcp.hub.ffi.WakerConfig

/** 交给 [Hub.setWaker] 的唤醒请求（`appId`、`instanceId`、`descriptor`、`token`、`activationArg`）。 */
typealias WakeRequest = dev.appmcp.hub.ffi.WakeRequest

/**
 * Hub 事件（sealed class）。休眠相关：`HubEvent.AppDormant(appId, instanceId)`、
 * `HubEvent.AppWaking(appId, instanceId?)`（`null` = 冷启动）；SDK 诊断上报：
 * `HubEvent.AppDiagnostic(appId, instanceId, code, message, count)`（spec/protocol.md 10.2）。
 * 未单独映射的新事件以 `HubEvent.Other(kind, json)` 送达。
 * 子类需通过 `dev.appmcp.hub.ffi.HubEvent.AppConnected` 等访问（typealias 不能访问嵌套类）。
 */
typealias HubEvent = dev.appmcp.hub.ffi.HubEvent

/**
 * Hub 操作错误的基类（`Tool`、`InvalidJson`、`InvalidConfig`、`Io`、`Shutdown`、`Unsupported`）。
 * `Unsupported`：本构建未包含所需能力（Android 精简库上 `mcpHttp = true`、`upstreams` 非空、`serveHttp`），
 * `detail` 说明缺哪个 cargo feature（spec/hub-api.md 3.10）；重试无效。
 * 嵌套类别不能经 typealias 访问：按类别捕获时用 `dev.appmcp.hub.ffi.HubException.Unsupported` 等。
 */
typealias HubException = dev.appmcp.hub.ffi.HubException

// 按名寻址（spec/hub-api.md 3.16、spec/naming.md 4.2）。
/** 宿主实现的名字服务（Android：`dev.appmcp.hub.android.AndroidNameService`）；[Hub.startWithNameService]。 */
typealias HubNameService = dev.appmcp.hub.ffi.HubNameService
/** 名字服务发现的一个 App（appId、可激活、平台名字、清单 JSON）。 */
typealias NamedApp = dev.appmcp.hub.ffi.NamedApp
/**
 * 拨号结果；变体经 `dev.appmcp.hub.ffi.DialOutcome.Channel` / `.Failed` / `.Blocked` 访问。`Blocked`：目标已安装但系统拒绝绑定
 * （关联启动 / 自启动管控），Hub 以 `USER_ACTION_REQUIRED`（`reason: "os-permission"`）结束调用。
 */
typealias DialOutcome = dev.appmcp.hub.ffi.DialOutcome

/** 原生库编译进的可选能力（[Hub.features]；spec/hub-api.md 3.10）。 */
typealias HubFeatures = dev.appmcp.hub.ffi.HubFeatures

internal val HubJson = Json { ignoreUnknownKeys = true }

private fun parseJson(text: String?): JsonElement? = text?.let { HubJson.parseToJsonElement(it) }

/** 工具调用以错误结束（[CallResult.getOrThrow]）。[kind] 为协议错误类别，如 `USER_REJECTED`。 */
class ToolException(val kind: String, override val message: String, val details: JsonElement?) : Exception(message)

/** 一次工具调用的结果。 */
data class CallResult(
    val callId: String,
    /** 成功时的结果数据。 */
    val data: JsonElement?,
    /** 失败时的错误（类别见 `kind`，如 `USER_REJECTED`、`TIMEOUT`）。 */
    val error: ToolErrorInfo?,
    val stateHints: List<String>,
    /** 实际执行的实例。 */
    val instanceId: String?,
    /** 本会话首次接触该 App 时附带的总览。 */
    val overview: AppOverviewInfo?,
    /** App 声明的业务状态（缺省 `DONE`；`PENDING` 时后续状态见 [stateResource]）。 */
    val status: ResultStatus = ResultStatus.DONE,
    /** `PENDING` 时可读取后续状态的资源 URI（`app-mcp://<appId>/<名>`）。 */
    val stateResource: String? = null,
    /** App 给出的一句结论。 */
    val summary: String? = null,
    /** App 对结果内容的标注，原样。 */
    val annotations: ContentAnnotations? = null,
    /** App 在后台、Hub 改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则为 `null`。 */
    val routedTo: String? = null,
    /** 从 Hub 收到调用到得出结果的毫秒数（spec/hub-api.md 3.15 `dev.appwire/durationMs`）。 */
    val durationMs: Long = 0,
    /** 本次调用是否唤醒了 App（`dev.appwire/woke`）。 */
    val woke: Boolean = false,
) {
    val isError: Boolean get() = error != null

    /** 成功时返回数据，失败时抛出 [ToolException]。 */
    fun getOrThrow(): JsonElement? {
        val e = error ?: return data
        throw ToolException(e.kind, e.message, parseJson(e.detailsJson))
    }
}

/**
 * 在 [Hub.setWaker] 的 handler 中抛出，以指定的协议错误类别（如 `APP_NOT_INSTALLED`）结束调用；
 * 其他异常按 `LAUNCH_FAILED`。
 */
class WakeFailedException(val kind: String, override val message: String) : Exception(message)

/** 无已连接实例、但有休眠实例（调用其工具时 Hub 先唤醒）。 */
val AppInfo.isDormant: Boolean get() = !connected && dormantInstances.isNotEmpty()

/** 工具参数 schema。 */
val HubTool.inputSchema: JsonElement get() = HubJson.parseToJsonElement(inputSchemaJson)

/** App 声明的结果 schema；未声明时为 `null`。 */
val HubTool.outputSchema: JsonElement? get() = parseJson(outputSchemaJson)

/** 待审批调用的参数。 */
val ApprovalRequest.arguments: JsonElement get() = HubJson.parseToJsonElement(argumentsJson)

/** 错误详情。 */
val ToolErrorInfo.details: JsonElement? get() = parseJson(detailsJson)

/**
 * 嵌入式 Hub（Agent 端）：连接本机所有 App，列工具、调用、导出 / 分派 LLM 工具格式、事件、审批。
 *
 * ```kotlin
 * val hub = Hub.start(HubConfig(approvalMinRisk = Risk.DESTRUCTIVE))
 * hub.setApprovalHandler { req -> showConfirmDialog(req) }       // suspend，可切到 Main
 * scope.launch { hub.events.collect { e -> refreshUi(e) } }
 * val tools = hub.exportTools(ToolFormat.OPEN_AI_CHAT)           // 交给自有 LLM
 * val reply = hub.dispatch(ToolFormat.OPEN_AI_CHAT, toolCallJson) // 回填给 LLM
 * hub.close()
 * ```
 *
 * 线程：所有方法线程安全；`suspend` 方法不阻塞调用线程。事件在原生分发线程上发出到 [events]，
 * 由收集方所在的调度器处理。审批 / 配对 / 唤醒 handler 是 `suspend` 函数：原生层以同步回调 + 完成句柄
 * 交给本类，本类在自己的协程作用域（默认 `Dispatchers.Default`，可按 handler 指定 `context`，如
 * `Dispatchers.Main`）里执行 handler，再经句柄回传结果；[close] 时取消未完成的 handler（视为拒绝）。
 */
class Hub private constructor(private val inner: FfiHub) : AutoCloseable {
    companion object {
        /** 启动 Hub（绑定 App 连接服务并启动后台任务）。 */
        fun start(config: HubConfig = HubConfig()): Hub = Hub(FfiHub.start(config))

        /**
         * 启动 Hub 并以宿主实现的名字服务按名寻址（spec/hub-api.md 3.16）：启动时 `discover` 一次（清单中的工具随即可列出，
         * 不启动 App）；调用到达而 App 没有活连接时 `dial`，通道宽限（[HubConfig.channelGraceMs]）后关闭并 `release`。
         * 安装 / 卸载变化用 [nameServiceInstalled] / [nameServiceRemoved] 推送。非 Unix 平台抛 `HubException.Unsupported`。
         */
        fun startWithNameService(config: HubConfig, kind: String, service: HubNameService): Hub =
            Hub(FfiHub.startWithNameService(config, kind, service))

        /** 解析格式名：`mcp`、`openai-chat`（`openai`）、`openai-responses`、`anthropic`、`gemini`。 */
        fun parseFormat(name: String): ToolFormat = dev.appmcp.hub.ffi.parseToolFormat(name)

        /**
         * 已加载的原生库编译进的可选能力（`mcpServer` / `upstream` / `schemaValidation`）。启动前据此检查：如独立 Hub App
         * 需要 `mcpServer`，Android 默认精简库（`generate.sh --android`）不含。
         */
        fun features(): HubFeatures = dev.appmcp.hub.ffi.hubFeatures()

        /** Hub 日志输出到 stderr（`RUST_LOG` 语法）。只有第一次调用生效。 */
        fun initLogging(filter: String? = null): Boolean = dev.appmcp.hub.ffi.initLogging(filter)
    }

    private val closed = AtomicBoolean(false)

    /** 执行审批 / 配对 / 唤醒 handler 的作用域；[close] 时取消。 */
    private val callbackScope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineName("app-mcp-hub-callback"))

    private val _events = MutableSharedFlow<HubEvent>(
        extraBufferCapacity = 1024,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    private val _lagged = MutableSharedFlow<Long>(extraBufferCapacity = 16, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** Hub 事件（热流：只收到订阅之后的事件）。 */
    val events: SharedFlow<HubEvent> = _events.asSharedFlow()

    /** 事件处理过慢被跳过的数量；收到后应重新拉取 [apps] / [tools]。 */
    val lagged: SharedFlow<Long> = _lagged.asSharedFlow()

    init {
        inner.setEventListener(object : dev.appmcp.hub.ffi.HubEventListener {
            override fun onEvent(event: HubEvent) {
                _events.tryEmit(event)
            }

            override fun onLagged(skipped: ULong) {
                _lagged.tryEmit(skipped.toLong())
            }
        })
    }

    /** HTTP 服务（`/app`、`/healthz`）的实际地址（端口 0 时为随机端口；App 端点为 `ws://<地址>/app`）；未开启时为 `null`。 */
    val listenAddr: String? get() = inner.listenAddr()

    /** 本地 IPC 连接服务的端点（`unix:…` / `pipe:…`，可直接作为 App 端 SDK 的 `hostUrl`）；未开启时为 `null`。 */
    val ipcEndpoint: String? get() = inner.ipcEndpoint()

    fun apps(): List<AppInfo> = inner.apps()

    fun tools(filter: ToolFilter = ToolFilter()): List<HubTool> = inner.tools(filter)

    fun resources(): List<HubResource> = inner.resources()

    fun overview(appId: String): AppOverviewInfo? = inner.overview(appId)

    /**
     * 运行状态（spec/hub-api.md 3.9，与 `GET /status` 相同）：身份、监听位置、令牌策略、各 App 与实例的状态
     * （实例带 `info.connectionId`）、最近错误、最近的 SDK 诊断上报。已停止时抛出 `HubException.Shutdown`。
     */
    fun status(): HubStatus = inner.status()

    /** 生效的策略规则、各规则命中次数与最近的加载错误（spec/hub-api.md 3.13）。已停止时抛出 `HubException.Shutdown`。 */
    fun policy(): PolicyStatus = inner.policy()

    /**
     * 替换策略规则集（命中计数清零；`PolicyConfig()` 清空）。规则不合法时抛出 `HubException.Tool`
     * （`kind = "INVALID_INPUT"`），之前的规则继续生效，原因记入 [policy] 的 `lastError`。
     */
    fun setPolicy(policy: PolicyConfig) = inner.setPolicy(policy)

    /**
     * 替换 Agent 登记（空列表清空），只影响之后到达的 MCP 请求。不合法时抛出 `HubException.Tool`
     * （`kind = "INVALID_INPUT"`），之前的登记继续生效。
     */
    fun setAgents(agents: List<AgentCredential>) = inner.setAgents(agents)

    /** 设置全局默认实例（`null` 恢复按规则路由）。 */
    fun selectInstance(appId: String, instanceId: String?) = inner.selectInstance(appId, instanceId)

    /**
     * 调用工具（全名 `<appId>.<tool>`）。工具层面的失败（用户拒绝、超时、App 报错……）放在
     * [CallResult.error]；名称无法解析时抛出 [HubException]。协程取消时自动取消调用。
     *
     * @param idempotencyKey Agent 的幂等键（1..=256 个字符），原样转交 App（spec/hub-api.md 3.15）；不合法时
     *   [CallResult.error] 为 `INVALID_INPUT`。
     * @param priority 调用优先级（为空 = `NORMAL`），原样转交 App，App 的调用队列先交互、后后台（第 16 项 P6）。
     * @param onProgress 接收调用进度（Hub 合并后，spec/hub-api.md 3.12）：在 Hub 的进度线程上按顺序同步调用，须尽快返回
     *   （需要时自行切换线程）；全部回调都在本函数返回之前完成。回调抛出的异常被忽略。为空时不接收进度。
     */
    suspend fun callTool(
        name: String,
        arguments: JsonObject? = null,
        instanceId: String? = null,
        timeout: Duration? = null,
        session: String? = null,
        callId: String? = null,
        idempotencyKey: String? = null,
        priority: CallPriority? = null,
        onProgress: ((ProgressUpdate) -> Unit)? = null,
    ): CallResult {
        val request = dev.appmcp.hub.ffi.CallRequest(
            name = name,
            argumentsJson = arguments?.toString(),
            instanceId = instanceId,
            timeoutMs = timeout?.inWholeMilliseconds?.coerceAtLeast(0)?.toULong(),
            callId = callId,
            session = session,
            idempotencyKey = idempotencyKey,
            priority = priority,
        )
        val out = if (onProgress == null) {
            inner.callTool(request)
        } else {
            inner.callToolWithProgress(request, object : dev.appmcp.hub.ffi.ProgressListener {
                override fun onProgress(update: ProgressUpdate) {
                    try {
                        onProgress(update)
                    } catch (_: Throwable) {
                        // @why 回调异常不影响调用：在原生线程上抛出会被 uniffi 视为 panic（被兜底后同样忽略）。
                    }
                }
            })
        }
        return CallResult(
            callId = out.callId,
            data = parseJson(out.dataJson),
            error = out.error,
            stateHints = out.stateHints,
            instanceId = out.instanceId,
            overview = out.overview,
            status = out.status,
            stateResource = out.stateResource,
            summary = out.summary,
            annotations = out.annotations,
            routedTo = out.routedTo,
            durationMs = out.durationMs.toLong(),
            woke = out.woke,
        )
    }

    fun cancelCall(callId: String) = inner.cancelCall(callId)

    suspend fun readResource(uri: String): ResourceContent = inner.readResource(uri)

    fun subscribe(uri: String) = inner.subscribe(uri)

    fun unsubscribe(uri: String) = inner.unsubscribe(uri)

    /** 导出工具定义（JSON 文本），直接放进 LLM 请求的 `tools`。 */
    fun exportToolsJson(format: ToolFormat, filter: ToolFilter = ToolFilter()): String =
        inner.exportTools(format, filter)

    /** 导出工具定义（已解析）。 */
    fun exportTools(format: ToolFormat, filter: ToolFilter = ToolFilter()): JsonElement =
        HubJson.parseToJsonElement(exportToolsJson(format, filter))

    /** 执行模型发出的一个工具调用（该格式的 JSON 文本），返回应回填给模型的 JSON 文本。 */
    suspend fun dispatchJson(format: ToolFormat, toolCallJson: String, session: String? = null): String =
        inner.dispatch(format, toolCallJson, session)

    /** 执行模型发出的一个工具调用（已解析的 JSON），返回应回填给模型的 JSON。 */
    suspend fun dispatch(format: ToolFormat, toolCall: JsonElement, session: String? = null): JsonElement =
        HubJson.parseToJsonElement(dispatchJson(format, toolCall.toString(), session))

    /** 清除会话状态（首次接触总览、会话内 `apps.select`）。 */
    fun resetSession(session: String? = null) = inner.resetSession(session)

    /** 同时以 MCP Streamable HTTP 对外提供，返回实际地址。 */
    suspend fun serveHttp(addr: String, allowRemote: Boolean = false): String = inner.serveHttp(addr, allowRemote)

    /**
     * 调用确认（风险不低于 `HubConfig.approvalMinRisk` 时询问）。返回 `false` 或抛出异常 →
     * `USER_REJECTED`。handler 在 [context]（缺省 `Dispatchers.Default`）中执行，可以挂起等待用户，
     * 例如传 `Dispatchers.Main` 直接弹确认框。
     */
    fun setApprovalHandler(
        context: CoroutineContext = EmptyCoroutineContext,
        handler: suspend (ApprovalRequest) -> Boolean,
    ) {
        inner.setApprovalHandler(object : dev.appmcp.hub.ffi.ApprovalHandler {
            override fun onRequest(request: ApprovalRequest, responder: dev.appmcp.hub.ffi.ApprovalResponder) {
                // 原生线程上立即返回；结果经句柄回传。协程结束（含未启动即被取消）后释放句柄，未完成即视为拒绝。
                callbackScope.launch(context) {
                    responder.complete(runCatching { handler(request) }.getOrDefault(false))
                }.invokeOnCompletion { responder.close() }
            }
        })
    }

    /** App 配对确认。返回 `false` 或抛出异常 → 拒绝。执行上下文同 [setApprovalHandler]。 */
    fun setPairingHandler(
        context: CoroutineContext = EmptyCoroutineContext,
        handler: suspend (PairingRequest) -> Boolean,
    ) {
        inner.setPairingHandler(object : dev.appmcp.hub.ffi.PairingHandler {
            override fun onRequest(request: PairingRequest, responder: dev.appmcp.hub.ffi.PairingResponder) {
                callbackScope.launch(context) {
                    responder.complete(runCatching { handler(request) }.getOrDefault(false))
                }.invokeOnCompletion { responder.close() }
            }
        })
    }

    /**
     * 自定义唤醒（spec/hub-api.md 3.5），替换默认的系统唤醒实现；`null` 恢复默认。
     * Android 厂商在这里按 `request.descriptor`（`ANDROID_INTENT`，`target` 为组件名）发送显式广播，
     * 把 `request.activationArg` 交给 App（App 端 `handleWake`）。
     *
     * 正常返回表示已发出激活，Hub 随后等待 App 回连（`HubConfig.wakeTimeoutMs`）；抛出 [WakeFailedException]
     * 以指定类别结束调用，其他异常按 `LAUNCH_FAILED`。执行上下文同 [setApprovalHandler]。
     */
    fun setWaker(handler: (suspend (WakeRequest) -> Unit)?) = setWaker(EmptyCoroutineContext, handler)

    /** 同 [setWaker]，handler 在 [context] 中执行。 */
    fun setWaker(context: CoroutineContext, handler: (suspend (WakeRequest) -> Unit)?) {
        if (handler == null) {
            inner.setWaker(null)
            return
        }
        inner.setWaker(object : dev.appmcp.hub.ffi.HubWaker {
            override fun wake(request: WakeRequest, responder: dev.appmcp.hub.ffi.WakeResponder) {
                callbackScope.launch(context) {
                    try {
                        handler(request)
                        responder.succeed()
                    } catch (e: WakeFailedException) {
                        responder.fail(e.kind, e.message)
                    } catch (e: CancellationException) {
                        responder.fail("LAUNCH_FAILED", "唤醒已取消")
                        throw e
                    } catch (e: Exception) {
                        responder.fail("LAUNCH_FAILED", e.message ?: e.toString())
                    }
                }.invokeOnCompletion { responder.close() }
            }
        })
    }

    /** 名字服务推送：App 安装或更新（[startWithNameService] 启动的 Hub）。 */
    fun nameServiceInstalled(app: NamedApp) = inner.nameServiceInstalled(app)

    /** 名字服务推送：App 卸载。 */
    fun nameServiceRemoved(appId: String) = inner.nameServiceRemoved(appId)

    /**
     * 在交来的 fd（Unix 流式套接字，如 Android `bindService` 换得的 socketpair 一端）上提供 MCP，挂起直到对端关闭；
     * 帧与 stdio 相同（每行一条 JSON-RPC）。[fd] 的所有权转移给 Hub。本构建不含 `mcp-server` 时抛 `HubException.Unsupported`。
     */
    suspend fun serveMcpFd(fd: Int) = inner.serveMcpFd(fd)

    /** 停止 Hub（断开所有 App、结束后台任务）。可重复调用。 */
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        callbackScope.cancel()
        runCatching { inner.setEventListener(null) }
        runCatching { inner.shutdown() }
        inner.close()
    }
}
