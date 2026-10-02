import AppMcpBindings
import Foundation

/// 工具对界面的依赖（spec/protocol.md 3.4）：`.app`（缺省）后台可调、可唤醒；`.view` 只在所在界面可见且处于最上层时启用。
public typealias ToolSurface = AppMcpBindings.ToolSurface

/// 导航回调的结果（Host 的 `app/navigate`，spec/protocol.md 3.4）。
public enum NavigationResult: Sendable, Equatable {
    /// 已切换到目标页面（最好在新页面的工具注册之后再返回，Host 会等待目标工具出现）。
    case ok
    /// 不愿切换（用户正在输入、页面需要登录等）→ `NAVIGATION_DENIED`；消息面向模型 / 用户。
    case denied(String)
    /// 页面不存在、参数不合法等 → `NAVIGATION_FAILED`。
    case failed(String)
}

/// 一次导航请求。
public struct NavigationRequest: Sendable, Equatable {
    /// 目标页面名（清单 `pages[].name` 或工具声明的 `page`）。
    public let page: String
    /// 页面参数（JSON 文本）；Host 没有给出时为 `nil`。
    public let paramsJSON: String?

    public init(page: String, paramsJSON: String?) {
        self.page = page
        self.paramsJSON = paramsJSON
    }

    /// 按 `T` 解码页面参数；没有参数时为 `nil`。
    public func params<T: Decodable>(_ type: T.Type) throws -> T? {
        guard let paramsJSON else { return nil }
        return try JSONDecoder().decode(type, from: Data(paramsJSON.utf8))
    }
}

/// 导航回调：在主 actor 上执行；抛出的错误按失败回复。
public typealias NavigateFunction = @MainActor @Sendable (NavigationRequest) async throws -> NavigationResult

/// `ToolHandle.update(_:)` 闭包里可改的工具声明；设为 `nil` 即清除该声明。
public struct ToolDeclaration {
    public var description: String
    /// `nil` = 无参数。
    public var inputSchema: String?
    /// `nil` = 缺省风险（`write`）。
    public var risk: Risk?
    public var activation: Activation?
    public var title: String?
    /// `nil` = 未声明（Hub 按 `risk` 推导）。
    public var annotations: ToolAnnotations?
    public var outputSchema: String?
    public var surface: ToolSurface
    public var page: String?

    init(_ spec: ToolSpec) {
        description = spec.description
        inputSchema = spec.inputSchemaJson
        risk = spec.risk
        activation = spec.activation
        title = spec.title
        annotations = spec.annotations
        outputSchema = spec.outputSchemaJson
        surface = spec.surface ?? .app
        page = spec.page
    }

    func applied(to spec: ToolSpec) -> ToolSpec {
        var next = spec
        next.description = description
        next.inputSchemaJson = inputSchema
        next.risk = risk
        next.activation = activation
        next.title = title
        next.annotations = annotations
        next.outputSchemaJson = outputSchema
        next.surface = surface == .app ? nil : surface
        next.page = page
        return next
    }
}

/// 按页面名分派的导航表：各框架适配（`NavigationRouter`、UIKit 导航控制器等）只需登记"页面 → 怎样切过去"。
/// 未登记的页面以 `.failed` 回复。
@MainActor
public final class PageRouter {
    public typealias Navigate = @MainActor (NavigationRequest) async throws -> NavigationResult
    private var pages: [String: Navigate] = [:]

    public nonisolated init() {}

    /// 登记页面；闭包正常返回即 `.ok`。同名覆盖。
    @discardableResult
    public func page(_ name: String, _ navigate: @escaping @MainActor (NavigationRequest) async throws -> Void) -> PageRouter {
        pages[name] = { request in
            try await navigate(request)
            return .ok
        }
        return self
    }

    /// 登记页面，由闭包决定结果（如未登录时返回 `.denied`）。
    @discardableResult
    public func pageWithResult(_ name: String, _ navigate: @escaping Navigate) -> PageRouter {
        pages[name] = navigate
        return self
    }

    public var pageNames: Set<String> { Set(pages.keys) }

    public func navigate(_ request: NavigationRequest) async throws -> NavigationResult {
        guard let navigate = pages[request.page] else { return .failed("未知页面：\(request.page)") }
        return try await navigate(request)
    }

    /// 作为 `AppMcpClient.setNavigationHandler(_:)` 的回调。
    public nonisolated var handler: NavigateFunction {
        { [self] request in try await self.navigate(request) }
    }
}
