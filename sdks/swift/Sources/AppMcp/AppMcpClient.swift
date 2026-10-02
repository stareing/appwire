import AppMcpBindings
import Foundation

// MARK: - 执行

/// handler 在哪里执行。
enum ExecutionTarget {
    /// 主 actor（UI 线程）。
    case mainActor
    /// 协作线程池。
    case background
}

/// 保证一次调用只“开始”或“超时放弃”其中之一。
final class Gate: @unchecked Sendable {
    private let lock = NSLock()
    private var state = 0 // 0 = 排队，1 = 执行中，2 = 已放弃

    func begin() -> Bool { transition(to: 1) }
    func abandon() -> Bool { transition(to: 2) }

    private func transition(to next: Int) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard state == 0 else { return false }
        state = next
        return true
    }
}

/// 失败回调：错误类别、消息、详情（JSON 文本，可为空）。
typealias FailFn = @Sendable (String, String, String?) -> Void

/// 启动执行 Task：错误映射为错误类别；超时未开始执行时以 `APP_NOT_RESPONDING` 失败。
func launch(
    on target: ExecutionTarget,
    timeout: TimeInterval?,
    fail: @escaping FailFn,
    body: @escaping @Sendable () async throws -> Void
) -> Task<Void, Never> {
    let gate = Gate()
    if let timeout, timeout > 0 {
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
            if gate.abandon() {
                fail(ErrorKind.appNotResponding, "\(timeout) 秒内未能在目标线程上开始执行", nil)
            }
        }
    }
    @Sendable func run() async {
        do {
            try await body()
        } catch is CancellationError {
            fail(ErrorKind.cancelled, "调用已取消", nil)
        } catch let e as ToolCallError {
            fail(e.kind, e.message, e.details.flatMap { try? encodeJSON($0) })
        } catch let e as DecodingError {
            fail(ErrorKind.invalidInput, "参数解码失败：\(e)", nil)
        } catch {
            fail(ErrorKind.handlerError, "\(error)", nil)
        }
    }
    switch target {
    case .mainActor:
        return Task { @MainActor in
            guard gate.begin() else { return }
            await run()
        }
    case .background:
        return Task.detached {
            guard gate.begin() else { return }
            await run()
        }
    }
}

let jsonEncoder: JSONEncoder = {
    let e = JSONEncoder()
    e.outputFormatting = [.sortedKeys]
    return e
}()

func encodeJSON<T: Encodable>(_ value: T) throws -> String {
    do {
        return String(decoding: try jsonEncoder.encode(value), as: UTF8.self)
    } catch {
        throw ToolCallError(ErrorKind.handlerError, "返回值无法编码为 JSON：\(error)")
    }
}

func decodeJSON<T: Decodable>(_ type: T.Type, _ text: String) throws -> T {
    try JSONDecoder().decode(type, from: Data((text.isEmpty ? "{}" : text).utf8))
}

private func failQuietly(_ call: Call, _ kind: String, _ message: String, _ details: String? = nil) {
    do {
        try call.failWithDetails(kind: kind, message: message, detailsJson: details)
    } catch AppMcpError.UnknownErrorKind {
        failQuietly(call, ErrorKind.handlerError, message, details)
    } catch AppMcpError.InvalidJson {
        failQuietly(call, kind, message, nil)
    } catch {
        // AlreadyCompleted：已取消或超时，忽略
    }
}

private func failQuietly(_ read: Read, _ kind: String, _ message: String, _ details: String? = nil) {
    do {
        try read.failWithDetails(kind: kind, message: message, detailsJson: details)
    } catch AppMcpError.UnknownErrorKind {
        failQuietly(read, ErrorKind.handlerError, message, details)
    } catch AppMcpError.InvalidJson {
        failQuietly(read, kind, message, nil)
    } catch {
        // AlreadyCompleted：已取消或超时，忽略
    }
}

/// 类型擦除后的工具实现：参数 JSON → 原生调用结果（`dataJson` 为 `nil` 表示 null）。
typealias ErasedTool = @Sendable (String, ToolContext) async throws -> CallResult

/// 普通返回值对应的调用结果。
func plainResult(_ dataJson: String?) -> CallResult {
    CallResult(dataJson: dataJson, stateHints: [], status: .done)
}

final class CancelBridge: CancelListener, @unchecked Sendable {
    private let action: @Sendable (CancelReason) -> Void
    init(_ action: @escaping @Sendable (CancelReason) -> Void) { self.action = action }
    func onCancel(reason: CancelReason) { action(reason) }
}

final class ToolBridge: ToolHandler, @unchecked Sendable {
    let target: ExecutionTarget
    let timeout: TimeInterval?
    let body: ErasedTool

    init(target: ExecutionTarget, timeout: TimeInterval?, body: @escaping ErasedTool) {
        self.target = target
        self.timeout = timeout
        self.body = body
    }

    // 在原生分发线程上调用：只做登记，立即返回。
    func invoke(call: Call) {
        let ctx = ToolContext(callId: call.callId(), toolName: call.toolName(), argumentsJSON: call.argumentsJson(), call: call,
                              idempotencyKey: call.idempotencyKey())
        let body = self.body
        let task = launch(on: target, timeout: timeout, fail: { failQuietly(call, $0, $1, $2) }) {
            var result = try await body(ctx.argumentsJSON, ctx)
            result.stateHints += ctx.stateHints
            do {
                try call.completeWith(result: result)
            } catch AppMcpError.AlreadyCompleted {
            } catch {
                failQuietly(call, ErrorKind.handlerError, "提交结果失败：\(error)")
            }
        }
        call.setCancelListener(listener: CancelBridge { reason in
            ctx.markCancelled(reason)
            task.cancel()
        })
    }
}

final class NavigationBridge: NavigationHandler, @unchecked Sendable {
    let timeout: TimeInterval?
    let body: NavigateFunction

    init(timeout: TimeInterval?, body: @escaping NavigateFunction) {
        self.timeout = timeout
        self.body = body
    }

    // 在原生分发线程上调用：只做登记，立即返回。
    func navigate(request: Navigate) {
        let req = NavigationRequest(page: request.page(), paramsJSON: request.paramsJson())
        let body = self.body
        _ = launch(on: .mainActor, timeout: timeout, fail: { kind, message, details in
            finishNavigate(request, navigationFailure(kind: kind, message: message, detailsJSON: details))
        }) {
            finishNavigate(request, try await body(req))
        }
    }
}

private func finishNavigate(_ request: Navigate, _ result: NavigationResult) {
    do {
        switch result {
        case .ok: try request.complete()
        case let .denied(message): try request.deny(message: message)
        case let .failed(message): try request.fail(message: message)
        case let .userActionRequired(message, reason, uri):
            try request.failUserAction(message: message, reason: reason, uri: uri)
        }
    } catch {
        // 已完成或连接已断开：回复被丢弃（spec/protocol.md 3.4）
    }
}

final class ReaderBridge: ResourceReader, @unchecked Sendable {
    let target: ExecutionTarget
    let timeout: TimeInterval?
    let body: @Sendable () async throws -> String

    init(target: ExecutionTarget, timeout: TimeInterval?, body: @escaping @Sendable () async throws -> String) {
        self.target = target
        self.timeout = timeout
        self.body = body
    }

    func read(read: Read) {
        let body = self.body
        _ = launch(on: target, timeout: timeout, fail: { failQuietly(read, $0, $1, $2) }) {
            let json = try await body()
            do {
                try read.complete(contentsJson: json)
            } catch AppMcpError.AlreadyCompleted {}
        }
    }
}

// MARK: - 句柄

/// 已注册的工具。
public final class ToolHandle: @unchecked Sendable {
    let inner: Tool
    private var spec: ToolSpec
    private let lock = NSLock()

    init(inner: Tool, spec: ToolSpec) {
        self.inner = inner
        self.spec = spec
    }

    public var name: String { inner.name() }

    public func setEnabled(_ enabled: Bool) throws {
        lock.lock()
        defer { lock.unlock() }
        try inner.setEnabled(enabled: enabled)
        // update 整体替换定义：记住启用状态，否则之后的 update 会把它改回去（view 工具随可见性切换）
        spec.enabled = enabled
    }

    /// 按补丁修改定义：闭包里改动的字段替换，**设为 `nil` 清除该声明**，没改动的保持不变（与网页 / Rust SDK 一致）。
    ///
    /// ```swift
    /// try handle.update { $0.description = "新描述"; $0.annotations = nil; $0.outputSchema = nil }
    /// ```
    public func update(_ change: (inout ToolDeclaration) -> Void) throws {
        lock.lock()
        defer { lock.unlock() }
        var decl = ToolDeclaration(spec)
        change(&decl)
        let next = decl.applied(to: spec)
        try inner.update(spec: next)
        spec = next
    }

    /// 修改定义；为 `nil` 的参数保持不变（无法清除声明——清除用 `update { $0.annotations = nil }`）。
    public func update(
        description: String? = nil,
        inputSchema: String? = nil,
        risk: Risk? = nil,
        title: String? = nil,
        annotations: ToolAnnotations? = nil,
        outputSchema: String? = nil,
        surface: ToolSurface? = nil,
        page: String? = nil,
        backgroundTool: String? = nil
    ) throws {
        try update { d in
            if let description { d.description = description }
            if let inputSchema { d.inputSchema = inputSchema }
            if let risk { d.risk = risk }
            if let title { d.title = title }
            if let annotations { d.annotations = annotations }
            if let outputSchema { d.outputSchema = outputSchema }
            if let surface { d.surface = surface }
            if let page { d.page = page }
            if let backgroundTool { d.backgroundTool = backgroundTool }
        }
    }

    /// 注销工具（幂等）。
    public func dispose() { inner.dispose() }
}

/// 已注册的资源。
public final class ResourceHandle: @unchecked Sendable {
    let inner: Resource
    init(inner: Resource) { self.inner = inner }

    public var name: String { inner.name() }
    public func notifyChanged() throws { try inner.notifyChanged() }
    public func dispose() { inner.dispose() }
}

// MARK: - 注册入口

/// 工具 / 资源 / 子作用域的注册入口，`AppMcpClient` 与 `ToolScope` 共用。
public class ToolRegistrar: @unchecked Sendable {
    let dispatchTimeout: TimeInterval?

    init(dispatchTimeout: TimeInterval?) {
        self.dispatchTimeout = dispatchTimeout
    }

    func registerRaw(_ spec: ToolSpec, _ handler: ToolHandler) throws -> Tool { fatalError("子类实现") }
    func registerRaw(_ spec: ResourceSpec, _ reader: ResourceReader) throws -> Resource { fatalError("子类实现") }
    func createRaw(_ name: String) throws -> AppMcpBindings.Scope { fatalError("子类实现") }

    func register(
        _ name: String, _ description: String, _ inputSchema: String?, _ risk: Risk, _ activation: Activation?,
        _ title: String?, _ enabled: Bool, _ annotations: ToolAnnotations?, _ outputSchema: String?,
        _ surface: ToolSurface, _ page: String?, _ backgroundTool: String?,
        _ target: ExecutionTarget, _ body: @escaping ErasedTool
    ) throws -> ToolHandle {
        let spec = ToolSpec(
            name: name, description: description, inputSchemaJson: inputSchema, risk: risk,
            activation: activation, title: title, enabled: enabled,
            annotations: annotations, outputSchemaJson: outputSchema,
            surface: surface == .app ? nil : surface, page: page, backgroundTool: backgroundTool
        )
        let raw = try registerRaw(spec, ToolBridge(target: target, timeout: dispatchTimeout, body: body))
        return ToolHandle(inner: raw, spec: spec)
    }

    /// 注册工具，handler 在**主 actor** 上执行。参数按 `Args` 解码（失败 → `INVALID_INPUT`），返回值按 JSON 编码。
    ///
    /// ```swift
    /// try client.tool("cart.add", description: "加入购物车", inputSchema: schema) { (args: AddItem, ctx) in
    ///     cart.add(args.sku, qty: args.qty)
    ///     ctx.addStateHint("cart")
    ///     return cart.snapshot
    /// }
    /// ```
    ///
    /// `risk` 为旧写法，优先用 `annotations`（标准 MCP 工具注解，原样转发给 Agent；为空时 Hub 按 `risk` 推导）；
    /// `outputSchema` 为结果的 JSON Schema 文本（MCP `outputSchema`）。返回 `ToolResult` 见下方重载。
    /// `surface: .view` = 依赖界面（spec/protocol.md 3.4），只在所在界面可见时启用（SwiftUI 用 `.viewTool(handle)`）；
    /// `page` 为所在页面名，Hub 在该工具未注册时据此导航（`setNavigationHandler`）。
    /// `backgroundTool` 为后台替身（只对 `.view` 工具有意义）：同 App 内一个 `.app` 工具的名称，App 在后台、本工具不可调用时
    /// Hub 改调该工具（spec/protocol.md 3.4「后台与前台」）。
    @discardableResult
    public func tool<Args: Decodable, Output: Encodable>(
        _ name: String,
        description: String,
        inputSchema: String? = nil,
        risk: Risk = .write,
        activation: Activation? = nil,
        title: String? = nil,
        enabled: Bool = true,
        annotations: ToolAnnotations? = nil,
        outputSchema: String? = nil,
        surface: ToolSurface = .app,
        page: String? = nil,
        backgroundTool: String? = nil,
        handler: @escaping @MainActor (Args, ToolContext) async throws -> Output
    ) throws -> ToolHandle {
        try register(name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool, .mainActor) { json, ctx in
            let args = try decodeJSON(Args.self, json)
            let output = try await handler(args, ctx)
            return plainResult(try encodeJSON(output))
        }
    }

    /// 同上，handler 返回结构化结果（业务状态、摘要、内容标注）。
    @discardableResult
    public func tool<Args: Decodable, Output: Encodable>(
        _ name: String,
        description: String,
        inputSchema: String? = nil,
        risk: Risk = .write,
        activation: Activation? = nil,
        title: String? = nil,
        enabled: Bool = true,
        annotations: ToolAnnotations? = nil,
        outputSchema: String? = nil,
        surface: ToolSurface = .app,
        page: String? = nil,
        backgroundTool: String? = nil,
        handler: @escaping @MainActor (Args, ToolContext) async throws -> ToolResult<Output>
    ) throws -> ToolHandle {
        try register(name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool, .mainActor) { json, ctx in
            let args = try decodeJSON(Args.self, json)
            return try await handler(args, ctx).ffi()
        }
    }

    /// 同上，handler 无返回值（结果为 `null`）。
    @discardableResult
    public func tool<Args: Decodable>(
        _ name: String,
        description: String,
        inputSchema: String? = nil,
        risk: Risk = .write,
        activation: Activation? = nil,
        title: String? = nil,
        enabled: Bool = true,
        annotations: ToolAnnotations? = nil,
        outputSchema: String? = nil,
        surface: ToolSurface = .app,
        page: String? = nil,
        backgroundTool: String? = nil,
        handler: @escaping @MainActor (Args, ToolContext) async throws -> Void
    ) throws -> ToolHandle {
        try register(name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool, .mainActor) { json, ctx in
            let args = try decodeJSON(Args.self, json)
            try await handler(args, ctx)
            return plainResult(nil)
        }
    }

    /// 注册工具，handler 在协作线程池上执行（不涉及界面的 handler）。
    @discardableResult
    public func backgroundTool<Args: Decodable, Output: Encodable>(
        _ name: String,
        description: String,
        inputSchema: String? = nil,
        risk: Risk = .write,
        activation: Activation? = nil,
        title: String? = nil,
        enabled: Bool = true,
        annotations: ToolAnnotations? = nil,
        outputSchema: String? = nil,
        surface: ToolSurface = .app,
        page: String? = nil,
        backgroundTool: String? = nil,
        handler: @escaping @Sendable (Args, ToolContext) async throws -> Output
    ) throws -> ToolHandle {
        try register(name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool, .background) { json, ctx in
            let args = try decodeJSON(Args.self, json)
            return plainResult(try encodeJSON(try await handler(args, ctx)))
        }
    }

    /// 同上，handler 返回结构化结果（业务状态、摘要、内容标注）。
    @discardableResult
    public func backgroundTool<Args: Decodable, Output: Encodable>(
        _ name: String,
        description: String,
        inputSchema: String? = nil,
        risk: Risk = .write,
        activation: Activation? = nil,
        title: String? = nil,
        enabled: Bool = true,
        annotations: ToolAnnotations? = nil,
        outputSchema: String? = nil,
        surface: ToolSurface = .app,
        page: String? = nil,
        backgroundTool: String? = nil,
        handler: @escaping @Sendable (Args, ToolContext) async throws -> ToolResult<Output>
    ) throws -> ToolHandle {
        try register(name, description, inputSchema, risk, activation, title, enabled, annotations, outputSchema, surface, page, backgroundTool, .background) { json, ctx in
            let args = try decodeJSON(Args.self, json)
            return try await handler(args, ctx).ffi()
        }
    }

    /// 注册资源，读取函数在主 actor 上执行。
    ///
    /// `realtime`：需实时推送（spec/lifecycle.md 第 13 节 B3）——被订阅时阻止休眠、休眠中变化时回连推送；
    /// 默认 `false`：订阅不阻止休眠，变化在下次连接时补发。
    /// `annotations`：资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上。
    /// 读取函数抛出 `ToolCallError`（含 `ToolCallError.userActionRequired`）时类别与详情原样交给 Host。
    @discardableResult
    public func resource<Output: Encodable>(
        _ name: String,
        description: String,
        mimeType: String? = nil,
        realtime: Bool = false,
        annotations: ContentAnnotations? = nil,
        reader: @escaping @MainActor () async throws -> Output
    ) throws -> ResourceHandle {
        let bridge = ReaderBridge(target: .mainActor, timeout: dispatchTimeout) {
            try encodeJSON(try await reader())
        }
        let spec = ResourceSpec(
            name: name, description: description, mimeType: mimeType, realtime: realtime, annotations: annotations
        )
        let raw = try registerRaw(spec, bridge)
        return ResourceHandle(inner: raw)
    }

    /// 创建子作用域；`dispose()` 时注销其下全部工具与资源。
    public func scope(_ name: String) throws -> ToolScope {
        ToolScope(inner: try createRaw(name), dispatchTimeout: dispatchTimeout)
    }
}

/// 作用域（如一个页面）。`dispose()` 时注销其下全部工具、资源与子作用域。
public final class ToolScope: ToolRegistrar, @unchecked Sendable {
    let inner: AppMcpBindings.Scope

    init(inner: AppMcpBindings.Scope, dispatchTimeout: TimeInterval?) {
        self.inner = inner
        super.init(dispatchTimeout: dispatchTimeout)
    }

    override func registerRaw(_ spec: ToolSpec, _ handler: ToolHandler) throws -> Tool {
        try inner.registerTool(spec: spec, handler: handler)
    }

    override func registerRaw(_ spec: ResourceSpec, _ reader: ResourceReader) throws -> Resource {
        try inner.registerResource(spec: spec, reader: reader)
    }

    override func createRaw(_ name: String) throws -> AppMcpBindings.Scope {
        try inner.createScope(name: name)
    }

    public func dispose() { inner.dispose() }
}

// MARK: - 客户端

final class ListenerBridge: ClientListener, @unchecked Sendable {
    let config: AppMcpConfig
    init(_ config: AppMcpConfig) { self.config = config }

    func onStateChanged(state: StateInfo) { config.onStateChange?(state) }
    func onPaired(token: String) { config.onPaired?(token) }
    func onLog(level: LogLevel, message: String) { config.onLog?(level, message) }
    func onIdleExit() {
        guard let handler = config.onIdleExit else { return }
        Task { @MainActor in handler() }
    }
}

/// app-mcp 客户端。
///
/// 线程模型：原生运行时在分发线程上同步回调；这里在回调中启动 `Task`（默认 `@MainActor`，
/// `backgroundTool` 为协作线程池），完成后提交结果。Host 取消 / 超时 / 断线会取消该 Task。
///
/// 生命周期：handler 闭包由原生层强引用，闭包中引用视图模型等对象时用 `[weak self]` 避免循环引用。
public final class AppMcpClient: ToolRegistrar, @unchecked Sendable {
    let inner: AppMcpBindings.AppMcpClient
    /// 生效的生命周期策略（配置为空时为 `LifecyclePolicy.platformDefault`）。
    public let lifecycle: LifecyclePolicy

    public init(config: AppMcpConfig) throws {
        let lifecycle = config.lifecycle ?? .platformDefault
        inner = try AppMcpBindings.AppMcpClient(config: config.ffi(lifecycle: lifecycle), listener: ListenerBridge(config))
        if let navigateInBackground = config.navigateInBackground {
            inner.setNavigateInBackground(enabled: navigateInBackground)
        }
        self.lifecycle = lifecycle
        super.init(dispatchTimeout: config.dispatchTimeout)
    }

    deinit {
        inner.stop()
    }

    override func registerRaw(_ spec: ToolSpec, _ handler: ToolHandler) throws -> Tool {
        try inner.registerTool(spec: spec, handler: handler)
    }

    override func registerRaw(_ spec: ResourceSpec, _ reader: ResourceReader) throws -> Resource {
        try inner.registerResource(spec: spec, reader: reader)
    }

    override func createRaw(_ name: String) throws -> AppMcpBindings.Scope {
        try inner.createScope(name: name)
    }

    public var instanceId: String { inner.instanceId() }
    public var state: StateInfo { inner.state() }
    /// 当前 token（配置带入的或配对后获得的）。
    public var token: String? { inner.token() }
    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 `cid` 对应；未连接时为 nil。
    public var connectionId: String? { inner.connectionId() }

    /// 开始连接 Host（重复调用无效果）。
    public func start() { inner.start() }

    /// 停止：取消所有调用、断开连接、不再重连。
    public func stop() { inner.stop() }

    public func setVisibility(_ visibility: Visibility, focused: Bool = true) {
        inner.setVisibility(visibility: visibility, focused: focused)
    }

    /// 后台时是否仍把导航请求交给导航回调（spec/protocol.md 3.4「后台与前台」），随时生效；初值见
    /// `AppMcpConfig.navigateInBackground`。为 `false` 时 App 不可见（`.hidden` / `.frozen`）收到的导航立即以
    /// `USER_ACTION_REQUIRED`（reason `foreground`）回复，不调用回调。
    public func setNavigateInBackground(_ enabled: Bool) {
        inner.setNavigateInBackground(enabled: enabled)
    }

    /// 设置导航回调（Host 的 `app/navigate`，spec/protocol.md 3.4）；`nil` 清除（之后的导航请求以 `NAVIGATION_FAILED`
    /// 回复）。回调在**主 actor** 上执行，可直接改 `NavigationPath` 等界面状态；抛出的错误按失败回复
    /// （`ToolCallError` 的类别为 `NAVIGATION_DENIED` 时按拒绝，`ToolCallError.userActionRequired` 按需要用户操作）。
    ///
    /// 能力在握手时声明：建议在 `start()` 之前设置；连接后才设置的回调在下次连接（回连 / 唤醒）时生效。
    /// SwiftUI 的 `NavigationStack` 适配见 `NavigationRouter`。
    public func setNavigationHandler(_ handler: NavigateFunction?) {
        inner.setNavigationHandler(handler: handler.map { NavigationBridge(timeout: dispatchTimeout, body: $0) })
    }

    // MARK: 生命周期（spec/lifecycle.md 第 8 节）

    /// 处理操作系统激活参数（命令行、D-Bus action 参数等）：识别 `app-mcp-wake:<token>`、
    /// `<scheme>://app-mcp/wake?token=`、`#app-mcp-wake=<token>`。不是本 SDK 的唤醒返回 `false`。
    /// 可以在 `start()` 之前调用（冷启动唤醒）。
    @discardableResult
    public func handleWake(_ args: String) -> Bool { inner.handleWake(args: args) }

    /// 处理 `onOpenURL` / `application(_:open:options:)` 收到的 URL。不是唤醒 URL 返回 `false`。
    @discardableResult
    public func handleWake(url: URL) -> Bool { inner.handleWake(args: url.absoluteString) }

    /// App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。
    @discardableResult
    public func wake(reason: WakeReason = .app) -> Bool { inner.wakeWithReason(reason: reason) }

    /// `onDemand` 模式下主动连接；尚未 `start()` 时等同于 `start()`。
    @discardableResult
    public func connectNow() -> Bool { inner.connectNow() }

    /// App 主动请求休眠（不受空闲条件与持有影响）。返回是否有效果。
    @discardableResult
    public func sleep(reason: SleepReason = .app) -> Bool { inner.sleepWithReason(reason: reason) }

    /// 临时阻止自动休眠，直到返回的持有被释放（`release()` 或对象释放）。
    public func hold() -> SleepHold { SleepHold(inner.hold()) }

    /// 当前工具与资源定义的摘要（16 个十六进制字符）。
    public var toolsHash: String { inner.toolsHash() }

    /// 从激活参数 / URL 中提取唤醒令牌；不是唤醒参数返回 `nil`。
    public static func wakeToken(in args: String) -> String? { parseWakeToken(args: args) }
}
