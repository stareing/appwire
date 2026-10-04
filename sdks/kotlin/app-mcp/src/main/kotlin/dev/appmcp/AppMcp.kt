package dev.appmcp

import dev.appmcp.ffi.AppMcpClient
import dev.appmcp.ffi.AppMcpException
import dev.appmcp.ffi.Call
import dev.appmcp.ffi.CallResult
import dev.appmcp.ffi.CancelListener
import dev.appmcp.ffi.ClientListener
import dev.appmcp.ffi.EventInfo
import dev.appmcp.ffi.Navigate
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
    /** Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 null。App 自行决定如何使用（如作为业务去重键）。 */
    val idempotencyKey: String? = null,
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

    /**
     * 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。
     * [progress] 应递增（不递增的值被 Host 丢弃），[total] 未知时为 null。调用已结束、已取消或未连接时无副作用。
     */
    fun progress(progress: Double, total: Double? = null, message: String? = null) {
        val c = call ?: return
        if (isCancelled) return
        try {
            c.reportProgress(progress, total, message)
        } catch (_: AppMcpException.AlreadyCompleted) {
            // 调用刚结束：进度只是提示，不影响结果。
        }
    }
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
     * @param surface `VIEW` = 依赖界面（spec/protocol.md 3.4），只在所在界面可见时启用（Android 用
     *   `dev.appmcp.android.enableWhile` / Compose `ViewToolEffect` 绑定生命周期）；缺省 `APP`。
     * @param page 所在页面名；Hub 在该工具未注册时据此导航（[AppMcp.setNavigationHandler]）。
     * @param backgroundTool 后台替身（只对 `VIEW` 工具有意义）：同 App 内一个 `APP` 工具的名称；App 在后台、本工具不可调用时
     *   Hub 改调该工具（spec/protocol.md 3.4「后台与前台」）。
     * @param concurrency 本工具同时执行的调用上限；0（缺省）= 不单独限制，只受 [AppMcpConfig.maxConcurrentCalls] 约束。
     * @param exclusive 互斥组名（命名规则同工具名）：同组工具同一时刻最多执行一个调用，其余按到达顺序排队。
     *   [concurrency] 与本项只在 SDK 内生效，不发给 Host（spec/protocol.md 5.3）。
     * @param implements 实现的标准意图（spec/intents.md，每项 `"<动词>@<主版本>"`，如 `listOf("message.send@1")`）；
     *   Agent 经 `apps.intents` 按动词找到实现者。格式不合法时抛 `AppMcpException.InvalidName`。
     * @param cache 结果缓存声明（spec/protocol.md 3.6）：`ttlMs` 内相同参数的调用 Hub 可直接返回上次结果、不调用 handler。
     *   只对生效注解只读的工具生效；`scope` 缺省 `PRIVATE`（按调用方隔离）。`ttlMs` 越界时抛 `AppMcpException.InvalidConfig`。
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
        surface: ToolSurface = ToolSurface.APP,
        page: String? = null,
        backgroundTool: String? = null,
        concurrency: Int = 0,
        exclusive: String? = null,
        implements: List<String> = emptyList(),
        cache: CachePolicy? = null,
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
            surface = surface.takeIf { it != ToolSurface.APP },
            page = page,
            backgroundTool = backgroundTool,
            concurrency = concurrency.coerceAtLeast(0).toUInt(),
            exclusive = exclusive,
            implements = implements,
            cache = cache,
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
        surface: ToolSurface = ToolSurface.APP,
        page: String? = null,
        backgroundTool: String? = null,
        concurrency: Int = 0,
        exclusive: String? = null,
        implements: List<String> = emptyList(),
        cache: CachePolicy? = null,
        noinline handler: suspend (args: A, ctx: ToolContext) -> R,
    ): ToolHandle = typedToolImpl(
        name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool,
        concurrency, exclusive, implements, cache, serializer<A>(), serializer<R>(), handler,
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
        surface: ToolSurface,
        page: String?,
        backgroundTool: String?,
        concurrency: Int,
        exclusive: String?,
        implements: List<String>,
        cache: CachePolicy?,
        argSerializer: KSerializer<A>,
        resultSerializer: KSerializer<R>,
        handler: suspend (A, ToolContext) -> R,
    ): ToolHandle = tool(
        name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool,
        concurrency, exclusive, implements, cache,
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
     * @param annotations 资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上。
     * @param cache 读取结果缓存声明（spec/protocol.md 3.6），同 [tool]；`ttlMs` 越界时抛 `AppMcpException.InvalidConfig`。
     * @error [reader] 抛出 [ToolCallException]（含 [ToolCallException.userActionRequired]）时类别与详情原样交给 Host。
     */
    fun resource(
        name: String,
        description: String,
        mimeType: String? = null,
        realtime: Boolean = false,
        annotations: ContentAnnotations? = null,
        cache: CachePolicy? = null,
        reader: ResourceFunction,
    ): ResourceHandle {
        val o = owner
        val spec = ResourceSpec(name, description, mimeType, realtime, annotations, cache)
        val raw = registerRaw(spec, object : ResourceReader {
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
    private val busyState: BusyState

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
        config.navigateInBackground?.let { inner.setNavigateInBackground(it) }
        busyState = BusyState { busy -> if (!closed.get()) inner.setBusy(busy) }
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

    /**
     * 后台时是否仍把导航请求交给导航回调（spec/protocol.md 3.4「后台与前台」），随时生效；初值见
     * [AppMcpConfig.navigateInBackground]。为 false 时 App 不可见（`HIDDEN` / `FROZEN`）收到的导航立即以
     * `USER_ACTION_REQUIRED`（reason `foreground`）回复，不调用回调。
     */
    fun setNavigateInBackground(enabled: Boolean) = inner.setNavigateInBackground(enabled)

    // -- 用户正在操作（spec/protocol.md 5.3） ------------------------------------------

    /**
     * 声明用户正在 / 不再在 App 内操作（何时算由 App 决定，如编辑框获得焦点、拖拽中）。期间写调用（生效注解不是
     * `readOnlyHint = true` 的工具）按 [AppMcpConfig.busyPolicy] 拒绝或排队；只读调用不受影响。状态只在 SDK 内，不发给 Host。
     * 与 [beginBusy] / [busy] 作用域合并：生效值为「本开关 ∨ 仍有作用域未结束」，`setBusy(false)` 不结束进行中的作用域。
     * 已关闭时只记录、不再交给原生层（作用域可在 [close] 之后结束）。
     */
    fun setBusy(busy: Boolean) = busyState.set(busy)

    /** 核心当前是否处于忙碌状态；已关闭时为 false。 */
    val isBusy: Boolean get() = !closed.get() && inner.isBusy()

    /** 修改忙碌期间写调用的处理方式，随即对排队中的调用生效（如由用户在 App 设置中选择）。已关闭时无效果。 */
    fun setBusyPolicy(policy: BusyPolicy) {
        if (!closed.get()) inner.setBusyPolicy(policy)
    }

    /**
     * 开始一段忙碌作用域，直到返回值 [BusyHold.close]。可嵌套、可跨线程同时持有（引用计数）：最后一个作用域结束且
     * [setBusy] 开关为关时恢复空闲。Compose 见 `dev.appmcp.compose.BusyEffect`。
     */
    fun beginBusy(): BusyHold {
        busyState.enter()
        return BusyHold(busyState)
    }

    /** 作用域写法：[block] 执行期间为忙碌（含异常 / 取消退出时归还），语义同 [beginBusy]。 */
    inline fun <T> busy(block: () -> T): T = beginBusy().use { block() }

    // -- 事件（spec/protocol.md 3.5） ----------------------------------------------------

    /**
     * 声明本实例可发出的事件（同名替换）；[payloadSchema] 为载荷的 JSON Schema（描述用，Hub 不校验）。
     * 已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。名称不合法 / 已停止时抛 [AppMcpException]。
     */
    fun declareEvent(name: String, description: String, payloadSchema: JsonObject? = null) =
        inner.declareEvent(EventInfo(name, description, payloadSchema?.toString()))

    /** 撤销事件声明；未声明过（或已停止）返回 false。 */
    fun removeEvent(name: String): Boolean = inner.removeEvent(name)

    /**
     * 发出已声明的事件。[payload] 为 JSON 对象（[JsonObject] 或 `Map`，转换规则同 [anyToJson]）；null = 无载荷。
     * 已连接时发送并返回 true；未连接（休眠、断线、重连中）丢弃并返回 false：不缓存、不为此连接或唤醒 Host，也不推迟空闲休眠。
     * 需要可靠送达的状态变化请改用资源。未声明 / 名称不合法抛 `AppMcpException.InvalidName`；载荷无法转换、不是对象
     * 或超过 8 KiB 抛 `AppMcpException.InvalidJson`。
     */
    fun emitEvent(name: String, payload: Any? = null): Boolean {
        val json = try {
            anyToJson(payload)
        } catch (e: IllegalArgumentException) {
            throw AppMcpException.InvalidJson("事件载荷无法转换为 JSON：${e.message}")
        }
        return inner.emitEvent(name, json.takeUnless { it is JsonNull }?.toString())
    }

    /**
     * 设置导航回调（Host 的 `app/navigate`，spec/protocol.md 3.4）；null 清除（之后的导航请求以 `NAVIGATION_FAILED`
     * 回复）。回调在 [AppMcpConfig.dispatcher] 上以协程执行（Android 默认主线程），可直接操作界面；
     * 在 [AppMcpConfig.dispatchTimeoutMillis] 内未开始执行时以失败回复。
     *
     * 能力在握手时声明：建议在 [start] 之前设置；连接后才设置的回调在下次连接（回连 / 唤醒）时生效。
     * 框架适配见 [PageRouter]、`dev.appmcp.compose.navigationRouter`。
     */
    fun setNavigationHandler(handler: NavigateFunction?) {
        val o = this
        inner.setNavigationHandler(handler?.let { h ->
            object : dev.appmcp.ffi.NavigationHandler {
                override fun navigate(request: Navigate) = o.runNavigate(request, h)
            }
        })
    }

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

    /**
     * 接受 Hub 交来的通道（socketpair 的一端，spec/naming.md 4.2）：Android `ToolsService` 在 Binder 事务内创建一对，
     * 一端以 fd 交到这里，另一端返回给 Hub。之后 SDK 在其上先发 `app/hello`（`wakeReason: os-activation`）；Hub 关闭
     * （宽限到期 / 解绑 / 进程死亡）后转休眠、不重连。
     *
     * @input [fd] 的所有权随调用转移给 SDK（无论是否接受）。
     * @output 客户端已关闭时为 null，此时 fd **未被接管**，由调用方关闭。
     */
    fun acceptChannelFd(fd: Int): ChannelOffer? = if (closed.get()) null else inner.acceptChannelFd(fd)

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
        val ctx = ToolContext(call.callId(), call.toolName(), args, call, call.idempotencyKey())
        val job = launchGuarded(
            label = "工具 ${call.toolName()}（callId ${call.callId()}）",
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
    internal fun runNavigate(request: Navigate, handler: NavigateFunction) {
        val params = try {
            request.paramsJson()?.let {
                AppMcpJson.parseToJsonElement(it) as? JsonObject ?: throw IllegalArgumentException("页面参数必须是 JSON 对象")
            }
        } catch (e: Exception) {
            finishNavigate(request, NavigationResult.Failed("页面参数不合法：${e.message}"))
            return
        }
        launchGuarded(label = "导航 ${request.page()}", fail = { kind, msg, details ->
            finishNavigate(request, navigationFailure(kind, msg, details))
        }) {
            finishNavigate(request, handler(request.page(), params))
        }.start()
    }

    private fun finishNavigate(request: Navigate, result: NavigationResult) {
        try {
            when (result) {
                NavigationResult.Ok -> request.complete()
                is NavigationResult.Denied -> request.deny(result.message)
                is NavigationResult.Failed -> request.fail(result.message)
                is NavigationResult.UserActionRequired -> request.failUserAction(result.message, result.reason, result.uri)
            }
        } catch (_: AppMcpException) {
            // 已完成或连接已断开：回复被丢弃（spec/protocol.md 3.4）。
        }
    }

    /** 在分发线程上调用。 */
    internal fun runRead(read: Read, reader: ResourceFunction) {
        launchGuarded(label = "资源 ${read.resourceName()}", fail = { kind, msg, details -> failQuietly(read, kind, msg, details) }) {
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
     *
     * @side-effect 未预期异常（非 [ToolCallException]、非取消）经 [AppMcpConfig.onLog] 记一条 ERROR 日志（含堆栈），
     *   [label] 标明是哪个工具 / 导航 / 资源；回复给 Host 的消息仍只有异常消息，不含堆栈（E-05）
     */
    private fun launchGuarded(
        label: String,
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
                logUnexpected(label, e)
                fail(ErrorKind.HANDLER_ERROR, e.message ?: e::class.java.simpleName, null)
            }
        }
        job.invokeOnCompletion { watchdog?.cancel() }
        return job
    }

    private fun logUnexpected(label: String, e: Throwable) {
        runCatching { config.onLog?.invoke(LogLevel.ERROR, "$label 未预期异常：${e.stackTraceToString()}") }
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

    private fun failQuietly(read: Read, kind: String, message: String, details: JsonElement? = null) {
        try {
            read.failWithDetails(kind, message, details?.toString())
        } catch (_: AppMcpException.AlreadyCompleted) {
        } catch (_: AppMcpException.UnknownErrorKind) {
            failQuietly(read, ErrorKind.HANDLER_ERROR, message, details)
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
