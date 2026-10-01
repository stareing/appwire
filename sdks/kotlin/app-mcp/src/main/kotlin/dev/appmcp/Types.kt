package dev.appmcp

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.serialization.json.JsonElement

// 直接复用 uniffi 生成的数据类型。
/** 风险等级（旧写法：优先用 [ToolAnnotations]；两者同时存在时注解中声明的字段优先，缺少的按 risk 推导）。 */
typealias Risk = dev.appmcp.ffi.Risk
/** 标准 MCP 工具注解（title、readOnlyHint、destructiveHint、idempotentHint、openWorldHint，均可选）。 */
typealias ToolAnnotations = dev.appmcp.ffi.ToolAnnotations
/** 结果内容的标注（MCP 内容注解：audience、priority、lastModified）。 */
typealias ContentAnnotations = dev.appmcp.ffi.ContentAnnotations
typealias Audience = dev.appmcp.ffi.Audience
/** 调用结果的业务状态：`DONE`（缺省）/ `PENDING` / `PARTIAL` / `NOOP`。 */
typealias ResultStatus = dev.appmcp.ffi.ResultStatus
typealias Activation = dev.appmcp.ffi.Activation
typealias Visibility = dev.appmcp.ffi.Visibility
typealias CancelReason = dev.appmcp.ffi.CancelReason
typealias StateInfo = dev.appmcp.ffi.StateInfo
typealias StateStatus = dev.appmcp.ffi.StateStatus
typealias LogLevel = dev.appmcp.ffi.LogLevel
typealias AppOverview = dev.appmcp.ffi.AppOverview
/** 原生层错误的基类。具体子类（如 `AlreadyCompleted`）需通过 `dev.appmcp.ffi.AppMcpException` 访问（typealias 不能访问嵌套类）。 */
typealias AppMcpException = dev.appmcp.ffi.AppMcpException

/** 协议错误类别（FFI 上的字符串形式）。 */
object ErrorKind {
    const val TOOL_NOT_FOUND = "TOOL_NOT_FOUND"
    const val TOOL_DISABLED = "TOOL_DISABLED"
    const val INVALID_INPUT = "INVALID_INPUT"
    const val USER_REJECTED = "USER_REJECTED"
    const val TIMEOUT = "TIMEOUT"
    const val HANDLER_ERROR = "HANDLER_ERROR"
    const val CANCELLED = "CANCELLED"
    const val APP_DISCONNECTED = "APP_DISCONNECTED"
    const val APP_NOT_INSTALLED = "APP_NOT_INSTALLED"
    const val LAUNCH_FAILED = "LAUNCH_FAILED"
    const val APP_NOT_RESPONDING = "APP_NOT_RESPONDING"
    const val INSTANCE_FROZEN = "INSTANCE_FROZEN"
    const val RESOURCE_NOT_FOUND = "RESOURCE_NOT_FOUND"
    const val UNAUTHORIZED = "UNAUTHORIZED"
    const val UNSUPPORTED_PROTOCOL = "UNSUPPORTED_PROTOCOL"
    /** Host 侧限流（App 一般不抛）。 */
    const val RATE_LIMITED = "RATE_LIMITED"
    /** Host 侧大小上限（App 一般不抛）。 */
    const val PAYLOAD_TOO_LARGE = "PAYLOAD_TOO_LARGE"

    /** 原生库认可的全部类别。 */
    val all: Set<String> by lazy { dev.appmcp.ffi.errorKinds().toSet() }
}

/**
 * handler 抛出此异常以指定错误类别，例如 `throw ToolCallException(ErrorKind.INVALID_INPUT, "数量必须为正")`。
 * 其他异常一律映射为 `HANDLER_ERROR`；协程取消映射为 `CANCELLED`。
 *
 * @property details 结构化详情：对象的字段合并进错误的 `data`，其他值放在 `data.details`
 *   （如 `buildJsonObject { put("field", "quantity") }`）。
 */
class ToolCallException @JvmOverloads constructor(
    val kind: String,
    override val message: String,
    val details: JsonElement? = null,
) : Exception(message)

/**
 * 结构化调用结果，作为 handler 返回值（spec/protocol.md 3.2）。直接返回普通值 = `DONE` 且无附加信息。
 *
 * @property data 返回值；null 表示无返回值（Hub 对模型输出"已完成"）。
 * @property stateHints 调用后内容可能变化的资源名。
 * @property status 业务状态：`PENDING`（已受理、待 App 内确认或异步完成）/ `PARTIAL` / `NOOP`；缺省 `DONE`。
 * @property stateResource `PENDING` 时可读取后续状态的资源名。
 * @property summary 一句面向模型 / 用户的结论（`PARTIAL` 时说明完成了哪部分）。
 * @property annotations 结果内容的标注，Hub 原样转发。
 */
data class ToolResult(
    val data: JsonElement?,
    val stateHints: List<String> = emptyList(),
    val status: ResultStatus = ResultStatus.DONE,
    val stateResource: String? = null,
    val summary: String? = null,
    val annotations: ContentAnnotations? = null,
)

/**
 * 客户端配置。
 *
 * @property dispatcher handler 执行所在的调度器。为空时：有 `Dispatchers.Main`（Android、Swing/JavaFX
 *   加对应 coroutines 模块）则用它，否则用 `Dispatchers.Default`。
 * @property dispatchTimeoutMillis 调度后超过该时间仍未开始执行（主线程卡住）时，以
 *   `APP_NOT_RESPONDING` 失败；`<= 0` 表示不限。
 */
data class AppMcpConfig(
    val appId: String,
    val appName: String,
    val hostUrl: String? = null,
    val instanceId: String? = null,
    val appVersion: String? = null,
    val instanceTitle: String? = null,
    val token: String? = null,
    val launchToken: String? = null,
    val maxConcurrentCalls: Int = 1,
    val overview: AppOverview? = null,
    val dispatcher: CoroutineDispatcher? = null,
    val dispatchTimeoutMillis: Long = 10_000,
    /** 配对成功得到的新 token，App 应持久化，下次放入 [token]。在原生分发线程上调用。 */
    val onPaired: ((String) -> Unit)? = null,
    /** 原生库日志。在原生分发线程上调用；为空时丢弃。 */
    val onLog: ((LogLevel, String) -> Unit)? = null,
    /** 生命周期策略（spec/lifecycle.md）；为空时为 `PERSISTENT`（不休眠）。Android 封装默认 `ON_DEMAND` + `sleepOnBackground`。 */
    val lifecycle: LifecyclePolicy? = null,
    /** 建立连接的超时；为空时为 5000 ms。 */
    val connectTimeoutMillis: Long? = null,
    /** 心跳策略（spec/lifecycle.md 第 11 节 A3）：`AUTO` 按传输（本地 IPC / 桌面回环不发）、`ALWAYS`、`OFF`。 */
    val heartbeat: HeartbeatMode = HeartbeatMode.AUTO,
    /**
     * 已进入休眠，且 [LifecyclePolicy.residency] 允许退出进程。在原生分发线程上调用；由 App 决定是否退出
     * （如 `exitProcess(0)` 或关闭最后一个窗口）。
     */
    val onIdleExit: (() -> Unit)? = null,
) {
    internal fun toFfi() = dev.appmcp.ffi.ClientConfig(
        appId = appId,
        appName = appName,
        instanceId = instanceId,
        hostUrl = hostUrl,
        appVersion = appVersion,
        instanceTitle = instanceTitle,
        token = token,
        launchToken = launchToken,
        maxConcurrentCalls = maxConcurrentCalls.coerceAtLeast(1).toUInt(),
        overview = overview,
        lifecycle = lifecycle?.toFfi(),
        connectTimeoutMs = connectTimeoutMillis?.coerceIn(1, UInt.MAX_VALUE.toLong())?.toUInt(),
        heartbeat = heartbeat,
    )
}
