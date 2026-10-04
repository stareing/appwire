import AppMcpBindings
import Foundation

// 直接复用 uniffi 生成的数据类型。
/// 风险等级（旧写法：优先用 `ToolAnnotations`；两者同时存在时注解中声明的字段优先，缺少的按 risk 推导）。
public typealias Risk = AppMcpBindings.Risk
/// 标准 MCP 工具注解（title、readOnlyHint、destructiveHint、idempotentHint、openWorldHint，均可选）。
public typealias ToolAnnotations = AppMcpBindings.ToolAnnotations
/// 结果缓存声明（spec/protocol.md 3.6）：`ttlMs`（1...86_400_000）内相同请求的结果可由 Hub 复用；`scope` 为 `nil` = `.private`。
public typealias CachePolicy = AppMcpBindings.CachePolicy
/// 结果缓存范围：`.private`（按调用方隔离，缺省）/ `.shared`（全体调用方共用，只用于与调用方无关的数据）。
public typealias CacheScope = AppMcpBindings.CacheScope
/// 结果内容的标注（MCP 内容注解：audience、priority、lastModified）。
public typealias ContentAnnotations = AppMcpBindings.ContentAnnotations
public typealias Audience = AppMcpBindings.Audience
/// 调用结果的业务状态：`.done`（缺省）/ `.pending` / `.partial` / `.noop`。
public typealias ResultStatus = AppMcpBindings.ResultStatus
public typealias Activation = AppMcpBindings.Activation
public typealias Visibility = AppMcpBindings.Visibility
public typealias CancelReason = AppMcpBindings.CancelReason
public typealias StateInfo = AppMcpBindings.StateInfo
public typealias StateStatus = AppMcpBindings.StateStatus
public typealias LogLevel = AppMcpBindings.LogLevel
public typealias AppOverview = AppMcpBindings.AppOverview
/// 原生层错误（重名、非法名称、已停止等）。
public typealias AppMcpError = AppMcpBindings.AppMcpError
public typealias LifecycleMode = AppMcpBindings.LifecycleMode
public typealias Residency = AppMcpBindings.Residency
public typealias WakeKind = AppMcpBindings.WakeKind
public typealias WakeReason = AppMcpBindings.WakeReason
public typealias SleepReason = AppMcpBindings.SleepReason
public typealias WakeDescriptor = AppMcpBindings.WakeDescriptor
public typealias HeartbeatMode = AppMcpBindings.HeartbeatMode
/// 调用去重策略（spec/protocol.md 3.3）：已开始执行的 `callId` 的首次结果在 `ttlMs` 内重放，最多 `maxEntries` 条；
/// 任一为 0 关闭。`CallDedupPolicy()` = 300000 ms、64 条。
public typealias CallDedupPolicy = AppMcpBindings.CallDedupPolicy

/// 协议错误类别（FFI 上的字符串形式）。
public enum ErrorKind {
    public static let toolNotFound = "TOOL_NOT_FOUND"
    public static let toolDisabled = "TOOL_DISABLED"
    public static let invalidInput = "INVALID_INPUT"
    public static let userRejected = "USER_REJECTED"
    public static let timeout = "TIMEOUT"
    public static let handlerError = "HANDLER_ERROR"
    public static let cancelled = "CANCELLED"
    public static let appDisconnected = "APP_DISCONNECTED"
    public static let appNotInstalled = "APP_NOT_INSTALLED"
    public static let launchFailed = "LAUNCH_FAILED"
    public static let appNotResponding = "APP_NOT_RESPONDING"
    public static let instanceFrozen = "INSTANCE_FROZEN"
    public static let resourceNotFound = "RESOURCE_NOT_FOUND"
    public static let unauthorized = "UNAUTHORIZED"
    public static let unsupportedProtocol = "UNSUPPORTED_PROTOCOL"
    /// Host 侧限流（App 一般不抛）。
    public static let rateLimited = "RATE_LIMITED"
    /// Host 侧大小上限（App 一般不抛）。
    public static let payloadTooLarge = "PAYLOAD_TOO_LARGE"
    /// Host 侧策略规则拒绝（App 一般不抛）。
    public static let policyDenied = "POLICY_DENIED"
    /// 需要用户本人操作后才能继续（用 `ToolCallError.userActionRequired(message:reason:uri:)` 构造）。
    public static let userActionRequired = "USER_ACTION_REQUIRED"
    /// 导航没有完成（不支持 / 出错 / 超时 / 导航后工具未出现，spec/protocol.md 3.4）。
    public static let navigationFailed = "NAVIGATION_FAILED"
    /// 导航被拒绝（App 拒绝或页面不可由 Agent 导航）。
    public static let navigationDenied = "NAVIGATION_DENIED"
    /// 对象锁冲突（由 Host 产生，App 一般不用；spec/hub-api.md 3.6「对象锁」）。
    public static let locked = "LOCKED"

    /// 原生库认可的全部类别。
    public static var all: Set<String> { Set(AppMcpBindings.errorKinds()) }
}

/// handler 抛出此错误以指定错误类别，例如 `throw ToolCallError(ErrorKind.invalidInput, "数量必须为正")`。
///
/// 其他错误映射为 `HANDLER_ERROR`；`DecodingError`（参数解码失败）映射为 `INVALID_INPUT`；
/// 任务取消（`CancellationError`）映射为 `CANCELLED`。
///
/// 可附带结构化详情 `details`（对象的字段合并进错误的 `data`，其他值放在 `data.details`），例如
/// `throw ToolCallError(ErrorKind.invalidInput, "库存不足", details: .object(["available": .number(2)]))`。
public struct ToolCallError: Error, Sendable, Equatable, CustomStringConvertible {
    public let kind: String
    public let message: String
    public let details: JSONValue?

    public init(_ kind: String, _ message: String, details: JSONValue? = nil) {
        self.kind = kind
        self.message = message
        self.details = details
    }

    public var description: String { "\(kind): \(message)" }

    /// `USER_ACTION_REQUIRED`（spec/protocol.md 第 4 节）：需要用户本人操作后才能继续（登录过期、系统权限未授予、
    /// 需切到前台、需在 App 内确认），例如
    /// `throw ToolCallError.userActionRequired(message: "登录已过期，请重新登录", reason: UserActionReason.login, uri: "shop://login")`。
    ///
    /// @input message 面向用户的说明（Agent 应转告用户）
    /// @input reason 可选类别：`UserActionReason` 中的值或其他字符串；`nil` 时不出现在 `data` 中
    /// @input uri 可选的 App 内入口（深链接等）；`nil` 时不出现在 `data` 中
    public static func userActionRequired(message: String, reason: String? = nil, uri: String? = nil) -> ToolCallError {
        var fields: [String: JSONValue] = [:]
        if let reason { fields["reason"] = .string(reason) }
        if let uri { fields["uri"] = .string(uri) }
        return ToolCallError(ErrorKind.userActionRequired, message, details: fields.isEmpty ? nil : .object(fields))
    }
}

/// `USER_ACTION_REQUIRED` 的 `data.reason` 建议取值（接收方遇到其他值按原样展示）。
public enum UserActionReason {
    /// 登录已过期 / 未登录。
    public static let login = "login"
    /// 系统权限未授予（相机、位置、通知等）。
    public static let permission = "permission"
    /// 需要把 App 切到前台。
    public static let foreground = "foreground"
    /// 需要用户在 App 内确认。
    public static let confirm = "confirm"
}

/// 结构化调用结果（spec/protocol.md 3.2），作为 handler 返回值。直接返回普通值 = `.done` 且无附加信息。
///
/// ```swift
/// return ToolResult(data: order, status: .pending, stateResource: "order.state", summary: "已提交，等待用户付款")
/// ```
///
/// @invariant 不遵循 `Encodable`：注册重载据此区分结构化结果与普通返回值。
public struct ToolResult<Value: Encodable> {
    /// 返回值；`nil` 表示无返回值（Hub 对模型输出"已完成"）。
    public var data: Value?
    /// 调用后内容可能变化的资源名（与 `ToolContext.addStateHint` 合并）。
    public var stateHints: [String]
    /// 业务状态：`.pending`（已受理、待 App 内确认或异步完成）/ `.partial` / `.noop`；缺省 `.done`。
    public var status: ResultStatus
    /// `.pending` 时可读取后续状态的资源名。
    public var stateResource: String?
    /// 一句面向模型 / 用户的结论（`.partial` 时说明完成了哪部分）。
    public var summary: String?
    /// 结果内容的标注，Hub 原样转发。
    public var annotations: ContentAnnotations?

    public init(
        data: Value?,
        stateHints: [String] = [],
        status: ResultStatus = .done,
        stateResource: String? = nil,
        summary: String? = nil,
        annotations: ContentAnnotations? = nil
    ) {
        self.data = data
        self.stateHints = stateHints
        self.status = status
        self.stateResource = stateResource
        self.summary = summary
        self.annotations = annotations
    }
}

extension ToolResult {
    /// 原生调用结果（`stateHints` 只含本结果声明的部分；上下文中的由提交方合并）。
    func ffi() throws -> CallResult {
        CallResult(
            dataJson: try data.map { try encodeJSON($0) },
            stateHints: stateHints,
            status: status,
            stateResource: stateResource,
            summary: summary,
            annotations: annotations
        )
    }
}

/// 无参数工具的参数类型。
public struct NoArguments: Codable, Sendable, Equatable {
    public init() {}
}

/// 任意 JSON 值，用于不想定义结构体的参数或返回值。
public enum JSONValue: Codable, Sendable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Double.self) {
            self = .number(n)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else {
            self = .object(try c.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case let .bool(b): try c.encode(b)
        case let .number(n): try c.encode(n)
        case let .string(s): try c.encode(s)
        case let .array(a): try c.encode(a)
        case let .object(o): try c.encode(o)
        }
    }

    public subscript(key: String) -> JSONValue? {
        if case let .object(o) = self { return o[key] }
        return nil
    }

    public var stringValue: String? {
        if case let .string(s) = self { return s }
        return nil
    }

    public var doubleValue: Double? {
        if case let .number(n) = self { return n }
        return nil
    }
}

/// 一次工具调用的上下文。
public final class ToolContext: @unchecked Sendable {
    public let callId: String
    public let toolName: String
    /// 已由 Host 按 inputSchema 校验过的参数（JSON 文本）。
    public let argumentsJSON: String
    /// Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 `nil`。App 自行决定如何使用（如作为业务去重键）。
    public let idempotencyKey: String?

    private let lock = NSLock()
    private var _cancelReason: CancelReason?
    private var _stateHints: [String] = []
    /// 原生调用句柄（单元测试构造的上下文为 `nil`）。
    let call: Call?

    init(callId: String, toolName: String, argumentsJSON: String, call: Call? = nil, idempotencyKey: String? = nil) {
        self.callId = callId
        self.toolName = toolName
        self.argumentsJSON = argumentsJSON
        self.idempotencyKey = idempotencyKey
        self.call = call
    }

    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的后台长任务），直到返回的持有被释放。
    /// 必须在调用结束前获取；调用已完成或已取消时抛 `AppMcpError.AlreadyCompleted`。
    public func hold() throws -> SleepHold {
        guard let call else { throw AppMcpError.AlreadyCompleted }
        return SleepHold(try call.hold())
    }

    /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。
    /// `progress` 应递增（不递增的值被 Host 丢弃），`total` 未知时为 `nil`。调用已结束、已取消或未连接时无副作用。
    public func progress(_ progress: Double, total: Double? = nil, message: String? = nil) {
        guard let call, !isCancelled else { return }
        // 调用刚结束时原生层报 AlreadyCompleted：进度只是提示，不影响结果。
        try? call.reportProgress(progress: progress, total: total, message: message)
    }

    /// 调用被取消的原因；未取消为 `nil`。取消同时会取消执行 handler 的 Task。
    public var cancelReason: CancelReason? {
        lock.lock()
        defer { lock.unlock() }
        return _cancelReason
    }

    public var isCancelled: Bool { cancelReason != nil }

    /// 声明调用后内容可能变化的资源，提示模型重新读取。
    public func addStateHint(_ resourceName: String) {
        lock.lock()
        _stateHints.append(resourceName)
        lock.unlock()
    }

    var stateHints: [String] {
        lock.lock()
        defer { lock.unlock() }
        return _stateHints
    }

    func markCancelled(_ reason: CancelReason) {
        lock.lock()
        if _cancelReason == nil { _cancelReason = reason }
        lock.unlock()
    }
}

/// 客户端配置。
public struct AppMcpConfig {
    public var appId: String
    public var appName: String
    /// Host 端点：`unix:<绝对路径>` / `ws://…` / `wss://…`（spec/protocol.md 第 1 节）。为空时：环境变量
    /// `APP_MCP_ENDPOINT` → 平台默认本地 IPC 端点（macOS / Linux 的 Unix 域套接字）→ `ws://127.0.0.1:7717`（iOS）。
    public var hostURL: String?
    public var instanceId: String?
    public var appVersion: String?
    public var instanceTitle: String?
    /// 之前配对得到的 token（见 `onPaired`）。
    public var token: String?
    public var launchToken: String?
    public var maxConcurrentCalls: Int
    /// App 总览：模型首次接触本 App 时由 Host 附带。
    public var overview: AppOverview?
    /// handler 调度后超过该秒数仍未开始执行（主线程卡住）时以 `APP_NOT_RESPONDING` 失败；`nil` 表示不限。
    public var dispatchTimeout: TimeInterval?
    /// 状态变化（在原生分发线程上调用）。
    public var onStateChange: (@Sendable (StateInfo) -> Void)?
    /// 配对成功得到的新 token，应持久化（在原生分发线程上调用）。
    public var onPaired: (@Sendable (String) -> Void)?
    /// 原生库日志（在原生分发线程上调用）。
    public var onLog: (@Sendable (LogLevel, String) -> Void)?
    /// 生命周期策略（spec/lifecycle.md）。为 `nil` 时用 `LifecyclePolicy.platformDefault`
    /// （iOS：`onDemand` 且进入后台立即休眠；其他平台：`persistent`）。桌面 App 有 URL scheme 唤醒时传
    /// `.platformDefault(wake: .urlScheme("…"))` 得到 `idle`。
    public var lifecycle: LifecyclePolicy?
    /// 建立连接的超时；为 `nil` 时为 5 秒。
    public var connectTimeout: TimeInterval?
    /// 心跳策略（spec/lifecycle.md 第 11 节 A3）：`.auto` 按传输（本地 IPC / 桌面回环不发）、`.always`、`.off`。
    public var heartbeat: HeartbeatMode
    /// 已休眠且驻留策略允许退出进程时调用（在主 actor 上）。App 自行决定是否退出。
    public var onIdleExit: (@MainActor @Sendable () -> Void)?
    /// 调用去重（spec/protocol.md 3.3）；为 `nil` 时 300000 ms、64 条，`CallDedupPolicy(ttlMs: 0, maxEntries: 0)` 关闭。
    /// 命中时经 `onLog` 记一条警告日志。
    public var callDedup: CallDedupPolicy?
    /// 后台时是否仍把导航请求交给导航回调（spec/protocol.md 3.4「后台与前台」；之后可用
    /// `AppMcpClient.setNavigateInBackground(_:)` 修改）。为 `nil` 时取平台默认：macOS / Linux 为 `true`（可自行前置窗口），
    /// iOS 为 `false`（后台不能自行打开界面，导航立即以 `USER_ACTION_REQUIRED`（reason `foreground`）回复）。
    /// 设为 `true` 时由回调决定，如发本地通知请用户点开后返回 `.userActionRequired(...)`。
    public var navigateInBackground: Bool?
    /// 按名寻址（spec/naming.md）：`start()` 后在系统名字服务登记本 App，Hub（`app-mcp-host serve --name-service`）按名拨入，
    /// 进程未运行时由系统激活；通常与 `lifecycle` 的 `.onDemand` 同用。Linux：D-Bus 会话总线名 `dev.appmcp.App.<appId>`；
    /// Windows：每 App 命名管道；两者都需先 `app-mcp-host app install` 登记。macOS（launchd 方案未实现）与 iOS（不支持，
    /// spec/naming.md 4.5）上经 `onLog` 报告一条日志，其余照常。默认 `false`。
    public var registerName: Bool
    /// 登记实例名（`[a-z][a-z0-9-]{0,31}`，不能是 `default`）：另登记 `dev.appmcp.App.<appId>.<instance>`，
    /// 供 `appmcp://<appId>/<instance>` 寻址。不合法时创建客户端抛 `AppMcpError.InvalidConfig`。
    public var nameInstance: String?
    /// 排队中的调用上限（spec/protocol.md 5.3）：满后新到的调用以 `RATE_LIMITED`（`scope = "queue"`）拒绝。
    /// 为 `nil` 时 64，`0` = 不限。
    public var maxQueuedCalls: Int?
    /// 用户正在操作（`AppMcpClient.setBusy(_:)`）期间写调用的处理方式（spec/protocol.md 5.3「用户正在操作」）：为 `nil` 时
    /// `.reject`（以 `RATE_LIMITED`、`scope = "busy"` 拒绝），`.queue` 排队、停手后按序执行。之后可用
    /// `AppMcpClient.setBusyPolicy(_:)` 修改。
    public var busyPolicy: BusyPolicy?

    public init(
        appId: String,
        appName: String,
        hostURL: String? = nil,
        instanceId: String? = nil,
        appVersion: String? = nil,
        instanceTitle: String? = nil,
        token: String? = nil,
        launchToken: String? = nil,
        maxConcurrentCalls: Int = 1,
        overview: AppOverview? = nil,
        dispatchTimeout: TimeInterval? = 10,
        onStateChange: (@Sendable (StateInfo) -> Void)? = nil,
        onPaired: (@Sendable (String) -> Void)? = nil,
        onLog: (@Sendable (LogLevel, String) -> Void)? = nil,
        lifecycle: LifecyclePolicy? = nil,
        connectTimeout: TimeInterval? = nil,
        heartbeat: HeartbeatMode = .auto,
        onIdleExit: (@MainActor @Sendable () -> Void)? = nil,
        callDedup: CallDedupPolicy? = nil,
        navigateInBackground: Bool? = nil,
        registerName: Bool = false,
        nameInstance: String? = nil,
        maxQueuedCalls: Int? = nil,
        busyPolicy: BusyPolicy? = nil
    ) {
        self.appId = appId
        self.appName = appName
        self.hostURL = hostURL
        self.instanceId = instanceId
        self.appVersion = appVersion
        self.instanceTitle = instanceTitle
        self.token = token
        self.launchToken = launchToken
        self.maxConcurrentCalls = maxConcurrentCalls
        self.overview = overview
        self.dispatchTimeout = dispatchTimeout
        self.onStateChange = onStateChange
        self.onPaired = onPaired
        self.onLog = onLog
        self.lifecycle = lifecycle
        self.connectTimeout = connectTimeout
        self.heartbeat = heartbeat
        self.onIdleExit = onIdleExit
        self.callDedup = callDedup
        self.navigateInBackground = navigateInBackground
        self.registerName = registerName
        self.nameInstance = nameInstance
        self.maxQueuedCalls = maxQueuedCalls
        self.busyPolicy = busyPolicy
    }
}

extension AppMcpConfig {
    /// 原生配置；`lifecycle` 为生效策略（`self.lifecycle ?? .platformDefault`）。
    func ffi(lifecycle: LifecyclePolicy) -> ClientConfig {
        ClientConfig(
            appId: appId,
            appName: appName,
            instanceId: instanceId,
            clientKind: nil,
            hostUrl: hostURL,
            appVersion: appVersion,
            instanceTitle: instanceTitle,
            token: token,
            launchToken: launchToken,
            maxConcurrentCalls: UInt32(max(1, maxConcurrentCalls)),
            overview: overview,
            lifecycle: lifecycle.ffi,
            connectTimeoutMs: connectTimeout.map { UInt32(max(1, min(Double(UInt32.max), $0 * 1000))) },
            heartbeat: heartbeat,
            callDedup: callDedup,
            registerName: registerName,
            nameInstance: nameInstance,
            maxQueuedCalls: maxQueuedCalls.map { UInt32(clamping: max(0, $0)) },
            busyPolicy: busyPolicy
        )
    }
}
