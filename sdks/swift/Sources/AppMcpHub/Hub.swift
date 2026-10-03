// Hub SDK（Agent 端）的 Swift 惯用封装：列工具、调用、导出 / 分派 LLM 工具格式、事件、审批与配对。
//
// 基于 uniffi 生成的 AppMcpHubBindings（bash bindings/hub-uniffi/scripts/generate.sh）。
//
//     let hub = try Hub(config: HubConfig(approvalMinRisk: .destructive))
//     hub.setApprovalHandler { req in await MainActor.run { confirm(req) } }
//     Task { for await e in hub.events() { refresh(e) } }
//     let tools = try hub.exportTools(.anthropic)                        // 放进 LLM 请求
//     let reply = try await hub.dispatch(.anthropic, toolCall: blockJSON) // 回填给 LLM
//     hub.close()

import AppMcpHubBindings
import Foundation

// 直接复用生成的数据类型。
public typealias HubConfig = AppMcpHubBindings.HubConfig
public typealias UpstreamSpec = AppMcpHubBindings.UpstreamSpec
public typealias ToolFilter = AppMcpHubBindings.ToolFilter
public typealias ToolFormat = AppMcpHubBindings.ToolFormat
public typealias Risk = AppMcpHubBindings.Risk
public typealias Activation = AppMcpHubBindings.Activation
public typealias Availability = AppMcpHubBindings.Availability
public typealias Visibility = AppMcpHubBindings.Visibility
public typealias AppKind = AppMcpHubBindings.AppKind
public typealias AppInfo = AppMcpHubBindings.AppInfo
public typealias InstanceInfo = AppMcpHubBindings.InstanceInfo
public typealias AppOverviewInfo = AppMcpHubBindings.AppOverviewInfo
public typealias HubTool = AppMcpHubBindings.HubTool
public typealias HubResource = AppMcpHubBindings.HubResource
public typealias ResourceContent = AppMcpHubBindings.ResourceContent
/// 调用进度（`progress`、`total`、`message`；`Hub.callTool(…, onProgress:)`，spec/hub-api.md 3.12）。
public typealias ProgressUpdate = AppMcpHubBindings.ProgressUpdate
public typealias ToolErrorInfo = AppMcpHubBindings.ToolErrorInfo
public typealias ApprovalRequest = AppMcpHubBindings.ApprovalRequest
public typealias PairingRequest = AppMcpHubBindings.PairingRequest
/// 工具暴露方式（`.auto` / `.progressive` / `.all`，spec/hub-api.md 3.7）。
public typealias ToolExposure = AppMcpHubBindings.ToolExposure
/// MCP 出口协商的协议版本范围（`.auto` / `.legacyOnly`，spec/hub-api.md 3.6「协议版本」）。
public typealias McpProtocolMode = AppMcpHubBindings.McpProtocolMode
/// 唤醒器配置（`.system` / `.disabled` / `.exec(argv:)`）。
public typealias WakerConfig = AppMcpHubBindings.WakerConfig
/// 交给 `Hub.setWaker` 的唤醒请求（`appId`、`instanceId`、`descriptor`、`token`、`activationArg`）。
public typealias WakeRequest = AppMcpHubBindings.WakeRequest
/// 唤醒描述（与 App 端模块 `AppMcp.WakeDescriptor` 同名，这里加前缀避免同时导入两个模块时歧义）。
public typealias HubWakeDescriptor = AppMcpHubBindings.WakeDescriptor
/// 唤醒方式（`.uri`、`.aumid`、`.appleEvent`、`.dbus`、`.androidIntent`、`.webUrl`、`.none`）。
public typealias HubWakeKind = AppMcpHubBindings.WakeKind
/// Hub 事件。休眠相关：`.appDormant(appId:instanceId:)`、`.appWaking(appId:instanceId:)`（`nil` = 冷启动）；
/// SDK 诊断上报：`.appDiagnostic(appId:instanceId:code:message:count:)`（spec/protocol.md 10.2）。
/// 未单独映射的新事件以 `.other(kind:json:)` 送达。
public typealias HubEvent = AppMcpHubBindings.HubEvent
/// Hub 操作错误（`.Tool`、`.InvalidJson`、`.InvalidConfig`、`.Io`、`.Shutdown`、`.Unsupported`）。
/// `.Unsupported(detail:)`：本构建未包含所需能力（`mcpHttp`、`upstreams`、`serveHttp`），`detail` 说明缺哪个 cargo feature
/// （spec/hub-api.md 3.10）；重试无效。
public typealias HubError = AppMcpHubBindings.HubError
// 运行状态（`Hub.status()`，spec/hub-api.md 3.9）。
public typealias HubStatus = AppMcpHubBindings.HubStatus
public typealias AuthStatus = AppMcpHubBindings.AuthStatus
public typealias AppStatus = AppMcpHubBindings.AppStatus
public typealias AppState = AppMcpHubBindings.AppState
public typealias InstanceStatus = AppMcpHubBindings.InstanceStatus
public typealias InstanceState = AppMcpHubBindings.InstanceState
public typealias LastError = AppMcpHubBindings.LastError
public typealias DiagnosticReport = AppMcpHubBindings.DiagnosticReport
/// 休眠记录持久化状态（`HubStatus.dormantStore`，配置了 `HubConfig.stateDir` 时）。
public typealias DormantStoreStatus = AppMcpHubBindings.DormantStoreStatus
public typealias StoreIssue = AppMcpHubBindings.StoreIssue
/// Agent 任务（`HubStatus.tasks`，spec/hub-api.md 3.6）：调用方的跨请求状态。
public typealias AgentTaskStatus = AppMcpHubBindings.AgentTaskStatus
/// 调用方的种类：legacy MCP 会话 / 无会话 MCP 请求的主体 / Hub API 会话。
public typealias CallerKind = AppMcpHubBindings.CallerKind
public typealias TaskSelectionStatus = AppMcpHubBindings.TaskSelectionStatus
public typealias TaskLeaseStatus = AppMcpHubBindings.TaskLeaseStatus
/// 未到期的对象锁（`HubStatus.locks`，spec/hub-api.md 3.6「对象锁」）。
public typealias LockStatus = AppMcpHubBindings.LockStatus
// 资源保护与工具声明（spec/hub-api.md 3.11）。
/// 限流与大小上限（`HubConfig.limits`；`HubStatus.limits` 为全部字段给出的生效值）。为空的字段取默认值。
public typealias LimitsConfig = AppMcpHubBindings.LimitsConfig
/// 结果与其 `outputSchema` 不符时的处理：`.off` / `.log`（默认）/ `.reject`。
public typealias OutputValidation = AppMcpHubBindings.OutputValidation
/// 一个工具的声明（`AppStatus.tools`）。
public typealias ToolDeclaration = AppMcpHubBindings.ToolDeclaration
/// 标准 MCP 工具注解（`HubTool.annotations`、`ApprovalRequest.annotations`）。
/// 与 App 端模块 `AppMcp.ToolAnnotations` 同名，这里加前缀避免同时导入两个模块时歧义（下同）。
public typealias HubToolAnnotations = AppMcpHubBindings.ToolAnnotations
/// 内容标注（MCP 内容注解：audience、priority、lastModified）。
public typealias HubContentAnnotations = AppMcpHubBindings.ContentAnnotations
public typealias HubAudience = AppMcpHubBindings.Audience
/// 调用结果的业务状态：`.done` / `.pending` / `.partial` / `.noop`。
public typealias HubResultStatus = AppMcpHubBindings.ResultStatus
/// App 工具对界面的依赖（`HubTool.surface`：`.app` / `.view`，spec/protocol.md 3.4；内置与上游工具为 `nil`）。
public typealias HubToolSurface = AppMcpHubBindings.ToolSurface
// 策略挂点（spec/hub-api.md 3.13）。
/// 策略规则集（`HubConfig.policy`、`Hub.setPolicy`）：按顺序匹配，`.deny` 取第一条命中的规则；空规则集 = 不做任何限制。
public typealias PolicyConfig = AppMcpHubBindings.PolicyConfig
/// 一条策略规则：`app` / `tool` 为精确名或以 `*` 结尾的前缀；`tool` 与 `annotations` 都为空时作用于整个 App。
public typealias PolicyRule = AppMcpHubBindings.PolicyRule
/// `.hide`：不出现在任何列表中、调用为 `TOOL_NOT_FOUND`；`.deny`：在 `hooks` 指定的执行点以 `POLICY_DENIED` 拒绝。
public typealias PolicyAction = AppMcpHubBindings.PolicyAction
/// 执行点；规则的 `hooks` 只能写 `.call` / `.wake`（只用于 `.deny`，为空时为 `[.call]`）。
public typealias PolicyHook = AppMcpHubBindings.PolicyHook
/// 按 App 声明的 MCP 注解匹配（给出的每一项都相等才命中）。
public typealias AnnotationMatch = AppMcpHubBindings.AnnotationMatch
/// 生效的规则、命中次数与最近的加载错误（`Hub.policy()`、`HubStatus.policy`）。
public typealias PolicyStatus = AppMcpHubBindings.PolicyStatus
public typealias PolicyRuleStatus = AppMcpHubBindings.PolicyRuleStatus
public typealias PolicyLoadError = AppMcpHubBindings.PolicyLoadError
/// Agent 访问令牌（`HubConfig.agents`、`Hub.setAgents`，spec/hub-api.md 3.6「Agent 身份」）。
public typealias AgentCredential = AppMcpHubBindings.AgentCredential

/// 工具调用以错误结束（`CallResult.decode` / `CallResult.get()`）。`kind` 为协议错误类别，如 `USER_REJECTED`。
public struct ToolError: Error, Sendable, Equatable, CustomStringConvertible {
    public let kind: String
    public let message: String
    /// 错误详情（JSON 文本）。
    public let detailsJSON: String?

    public var description: String { "\(kind): \(message)" }
}

/// 一次工具调用的结果。
public struct CallResult: Sendable, Equatable {
    public let callId: String
    /// 成功时的结果数据（JSON 文本）。
    public let dataJSON: String?
    /// 失败时的错误（`kind` 如 `USER_REJECTED`、`TIMEOUT`）。
    public let error: ToolErrorInfo?
    public let stateHints: [String]
    /// 实际执行的实例。
    public let instanceId: String?
    /// 本会话首次接触该 App 时附带的总览。
    public let overview: AppOverviewInfo?
    /// App 声明的业务状态（缺省 `.done`；`.pending` 时后续状态见 `stateResource`）。
    public let status: HubResultStatus
    /// `.pending` 时可读取后续状态的资源 URI（`app-mcp://<appId>/<名>`）。
    public let stateResource: String?
    /// App 给出的一句结论。
    public let summary: String?
    /// App 对结果内容的标注，原样。
    public let annotations: HubContentAnnotations?
    /// App 在后台、Hub 改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则为 `nil`。
    public let routedTo: String?
    /// 从 Hub 收到调用到得出结果的毫秒数（spec/hub-api.md 3.15 `dev.appwire/durationMs`）。
    public let durationMs: UInt64
    /// 本次调用是否唤醒了 App（`dev.appwire/woke`）。
    public let woke: Bool

    public var isError: Bool { error != nil }

    /// 成功时返回数据的 JSON 文本，失败时抛出 `ToolError`。
    public func get() throws -> String? {
        if let e = error { throw ToolError(kind: e.kind, message: e.message, detailsJSON: e.detailsJson) }
        return dataJSON
    }

    /// 解码成功结果；失败时抛出 `ToolError`。
    public func decode<T: Decodable>(_ type: T.Type = T.self) throws -> T {
        let json = try get() ?? "null"
        return try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }
}

/// 在 `Hub.setWaker` 的 handler 中抛出，以指定的协议错误类别（如 `APP_NOT_INSTALLED`）结束调用；
/// 其他错误按 `LAUNCH_FAILED`。
public struct WakeFailed: Error, Sendable, Equatable {
    public let kind: String
    public let message: String
    public init(kind: String = "LAUNCH_FAILED", message: String) {
        self.kind = kind
        self.message = message
    }
}

extension AppInfo {
    /// 无已连接实例、但有休眠实例（调用其工具时 Hub 先唤醒）。
    public var isDormant: Bool { !connected && !dormantInstances.isEmpty }
}

extension ToolFormat {
    /// 解析格式名：`mcp`、`openai-chat`（`openai`）、`openai-responses`、`anthropic`、`gemini`。
    public static func parse(_ name: String) throws -> ToolFormat { try parseToolFormat(name: name) }
}

// MARK: - 回调桥接
//
// 原生层以同步回调 + 完成句柄（ApprovalResponder 等）交给桥接对象；桥接在新的 Task 中执行
// 用户的 async handler（需要主线程时 handler 自身标注 @MainActor 即可），再经句柄回传结果。
// 回调线程上不需要任何 Swift 并发上下文。句柄未完成即被释放 → 拒绝 / LAUNCH_FAILED。

private final class ApprovalBridge: ApprovalHandler, @unchecked Sendable {
    let body: @Sendable (ApprovalRequest) async throws -> Bool
    init(_ body: @escaping @Sendable (ApprovalRequest) async throws -> Bool) { self.body = body }
    func onRequest(request: ApprovalRequest, responder: ApprovalResponder) {
        let body = self.body
        Task { _ = responder.complete(approved: (try? await body(request)) ?? false) }
    }
}

private final class PairingBridge: PairingHandler, @unchecked Sendable {
    let body: @Sendable (PairingRequest) async throws -> Bool
    init(_ body: @escaping @Sendable (PairingRequest) async throws -> Bool) { self.body = body }
    func onRequest(request: PairingRequest, responder: PairingResponder) {
        let body = self.body
        Task { _ = responder.complete(approved: (try? await body(request)) ?? false) }
    }
}

private final class WakerBridge: HubWaker, @unchecked Sendable {
    let body: @Sendable (WakeRequest) async throws -> Void
    init(_ body: @escaping @Sendable (WakeRequest) async throws -> Void) { self.body = body }
    func wake(request: WakeRequest, responder: WakeResponder) {
        let body = self.body
        Task {
            do {
                try await body(request)
                _ = responder.succeed()
            } catch let e as WakeFailed {
                _ = responder.fail(kind: e.kind, reason: e.message)
            } catch {
                _ = responder.fail(kind: "LAUNCH_FAILED", reason: "\(error)")
            }
        }
    }
}

/// 把单一的原生监听分发给多个订阅者。
private final class EventHub: HubEventListener, @unchecked Sendable {
    private let lock = NSLock()
    private var subscribers: [UUID: @Sendable (HubEvent) -> Void] = [:]
    private var laggedSubscribers: [UUID: @Sendable (UInt64) -> Void] = [:]

    func add(_ fn: @escaping @Sendable (HubEvent) -> Void) -> UUID {
        let id = UUID()
        lock.lock(); subscribers[id] = fn; lock.unlock()
        return id
    }

    func addLagged(_ fn: @escaping @Sendable (UInt64) -> Void) -> UUID {
        let id = UUID()
        lock.lock(); laggedSubscribers[id] = fn; lock.unlock()
        return id
    }

    func remove(_ id: UUID) {
        lock.lock(); subscribers[id] = nil; laggedSubscribers[id] = nil; lock.unlock()
    }

    func onEvent(event: HubEvent) {
        lock.lock(); let subs = Array(subscribers.values); lock.unlock()
        for fn in subs { fn(event) }
    }

    func onLagged(skipped: UInt64) {
        lock.lock(); let subs = Array(laggedSubscribers.values); lock.unlock()
        for fn in subs { fn(skipped) }
    }
}

/// 取消事件订阅的句柄。
public final class EventSubscription: @unchecked Sendable {
    private let cancelFn: () -> Void
    private let lock = NSLock()
    private var cancelled = false

    init(_ cancel: @escaping () -> Void) { cancelFn = cancel }

    public func cancel() {
        lock.lock()
        let first = !cancelled
        cancelled = true
        lock.unlock()
        if first { cancelFn() }
    }

    deinit { cancel() }
}

// MARK: - Hub

/// 嵌入式 Hub：连接本机所有 App（WebSocket），并把它们的工具提供给自有 LLM 循环。
///
/// 线程：所有方法线程安全；`async` 方法不阻塞调用线程。事件在 Hub 分发线程上产生，
/// 经 `events()`（AsyncStream）或 `onEvent` 送达；审批 / 配对 handler 在 Swift 并发运行时上执行，
/// 需要 UI 时自行 `await MainActor.run { … }`。
public final class Hub: @unchecked Sendable {
    private let inner: AppMcpHub
    private let eventHub = EventHub()
    private let lock = NSLock()
    private var closed = false

    /// 启动 Hub（绑定 App 连接服务并启动后台任务）。
    public init(config: HubConfig = HubConfig()) throws {
        inner = try AppMcpHub.start(config: config)
        inner.setEventListener(listener: eventHub)
    }

    deinit { close() }

    /// Hub 日志输出到 stderr（`RUST_LOG` 语法）。只有第一次调用生效。
    @discardableResult
    public static func initLogging(_ filter: String? = nil) -> Bool { AppMcpHubBindings.initLogging(filter: filter) }

    /// 停止 Hub（断开所有 App、结束后台任务）。可重复调用。
    public func close() {
        lock.lock()
        let first = !closed
        closed = true
        lock.unlock()
        guard first else { return }
        inner.setEventListener(listener: nil)
        inner.shutdown()
    }

    /// HTTP 服务（`/app`、`/healthz`）的实际地址（端口 0 时为随机端口；App 端点为 `ws://<地址>/app`）；未开启时为 `nil`。
    public var listenAddr: String? { inner.listenAddr() }

    /// 本地 IPC 连接服务的端点（`unix:…` / `pipe:…`，可直接作为 App 端 SDK 的 `hostURL`）；未开启时为 `nil`。
    public var ipcEndpoint: String? { inner.ipcEndpoint() }

    // MARK: 查询

    public func apps() -> [AppInfo] { inner.apps() }

    public func tools(_ filter: ToolFilter = ToolFilter()) -> [HubTool] { inner.tools(filter: filter) }

    public func resources() -> [HubResource] { inner.resources() }

    public func overview(appId: String) -> AppOverviewInfo? { inner.overview(appId: appId) }

    /// 运行状态（spec/hub-api.md 3.9，与 `GET /status` 相同）：身份、监听位置、令牌策略、各 App 与实例的状态
    /// （实例带 `info.connectionId`）、最近错误、最近的 SDK 诊断上报。已关闭时抛出 `HubError.Shutdown`。
    public func status() throws -> HubStatus { try inner.status() }

    /// 生效的策略规则、各规则命中次数与最近的加载错误（spec/hub-api.md 3.13）。已关闭时抛 `HubError.Shutdown`。
    public func policy() throws -> PolicyStatus { try inner.policy() }

    /// 替换策略规则集（命中计数清零）；传 `PolicyConfig(rules: [])` 清空。
    /// 规则不合法时抛 `HubError.Tool`（`kind == "INVALID_INPUT"`），之前的规则继续生效，原因记入 `policy().lastError`。
    public func setPolicy(_ policy: PolicyConfig) throws { try inner.setPolicy(policy: policy) }

    /// 替换 Agent 登记（空数组清空），只影响之后到达的 MCP 请求。
    /// 不合法时抛 `HubError.Tool`（`kind == "INVALID_INPUT"`），之前的登记继续生效。
    public func setAgents(_ agents: [AgentCredential]) throws { try inner.setAgents(agents: agents) }

    /// 设置全局默认实例（`nil` 恢复按规则路由）。
    public func selectInstance(appId: String, instanceId: String?) {
        inner.selectInstance(appId: appId, instanceId: instanceId)
    }

    // MARK: 调用

    /// 调用工具（全名 `<appId>.<tool>`，参数为 JSON 对象文本）。
    ///
    /// 工具层面的失败（用户拒绝、超时、App 报错……）放在 `CallResult.error`；名称无法解析时抛出 `HubError`。
    /// Task 被取消时自动取消调用。
    /// `onProgress`：接收调用进度（Hub 合并后，spec/hub-api.md 3.12），在 Hub 的进度线程上按顺序同步调用、须尽快返回
    /// （需要时自行切到主 actor）；全部回调在本函数返回之前完成。
    public func callTool(
        _ name: String,
        argumentsJSON: String? = nil,
        instanceId: String? = nil,
        timeout: TimeInterval? = nil,
        session: String? = nil,
        callId: String? = nil,
        idempotencyKey: String? = nil,
        onProgress: (@Sendable (ProgressUpdate) -> Void)? = nil
    ) async throws -> CallResult {
        let request = CallRequest(
            name: name,
            argumentsJson: argumentsJSON,
            instanceId: instanceId,
            timeoutMs: timeout.map { UInt64(max(0, $0 * 1000)) },
            callId: callId,
            session: session,
            idempotencyKey: idempotencyKey
        )
        let out: CallOutcome
        if let onProgress {
            out = try await inner.callToolWithProgress(request: request, listener: ProgressBridge(onProgress))
        } else {
            out = try await inner.callTool(request: request)
        }
        return CallResult(
            callId: out.callId,
            dataJSON: out.dataJson,
            error: out.error,
            stateHints: out.stateHints,
            instanceId: out.instanceId,
            overview: out.overview,
            status: out.status,
            stateResource: out.stateResource,
            summary: out.summary,
            annotations: out.annotations,
            routedTo: out.routedTo,
            durationMs: out.durationMs,
            woke: out.woke
        )
    }

    /// 调用工具，参数为 `Encodable`（编码为 JSON 对象）。
    public func callTool<Args: Encodable>(
        _ name: String,
        arguments: Args,
        instanceId: String? = nil,
        timeout: TimeInterval? = nil,
        session: String? = nil,
        callId: String? = nil,
        idempotencyKey: String? = nil,
        onProgress: (@Sendable (ProgressUpdate) -> Void)? = nil
    ) async throws -> CallResult {
        let json = String(decoding: try JSONEncoder().encode(arguments), as: UTF8.self)
        return try await callTool(
            name, argumentsJSON: json, instanceId: instanceId, timeout: timeout, session: session, callId: callId,
            idempotencyKey: idempotencyKey, onProgress: onProgress
        )
    }

    public func cancelCall(_ callId: String) { inner.cancelCall(callId: callId) }

    public func readResource(_ uri: String) async throws -> ResourceContent { try await inner.readResource(uri: uri) }

    public func subscribe(_ uri: String) throws { try inner.subscribe(uri: uri) }

    public func unsubscribe(_ uri: String) { inner.unsubscribe(uri: uri) }

    // MARK: LLM 工具格式

    /// 导出工具定义（JSON 文本），直接放进 LLM 请求的 `tools`。
    public func exportTools(_ format: ToolFormat, filter: ToolFilter = ToolFilter()) -> String {
        inner.exportTools(format: format, filter: filter)
    }

    /// 执行模型发出的一个工具调用（该格式的 JSON 文本），返回应回填给模型的 JSON 文本。
    public func dispatch(_ format: ToolFormat, toolCall: String, session: String? = nil) async throws -> String {
        try await inner.dispatch(format: format, toolCallJson: toolCall, session: session)
    }

    /// 清除会话状态（首次接触总览、会话内 `apps.select`）。
    public func resetSession(_ session: String? = nil) { inner.resetSession(session: session) }

    /// 同时以 MCP Streamable HTTP 对外提供，返回实际地址。
    public func serveHTTP(_ addr: String, allowRemote: Bool = false) async throws -> String {
        try await inner.serveHttp(addr: addr, allowRemote: allowRemote)
    }

    // MARK: 回调

    /// 调用确认（风险不低于 `approvalMinRisk` 时询问）。返回 `false` 或抛出错误 → `USER_REJECTED`。
    public func setApprovalHandler(_ handler: @escaping @Sendable (ApprovalRequest) async throws -> Bool) {
        inner.setApprovalHandler(handler: ApprovalBridge(handler))
    }

    /// App 配对确认。返回 `false` 或抛出错误 → 拒绝。
    public func setPairingHandler(_ handler: @escaping @Sendable (PairingRequest) async throws -> Bool) {
        inner.setPairingHandler(handler: PairingBridge(handler))
    }

    /// 自定义唤醒（spec/hub-api.md 3.5），替换默认的系统唤醒实现；`nil` 恢复默认。
    ///
    /// 调用休眠实例的工具（或未运行而清单声明了 wake 的 App）时，Hub 生成一次性令牌并调用 handler：
    /// 正常返回表示已发出激活（Hub 随后等待 App 回连，`HubConfig.wakeTimeoutMs`）；抛出 `WakeFailed`
    /// 以指定类别结束调用，其他错误按 `LAUNCH_FAILED`。
    public func setWaker(_ handler: (@Sendable (WakeRequest) async throws -> Void)?) {
        inner.setWaker(waker: handler.map { WakerBridge($0) })
    }

    /// 订阅事件（在 Hub 分发线程上同步回调，须尽快返回）。释放或 `cancel()` 返回值即取消订阅。
    public func onEvent(_ callback: @escaping @Sendable (HubEvent) -> Void) -> EventSubscription {
        let id = eventHub.add(callback)
        return EventSubscription { [eventHub] in eventHub.remove(id) }
    }

    /// 事件处理过慢被跳过时回调（参数为跳过的数量）；收到后应重新拉取 `apps()` / `tools()`。
    public func onLagged(_ callback: @escaping @Sendable (UInt64) -> Void) -> EventSubscription {
        let id = eventHub.addLagged(callback)
        return EventSubscription { [eventHub] in eventHub.remove(id) }
    }

    /// 事件流（创建时即开始接收；迭代结束或 Task 取消时取消订阅）。
    public func events(bufferingNewest limit: Int = 1024) -> AsyncStream<HubEvent> {
        AsyncStream(bufferingPolicy: .bufferingNewest(limit)) { continuation in
            let id = eventHub.add { continuation.yield($0) }
            continuation.onTermination = { [eventHub] _ in eventHub.remove(id) }
        }
    }
}

/// 进度回调桥：在 Hub 的进度线程上按顺序同步调用 `onProgress`（须尽快返回；全部回调在 `callTool` 返回前完成）。
final class ProgressBridge: ProgressListener, @unchecked Sendable {
    let action: @Sendable (ProgressUpdate) -> Void
    init(_ action: @escaping @Sendable (ProgressUpdate) -> Void) { self.action = action }
    func onProgress(update: ProgressUpdate) { action(update) }
}
