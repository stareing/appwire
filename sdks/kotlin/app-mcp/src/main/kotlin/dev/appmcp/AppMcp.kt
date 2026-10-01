package dev.appmcp

import dev.appmcp.ffi.AppMcpClient
import dev.appmcp.ffi.AppMcpException
import dev.appmcp.ffi.Call
import dev.appmcp.ffi.CallResult
import dev.appmcp.ffi.CancelListener
import dev.appmcp.ffi.ClientListener
import dev.appmcp.ffi.Read
import dev.appmcp.ffi.ResourceReader
import dev.appmcp.ffi.ResourceSpec
import dev.appmcp.ffi.ToolHandler
import dev.appmcp.ffi.ToolSpec
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import kotlin.coroutines.EmptyCoroutineContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.serializer
import dev.appmcp.ffi.Resource as FfiResource
import dev.appmcp.ffi.Scope as FfiScope
import dev.appmcp.ffi.Tool as FfiTool

/** 工具 handler：在配置的调度器上以协程执行；调用被取消时协程被取消。 */
typealias ToolFunction = suspend (args: JsonObject, ctx: ToolContext) -> Any?

/** 资源读取函数。 */
typealias ResourceFunction = suspend () -> Any?

/** 一次工具调用的上下文。 */
class ToolContext internal constructor(
    val callId: String,
    val toolName: String,
    val arguments: JsonObject,
    private val call: Call? = null,
) {
    @Volatile
    var cancelReason: CancelReason? = null
        internal set

    val isCancelled: Boolean get() = cancelReason != null

    internal val stateHints = CopyOnWriteArrayList<String>()

    /** 声明调用后内容可能变化的资源，提示模型重新读取。 */
    fun addStateHint(resourceName: String) {
        stateHints += resourceName
    }

    /**
     * 延长持有：handler 返回后仍阻止自动休眠（handler 发起的后台长任务），直到返回的句柄被关闭。
     * 调用已结束（完成、取消）时抛 `AppMcpException.AlreadyCompleted`。
     */
    fun hold(): HoldHandle {
        val c = call ?: throw IllegalStateException("该上下文不关联原生调用")
        return HoldHandle(c.hold())
    }
}

/** 已注册的工具。 */
class ToolHandle internal constructor(private val inner: FfiTool, @Volatile private var spec: ToolSpec) {
    val name: String get() = inner.name()

    fun setEnabled(enabled: Boolean) = inner.setEnabled(enabled)

    /** 修改定义；为空的参数保持不变。 */
    fun update(
        description: String? = null,
        inputSchema: JsonObject? = null,
        risk: Risk? = null,
        title: String? = null,
        annotations: ToolAnnotations? = null,
        outputSchema: JsonObject? = null,
    ) {
        val next = spec.copy(
            description = description ?: spec.description,
            inputSchemaJson = inputSchema?.toString() ?: spec.inputSchemaJson,
            risk = risk ?: spec.risk,
            title = title ?: spec.title,
            annotations = annotations ?: spec.annotations,
            outputSchemaJson = outputSchema?.toString() ?: spec.outputSchemaJson,
        )
        inner.update(next)
        spec = next
    }

    /** 注销工具（幂等）。 */
    fun dispose() = inner.dispose()
}

/** 已注册的资源。 */
class ResourceHandle internal constructor(private val inner: FfiResource) {
    val name: String get() = inner.name()
    fun notifyChanged() = inner.notifyChanged()
    fun dispose() = inner.dispose()
}

/** 工具 / 资源 / 子作用域的注册入口，[AppMcp] 与 [Scope] 共用。 */
abstract class AppMcpRegistrar internal constructor() {
    internal abstract val owner: AppMcp
    internal abstract fun registerRaw(spec: ToolSpec, handler: ToolHandler): FfiTool
    internal abstract fun registerRaw(spec: ResourceSpec, reader: ResourceReader): FfiResource
    internal abstract fun createRaw(name: String): FfiScope

    /**
     * 注册工具。
     *
     * ```kotlin
     * client.tool(
     *     "cart.checkout", "结账", schema,
     *     annotations = ToolAnnotations(destructiveHint = true, openWorldHint = true),
     * ) { args, ctx ->
     *     checkout(args["coupon"]?.jsonPrimitive?.contentOrNull)
     * }
     * ```
     *
     * 返回值见 [anyToJson]；返回 [ToolResult] 可附带 stateHints、业务状态、摘要与内容标注。
     *
     * @param risk 旧写法，优先用 [annotations]。
     * @param annotations 标准 MCP 工具注解，原样转发给 Agent；为空时 Hub 按 [risk] 推导。
     * @param outputSchema 结果的 JSON Schema（MCP `outputSchema`）。
     */
    fun tool(
        name: String,
        description: String,
        inputSchema: JsonObject? = null,
        risk: Risk = Risk.WRITE,
        activation: Activation? = null,
        title: String? = null,
        enabled: Boolean = true,
        annotations: ToolAnnotations? = null,
        outputSchema: JsonObject? = null,
        handler: ToolFunction,
    ): ToolHandle {
        val spec = ToolSpec(
            name = name,
            description = description,
            inputSchemaJson = inputSchema?.toString(),
            risk = risk,
            activation = activation,
            title = title,
            enabled = enabled,
            annotations = annotations,
            outputSchemaJson = outputSchema?.toString(),
        )
        val o = owner
        val raw = registerRaw(spec, object : ToolHandler {
            override fun invoke(call: Call) = o.runTool(call, handler)
        })
        return ToolHandle(raw, spec)
    }

    /**
     * 带类型的工具：参数用 kotlinx.serialization 解码为 [A]（失败 → `INVALID_INPUT`），返回值按 [R] 编码。
     */
    inline fun <reified A, reified R> typedTool(
        name: String,
        description: String,
        inputSchema: JsonObject? = null,
        risk: Risk = Risk.WRITE,
        activation: Activation? = null,
        title: String? = null,
        enabled: Boolean = true,
        annotations: ToolAnnotations? = null,
        outputSchema: JsonObject? = null,
        noinline handler: suspend (args: A, ctx: ToolContext) -> R,
    ): ToolHandle = typedToolImpl(
        name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema,
        serializer<A>(), serializer<R>(), handler,
    )

    @PublishedApi
    internal fun <A, R> typedToolImpl(
        name: String,
        description: String,
        inputSchema: JsonObject?,
        risk: Risk,
        activation: Activation?,
        title: String?,
        enabled: Boolean,
        annotations: ToolAnnotations?,
        outputSchema: JsonObject?,
        argSerializer: KSerializer<A>,
        resultSerializer: KSerializer<R>,
        handler: suspend (A, ToolContext) -> R,
    ): ToolHandle = tool(
        name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema,
    ) { args, ctx ->
        val decoded = try {
            AppMcpJson.decodeFromJsonElement(argSerializer, args)
        } catch (e: SerializationException) {
            throw ToolCallException(ErrorKind.INVALID_INPUT, "参数解码失败：${e.message}")
        } catch (e: IllegalArgumentException) {
            throw ToolCallException(ErrorKind.INVALID_INPUT, "参数解码失败：${e.message}")
        }
        AppMcpJson.encodeToJsonElement(resultSerializer, handler(decoded, ctx))
    }

    /**
     * 注册资源；[reader] 的返回值见 [anyToJson]。
     *
     * @param realtime 需实时推送（spec/lifecycle.md 第 13 节 B3）：被订阅时阻止休眠、休眠中变化时回连推送。
     *   默认 false：订阅不阻止休眠，变化在下次连接时补发。
     */
    fun resource(
        name: String,
        description: String,
        mimeType: String? = null,
        realtime: Boolean = false,
        reader: ResourceFunction,
    ): ResourceHandle {
        val o = owner
        val raw = registerRaw(ResourceSpec(name, description, mimeType, realtime), object : ResourceReader {
            override fun read(read: Read) = o.runRead(read, reader)
        })
        return ResourceHandle(raw)
    }

    /** 创建子作用域；[Scope.dispose]（或 `use {}`）时注销其下全部工具与资源。 */
    fun scope(name: String): Scope = Scope(owner, createRaw(name))
}

/** 作用域。 */
class Scope internal constructor(override val owner: AppMcp, private val inner: FfiScope) :
    AppMcpRegistrar(), AutoCloseable {
    override fun registerRaw(spec: ToolSpec, handler: ToolHandler) = inner.registerTool(spec, handler)
    override fun registerRaw(spec: ResourceSpec, reader: ResourceReader) = inner.registerResource(spec, reader)
    override fun createRaw(name: String) = inner.createScope(name)

    fun dispose() = inner.dispose()
    override fun close() = dispose()
}

/**
 * app-mcp 客户端。
 *
 * 线程模型：原生运行时在分发线程上同步回调；这里在回调中启动协程（配置的 [AppMcpConfig.dispatcher]，
 * Android 默认 `Dispatchers.Main`），协程结束后从所在线程提交结果。Host 取消 / 超时 / 断线会取消协程。
 */
class AppMcp private constructor(
    private val config: AppMcpConfig,
    private val dispatcher: CoroutineDispatcher,
) : AppMcpRegistrar(), AutoCloseable {
    override val owner: AppMcp get() = this

    private val closed = java.util.concurrent.atomic.AtomicBoolean(false)
    private val scope = CoroutineScope(SupervisorJob() + dispatcher + CoroutineName("app-mcp"))
    private val _state = MutableStateFlow(StateInfo(StateStatus.IDLE, null, null, null))

    /** 原生状态回调次数（每次回调 +1，计数不会被合并）：[handleWakeAndAwaitSleep] 据此等待变化而不是轮询。 */
    private val stateChanges = MutableStateFlow(0L)

    /** 进入非 `DORMANT` / `STOPPED` 状态的回调次数：等待期间即使整段往返发生在两次检查之间也能看出曾经醒来。 */
    private val activeEntries = java.util.concurrent.atomic.AtomicLong(0)

    /** [handleWakeAndAwaitSleep] 检查原生状态的累计次数（测试用：断言等待不轮询）。 */
    internal val awaitSleepChecks = java.util.concurrent.atomic.AtomicLong(0)

    private val inner: AppMcpClient

    init {
        val listener = object : ClientListener {
            override fun onStateChanged(state: StateInfo) {
                _state.value = state
                if (state.status != StateStatus.DORMANT && state.status != StateStatus.STOPPED) {
                    activeEntries.incrementAndGet()
                }
                stateChanges.update { it + 1 }
            }

            override fun onPaired(token: String) {
                runCatching { config.onPaired?.invoke(token) }
            }

            override fun onLog(level: LogLevel, message: String) {
                runCatching { config.onLog?.invoke(level, message) }
            }

            override fun onIdleExit() {
                runCatching { config.onIdleExit?.invoke() }
            }
        }
        inner = AppMcpClient(config.toFfi(), listener)
        _state.value = inner.state()
    }

    companion object {
        /** 创建客户端（不连接）。配置非法时抛 [AppMcpException]。 */
        @JvmStatic
        fun create(config: AppMcpConfig): AppMcp =
            AppMcp(config, config.dispatcher ?: defaultDispatcher())

        /** 有可用的 `Dispatchers.Main` 则用它，否则 `Dispatchers.Default`。 */
        @JvmStatic
        fun defaultDispatcher(): CoroutineDispatcher = try {
            Dispatchers.Main.also { it.isDispatchNeeded(EmptyCoroutineContext) }
        } catch (_: Throwable) {
            Dispatchers.Default
        }
    }

    override fun registerRaw(spec: ToolSpec, handler: ToolHandler) = inner.registerTool(spec, handler)
    override fun registerRaw(spec: ResourceSpec, reader: ResourceReader) = inner.registerResource(spec, reader)
    override fun createRaw(name: String) = inner.createScope(name)

    /** 连接状态。 */
    val state: StateFlow<StateInfo> = _state.asStateFlow()

    val instanceId: String get() = inner.instanceId()

    /** 当前 token（配置带入的或配对后获得的）。 */
    val token: String? get() = inner.token()

    /** Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 `cid` 对应；未连接时为 null。 */
    val connectionId: String? get() = inner.connectionId()

    /** 开始连接 Host（重复调用无效果）。 */
    fun start(): AppMcp = apply { inner.start() }

    /** 停止：取消所有调用、断开连接、不再重连。 */
    fun stop() = inner.stop()

    fun setVisibility(visibility: Visibility, focused: Boolean = true) = inner.setVisibility(visibility, focused)

    /** 挂起直到进入 [status]；超时返回 false。 */
    suspend fun awaitState(status: StateStatus, timeoutMillis: Long = Long.MAX_VALUE): Boolean =
        withTimeoutOrNull(timeoutMillis) { state.first { it.status == status } } != null

    /** 生效的生命周期策略。 */
    val lifecycle: LifecyclePolicy = config.lifecycle ?: LifecyclePolicy()

    /** 是否已 [close]。 */
    val isClosed: Boolean get() = closed.get()

    /** 当前状态（直接读取原生层；[state] 由分发线程异步更新，可能稍有滞后）。已关闭时为 `STOPPED`。 */
    fun currentState(): StateInfo =
        if (closed.get()) StateInfo(StateStatus.STOPPED, null, null, null) else inner.state()

    // -- 生命周期（spec/lifecycle.md 第 8 节） ------------------------------------

    /**
     * 处理操作系统激活参数 / URL（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、
     * `#app-mcp-wake=<token>`；Android 由 `WakeReceiver` 调用）。不是唤醒参数（或已关闭）返回 false。
     * 可以在 [start] 之前调用。
     */
    fun handleWake(args: String): Boolean = !closed.get() && inner.handleWake(args)

    /** App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。 */
    fun wake(reason: WakeReason = WakeReason.APP): Boolean = !closed.get() && inner.wakeWithReason(reason)

    /** `ON_DEMAND` 模式下主动连接；尚未 [start] 时等同于 [start]。 */
    fun connectNow(): Boolean = !closed.get() && inner.connectNow()

    /** App 主动请求休眠（不受空闲条件与持有影响）。返回是否有效果。 */
    fun sleep(reason: SleepReason = SleepReason.APP): Boolean = !closed.get() && inner.sleepWithReason(reason)

    /** 临时阻止自动休眠，直到返回的句柄被关闭（`hold().use { … }`）。 */
    fun hold(): HoldHandle = HoldHandle(inner.hold())

    /** 当前工具与资源定义的摘要（16 个十六进制字符）。 */
    val toolsHash: String get() = inner.toolsHash()

    /**
     * 处理唤醒参数，并挂起到这次唤醒的任务完成、客户端再次休眠（或停止）为止。
     * 供没有界面的唤醒路径使用（Android `WakeWorker`、D-Bus 激活的辅助进程等）：返回后进程可交给系统回收。
     *
     * @return [WakeOutcome.NOT_A_WAKE]：不是唤醒参数；[WakeOutcome.SLEPT]：已回连并再次休眠；
     *   [WakeOutcome.TIMED_OUT]：[timeoutMillis] 内没有再次休眠；[WakeOutcome.PERSISTENT]：`PERSISTENT` 模式
     *   不会休眠，回连成功后立即返回。
     * @param pollMillis 兜底检查间隔：等待由原生状态回调驱动（每次状态变化立即检查），此间隔只防回调丢失。
     * @why 不按固定间隔轮询：唤醒后在线期间（后台可达数十秒）每 100 ms 一次 JNA 读状态，在魅族 18 Pro 后台小核上
     *   约 2 ms / 次，一次后台唤醒耗约 1.3 s 进程 CPU（TASKS.md 4e 真机复测），远超回连本身（约 5.6 ms）。
     */
    suspend fun handleWakeAndAwaitSleep(
        args: String,
        timeoutMillis: Long = 120_000,
        pollMillis: Long = 5_000,
    ): WakeOutcome {
        val activeBefore = activeEntries.get()
        if (!handleWake(args)) return WakeOutcome.NOT_A_WAKE
        // 原生状态为准（状态流由分发线程异步更新）；activeEntries 记下两次检查之间完成的 DORMANT → … → DORMANT 往返。
        var active = false
        val outcome = withTimeoutOrNull(timeoutMillis) {
            var result: WakeOutcome? = null
            while (result == null) {
                val seen = stateChanges.value
                val status = currentState().status
                awaitSleepChecks.incrementAndGet()
                if (activeEntries.get() != activeBefore) active = true
                result = when {
                    status == StateStatus.STOPPED -> WakeOutcome.SLEPT
                    status == StateStatus.DORMANT && active -> WakeOutcome.SLEPT
                    status == StateStatus.CONNECTED && lifecycle.mode == LifecycleMode.PERSISTENT -> WakeOutcome.PERSISTENT
                    else -> {
                        if (status != StateStatus.DORMANT) active = true
                        withTimeoutOrNull(pollMillis) { stateChanges.first { it != seen } }
                        null
                    }
                }
            }
            result
        }
        return outcome ?: WakeOutcome.TIMED_OUT
    }

    /** 停止并释放原生对象与协程作用域。幂等。 */
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        inner.stop()
        scope.cancel()
        inner.close()
        stateChanges.update { it + 1 } // 让 handleWakeAndAwaitSleep 立即看到 STOPPED
    }

    // -- 执行 ------------------------------------------------------------------

    /** 在分发线程上调用。 */
    internal fun runTool(call: Call, handler: ToolFunction) {
        val args = try {
            AppMcpJson.parseToJsonElement(call.argumentsJson().ifEmpty { "{}" }) as? JsonObject
                ?: throw IllegalArgumentException("参数必须是 JSON 对象")
        } catch (e: Exception) {
            failQuietly(call, ErrorKind.INVALID_INPUT, "参数不是合法的 JSON 对象：${e.message}")
            return
        }
        val ctx = ToolContext(call.callId(), call.toolName(), args, call)
        val job = launchGuarded(
            fail = { kind, msg, details -> failQuietly(call, kind, msg, details) },
        ) {
            val result = handler(args, ctx)
            val completion = when (result) {
                is ToolResult -> CallResult(
                    dataJson = (result.data ?: JsonNull).toString(),
                    stateHints = result.stateHints + ctx.stateHints,
                    status = result.status,
                    stateResource = result.stateResource,
                    summary = result.summary,
                    annotations = result.annotations,
                )
                else -> CallResult(
                    dataJson = anyToJson(result).toString(),
                    stateHints = ctx.stateHints.toList(),
                    status = ResultStatus.DONE,
                )
            }
            try {
                call.completeWith(completion)
            } catch (_: AppMcpException.AlreadyCompleted) {
            }
        }
        call.setCancelListener(object : CancelListener {
            override fun onCancel(reason: CancelReason) {
                ctx.cancelReason = reason
                job.cancel(CancellationException("调用已取消：$reason"))
            }
        })
        job.start()
    }

    /** 在分发线程上调用。 */
    internal fun runRead(read: Read, reader: ResourceFunction) {
        launchGuarded(fail = { kind, msg, _ -> failQuietly(read, kind, msg) }) {
            val data = anyToJson(reader())
            try {
                read.complete(data.toString())
            } catch (_: AppMcpException.AlreadyCompleted) {
            }
        }.start()
    }

    /**
     * 启动（LAZY）执行协程：异常映射为错误类别；在 [AppMcpConfig.dispatchTimeoutMillis] 内未开始执行时
     * 以 `APP_NOT_RESPONDING` 失败，之后即使开始也不再执行。
     */
    private fun launchGuarded(
        fail: (String, String, JsonElement?) -> Unit,
        body: suspend () -> Unit,
    ): Job {
        // 0 = 排队，1 = 执行中，2 = 已放弃
        val gate = AtomicInteger(0)
        val timeout = config.dispatchTimeoutMillis
        val watchdog: Job? = if (timeout > 0) {
            scope.launch(Dispatchers.Default) {
                delay(timeout)
                if (gate.compareAndSet(0, 2)) {
                    fail(ErrorKind.APP_NOT_RESPONDING, "${timeout} ms 内未能在目标线程上开始执行", null)
                }
            }
        } else {
            null
        }
        val job = scope.launch(start = CoroutineStart.LAZY) {
            if (!gate.compareAndSet(0, 1)) return@launch
            watchdog?.cancel()
            try {
                body()
            } catch (e: CancellationException) {
                fail(ErrorKind.CANCELLED, "调用已取消", null)
                throw e
            } catch (e: ToolCallException) {
                fail(e.kind, e.message, e.details)
            } catch (e: Throwable) {
                fail(ErrorKind.HANDLER_ERROR, e.message ?: e::class.java.simpleName, null)
            }
        }
        job.invokeOnCompletion { watchdog?.cancel() }
        return job
    }

    private fun failQuietly(call: Call, kind: String, message: String, details: JsonElement? = null) {
        try {
            call.failWithDetails(kind, message, details?.toString())
        } catch (_: AppMcpException.AlreadyCompleted) {
        } catch (_: AppMcpException.UnknownErrorKind) {
            failQuietly(call, ErrorKind.HANDLER_ERROR, message, details)
        } catch (_: AppMcpException) {
        }
    }

    private fun failQuietly(read: Read, kind: String, message: String) {
        try {
            read.fail(kind, message)
        } catch (_: AppMcpException.AlreadyCompleted) {
        } catch (_: AppMcpException.UnknownErrorKind) {
            failQuietly(read, ErrorKind.HANDLER_ERROR, message)
        } catch (_: AppMcpException) {
        }
    }
}

/** [AppMcp.handleWakeAndAwaitSleep] 的结果。 */
enum class WakeOutcome {
    /** 参数不是本 SDK 的唤醒（或客户端已关闭）。 */
    NOT_A_WAKE,
    /** 已回连，任务完成后再次休眠（或客户端已停止）。 */
    SLEPT,
    /** `PERSISTENT` 模式：已回连，不会休眠。 */
    PERSISTENT,
    /** 超时仍未再次休眠。 */
    TIMED_OUT,
}
