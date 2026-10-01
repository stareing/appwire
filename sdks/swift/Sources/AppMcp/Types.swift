import AppMcpBindings
import Foundation

// 直接复用 uniffi 生成的数据类型。
public typealias Risk = AppMcpBindings.Risk
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

    private let lock = NSLock()
    private var _cancelReason: CancelReason?
    private var _stateHints: [String] = []
    /// 原生调用句柄（单元测试构造的上下文为 `nil`）。
    let call: Call?

    init(callId: String, toolName: String, argumentsJSON: String, call: Call? = nil) {
        self.callId = callId
        self.toolName = toolName
        self.argumentsJSON = argumentsJSON
        self.call = call
    }

    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的后台长任务），直到返回的持有被释放。
    /// 必须在调用结束前获取；调用已完成或已取消时抛 `AppMcpError.AlreadyCompleted`。
    public func hold() throws -> SleepHold {
        guard let call else { throw AppMcpError.AlreadyCompleted }
        return SleepHold(try call.hold())
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
        onIdleExit: (@MainActor @Sendable () -> Void)? = nil
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
            heartbeat: heartbeat
        )
    }
}
