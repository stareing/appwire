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
typealias Activation = dev.appmcp.hub.ffi.Activation
typealias Availability = dev.appmcp.hub.ffi.Availability
typealias Visibility = dev.appmcp.hub.ffi.Visibility
typealias AppKind = dev.appmcp.hub.ffi.AppKind
typealias AppInfo = dev.appmcp.hub.ffi.AppInfo
typealias InstanceInfo = dev.appmcp.hub.ffi.InstanceInfo
typealias AppOverviewInfo = dev.appmcp.hub.ffi.AppOverviewInfo
typealias HubTool = dev.appmcp.hub.ffi.HubTool
typealias HubResource = dev.appmcp.hub.ffi.HubResource
typealias ResourceContent = dev.appmcp.hub.ffi.ResourceContent
typealias ToolErrorInfo = dev.appmcp.hub.ffi.ToolErrorInfo
typealias ApprovalRequest = dev.appmcp.hub.ffi.ApprovalRequest
typealias PairingRequest = dev.appmcp.hub.ffi.PairingRequest
typealias WakeKind = dev.appmcp.hub.ffi.WakeKind
typealias WakeDescriptor = dev.appmcp.hub.ffi.WakeDescriptor

/** 工具暴露方式（`AUTO` / `PROGRESSIVE` / `ALL`，spec/hub-api.md 3.7）。 */
typealias ToolExposure = dev.appmcp.hub.ffi.ToolExposure

/** 唤醒器配置；变体需通过 `dev.appmcp.hub.ffi.WakerConfig.System` / `.Disabled` / `.Exec(argv)` 访问（typealias 不能访问嵌套类）。 */
typealias WakerConfig = dev.appmcp.hub.ffi.WakerConfig

/** 交给 [Hub.setWaker] 的唤醒请求（`appId`、`instanceId`、`descriptor`、`token`、`activationArg`）。 */
typealias WakeRequest = dev.appmcp.hub.ffi.WakeRequest

/**
 * Hub 事件（sealed class）。休眠相关：`HubEvent.AppDormant(appId, instanceId)`、
 * `HubEvent.AppWaking(appId, instanceId?)`（`null` = 冷启动）。未单独映射的新事件以 `HubEvent.Other(kind, json)` 送达。
 * 子类需通过 `dev.appmcp.hub.ffi.HubEvent.AppConnected` 等访问（typealias 不能访问嵌套类）。
 */
typealias HubEvent = dev.appmcp.hub.ffi.HubEvent

/** Hub 操作错误的基类（`Tool`、`InvalidJson`、`InvalidConfig`、`Io`、`Shutdown`）。 */
typealias HubException = dev.appmcp.hub.ffi.HubException

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

        /** 解析格式名：`mcp`、`openai-chat`（`openai`）、`openai-responses`、`anthropic`、`gemini`。 */
        fun parseFormat(name: String): ToolFormat = dev.appmcp.hub.ffi.parseToolFormat(name)

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

    /** App 连接服务的实际地址（端口 0 时为随机端口）；未开启时为 `null`。 */
    val wsAddr: String? get() = inner.wsAddr()

    /** 本地 IPC 连接服务的端点（`unix:…` / `pipe:…`，可直接作为 App 端 SDK 的 `hostUrl`）；未开启时为 `null`。 */
    val ipcEndpoint: String? get() = inner.ipcEndpoint()

    fun apps(): List<AppInfo> = inner.apps()

    fun tools(filter: ToolFilter = ToolFilter()): List<HubTool> = inner.tools(filter)

    fun resources(): List<HubResource> = inner.resources()

    fun overview(appId: String): AppOverviewInfo? = inner.overview(appId)

    /** 设置全局默认实例（`null` 恢复按规则路由）。 */
    fun selectInstance(appId: String, instanceId: String?) = inner.selectInstance(appId, instanceId)

    /**
     * 调用工具（全名 `<appId>.<tool>`）。工具层面的失败（用户拒绝、超时、App 报错……）放在
     * [CallResult.error]；名称无法解析时抛出 [HubException]。协程取消时自动取消调用。
     */
    suspend fun callTool(
        name: String,
        arguments: JsonObject? = null,
        instanceId: String? = null,
        timeout: Duration? = null,
        session: String? = null,
        callId: String? = null,
    ): CallResult {
        val out = inner.callTool(
            dev.appmcp.hub.ffi.CallRequest(
                name = name,
                argumentsJson = arguments?.toString(),
                instanceId = instanceId,
                timeoutMs = timeout?.inWholeMilliseconds?.coerceAtLeast(0)?.toULong(),
                callId = callId,
                session = session,
            ),
        )
        return CallResult(
            callId = out.callId,
            data = parseJson(out.dataJson),
            error = out.error,
            stateHints = out.stateHints,
            instanceId = out.instanceId,
            overview = out.overview,
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

    /** 停止 Hub（断开所有 App、结束后台任务）。可重复调用。 */
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        callbackScope.cancel()
        runCatching { inner.setEventListener(null) }
        runCatching { inner.shutdown() }
        inner.close()
    }
}
