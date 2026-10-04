// 由 app-mcp-codegen 生成（target: swift），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果

import Foundation

/// 按状态列出订单（旧版）
public struct OrdersListParams: Codable, Sendable, Equatable {
    /// 订单状态
    @available(*, deprecated, message: "参数已弃用（inputSchema 中 deprecated: true）")
    public var status: String {
        get { _status }
        set { _status = newValue }
    }
    private var _status: String
    /// 页码
    /// 取值范围：≥ 1
    @available(*, deprecated, message: "参数已弃用（inputSchema 中 deprecated: true）")
    public var page: Int? {
        get { _page }
        set { _page = newValue }
    }
    private var _page: Int?
    /// 最多返回条数
    public var limit: Int?

    public init(status: String, page: Int? = nil, limit: Int? = nil) {
        self._status = status
        self._page = page
        self.limit = limit
    }

    enum CodingKeys: String, CodingKey {
        case _status = "status"
        case _page = "page"
        case limit = "limit"
    }
}

/// 过滤条件
public struct OrdersFindFilter: Codable, Sendable, Equatable {
    /// 关键词
    public var keyword: String?
    /// 旧标签
    @available(*, deprecated, message: "参数已弃用（inputSchema 中 deprecated: true）")
    public var legacyTag: String? {
        get { _legacyTag }
        set { _legacyTag = newValue }
    }
    private var _legacyTag: String?

    public init(keyword: String? = nil, legacyTag: String? = nil) {
        self.keyword = keyword
        self._legacyTag = legacyTag
    }

    enum CodingKeys: String, CodingKey {
        case keyword = "keyword"
        case _legacyTag = "legacyTag"
    }
}

/// 按状态查找订单
public struct OrdersFindParams: Codable, Sendable, Equatable {
    /// 订单状态
    public var state: String
    /// 分页游标
    public var cursor: String?
    /// 过滤条件
    public var filter: OrdersFindFilter?

    public init(state: String, cursor: String? = nil, filter: OrdersFindFilter? = nil) {
        self.state = state
        self.cursor = cursor
        self.filter = filter
    }

    enum CodingKeys: String, CodingKey {
        case state = "state"
        case cursor = "cursor"
        case filter = "filter"
    }
}

/// 清空购物车（旧版）
public struct CartLegacyClearParams: Codable, Sendable, Equatable {
    public init() {}
}

/// 弃用示例 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
/// 返回值会编码为 JSON 作为工具结果（返回 String 时原样作为文本）。
public protocol LegacyToolHandlers: Sendable {
    /// 按状态列出订单（旧版）
    ///
    /// 工具 `orders.list`「列出订单」，风险：read
    @available(*, deprecated, message: "旧版 \"列表\" 接口 */ 不再维护：$x ${y} \\(z) 'q' <b>&amp; #{w}\n请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）")
    func ordersList(_ params: OrdersListParams) async throws -> any Encodable & Sendable
    /// 按状态查找订单
    ///
    /// 工具 `orders.find`「查找订单」，风险：read
    func ordersFind(_ params: OrdersFindParams) async throws -> any Encodable & Sendable
    /// 清空购物车（旧版）
    ///
    /// 工具 `cart.legacyClear`「清空购物车」，风险：write
    @available(*, deprecated, message: "改用 cart.clear")
    func cartLegacyClear(_ params: CartLegacyClearParams) async throws -> any Encodable & Sendable
}

/// 生成代码调用已弃用 handler 的转发协议（Swift 没有局部关闭弃用警告的写法）。
protocol LegacyDeprecatedToolCalls {
    func ordersList(_ handlers: any LegacyToolHandlers, _ params: OrdersListParams) async throws -> any Encodable & Sendable
    func cartLegacyClear(_ handlers: any LegacyToolHandlers, _ params: CartLegacyClearParams) async throws -> any Encodable & Sendable
}

struct LegacyDeprecatedToolCaller: LegacyDeprecatedToolCalls {
    static var shared: any LegacyDeprecatedToolCalls { LegacyDeprecatedToolCaller() }

    @available(*, deprecated)
    func ordersList(_ handlers: any LegacyToolHandlers, _ params: OrdersListParams) async throws -> any Encodable & Sendable {
        try await handlers.ordersList(params)
    }

    @available(*, deprecated)
    func cartLegacyClear(_ handlers: any LegacyToolHandlers, _ params: CartLegacyClearParams) async throws -> any Encodable & Sendable {
        try await handlers.cartLegacyClear(params)
    }
}

public enum LegacyToolError: Error, Equatable {
    /// 未知的工具名。
    case unknownTool(String)
    /// 尚未设置 handler（原生意图框架调用时）。
    case handlersNotSet
}

/// 工具名与分派辅助。
public enum LegacyTools {
    /// 清单中的全部工具名。
    public static let names: [String] = [
        "orders.list",
        "orders.find",
        "cart.legacyClear",
    ]

    /// 按工具名把调用分派到对应的 handler。`arguments` 为参数 JSON（空数据视为 `{}`）。
    public static func dispatch(_ handlers: any LegacyToolHandlers, name: String, arguments: Data) async throws -> any Encodable & Sendable {
        let data = arguments.isEmpty ? Data("{}".utf8) : arguments
        let decoder = JSONDecoder()
        switch name {
        case "orders.list":
            return try await LegacyDeprecatedToolCaller.shared.ordersList(handlers, decoder.decode(OrdersListParams.self, from: data))
        case "orders.find":
            return try await handlers.ordersFind(decoder.decode(OrdersFindParams.self, from: data))
        case "cart.legacyClear":
            return try await LegacyDeprecatedToolCaller.shared.cartLegacyClear(handlers, decoder.decode(CartLegacyClearParams.self, from: data))
        default:
            throw LegacyToolError.unknownTool(name)
        }
    }
}
