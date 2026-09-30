// 由 app-mcp-codegen 生成（target: swift），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算

import Foundation

/// 商品分类
public enum CatalogSearchCategory: String, Codable, Sendable, CaseIterable {
    case electronics = "electronics"
    case homeGoods = "home-goods"
    case food = "food"
}

/// 按关键词搜索商品，返回商品 ID、名称、价格
public struct CatalogSearchParams: Codable, Sendable, Equatable {
    /// 商品名或分类，如“耳机”
    public var keyword: String?
    /// 商品分类
    public var category: CatalogSearchCategory?
    /// 最多返回条数
    /// 取值范围：≥ 1，≤ 100；默认值：20
    public var limit: Int?
    /// 只看有货
    public var inStock: Bool?
    /// 价格上限（元）
    /// 取值范围：> 0
    public var maxPrice: Double?

    public init(keyword: String? = nil, category: CatalogSearchCategory? = nil, limit: Int? = nil, inStock: Bool? = nil, maxPrice: Double? = nil) {
        self.keyword = keyword
        self.category = category
        self.limit = limit
        self.inStock = inStock
        self.maxPrice = maxPrice
    }

    enum CodingKeys: String, CodingKey {
        case keyword = "keyword"
        case category = "category"
        case limit = "limit"
        case inStock = "inStock"
        case maxPrice = "maxPrice"
    }
}

/// 把商品加入购物车
public struct CartAddParams: Codable, Sendable, Equatable {
    /// 商品 ID
    public var productId: String
    /// 数量
    /// 取值范围：≥ 1
    public var qty: Int
    /// 备注，可为 null
    public var note: String?

    public init(productId: String, qty: Int, note: String? = nil) {
        self.productId = productId
        self.qty = qty
        self.note = note
    }

    enum CodingKeys: String, CodingKey {
        case productId = "productId"
        case qty = "qty"
        case note = "note"
    }
}

/// 从购物车移除一个条目
public struct CartRemoveItemParams: Codable, Sendable, Equatable {
    public var itemId: String

    public init(itemId: String) {
        self.itemId = itemId
    }

    enum CodingKeys: String, CodingKey {
        case itemId = "itemId"
    }
}

/// 配送方式
public enum CartCheckoutShippingMethod: String, Codable, Sendable, CaseIterable {
    case standard = "standard"
    case express = "express"
}

/// 配送选项
public struct CartCheckoutShipping: Codable, Sendable, Equatable {
    /// 配送方式
    public var method: CartCheckoutShippingMethod
    /// 最早送达时间
    /// 格式：date-time
    public var deliverAfter: String?

    public init(method: CartCheckoutShippingMethod, deliverAfter: String? = nil) {
        self.method = method
        self.deliverAfter = deliverAfter
    }

    enum CodingKeys: String, CodingKey {
        case method = "method"
        case deliverAfter = "deliverAfter"
    }
}

public struct CartCheckoutItemsItem: Codable, Sendable, Equatable {
    public var itemId: String
    /// 取值范围：≥ 1
    public var qty: Int?

    public init(itemId: String, qty: Int? = nil) {
        self.itemId = itemId
        self.qty = qty
    }

    enum CodingKeys: String, CodingKey {
        case itemId = "itemId"
        case qty = "qty"
    }
}

/// 提交订单并支付
public struct CartCheckoutParams: Codable, Sendable, Equatable {
    /// 收货地址 ID
    public var addressId: String
    /// 优惠码
    public var coupon: String?
    /// 配送选项
    public var shipping: CartCheckoutShipping?
    /// 只结算这些条目；省略时结算全部
    /// 元素个数：≥ 1
    public var items: [CartCheckoutItemsItem]?
    /// 默认值：false
    public var giftWrap: Bool?

    public init(addressId: String, coupon: String? = nil, shipping: CartCheckoutShipping? = nil, items: [CartCheckoutItemsItem]? = nil, giftWrap: Bool? = nil) {
        self.addressId = addressId
        self.coupon = coupon
        self.shipping = shipping
        self.items = items
        self.giftWrap = giftWrap
    }

    enum CodingKeys: String, CodingKey {
        case addressId = "addressId"
        case coupon = "coupon"
        case shipping = "shipping"
        case items = "items"
        case giftWrap = "giftWrap"
    }
}

/// 新增一条待办
public struct TodosAddParams: Codable, Sendable, Equatable {
    /// 待办内容
    /// 长度：1–200
    public var title: String
    /// 优先级
    /// 可选值：1, 2, 3
    public var priority: Int?
    /// 标签
    public var tags: [String]?
    /// 截止日期
    /// 格式：date
    public var dueDate: String?
    /// 分类（属性名是保留字）
    public var `class`: String?
    /// 属性名含连字符
    public var isUrgent: Bool?

    public init(title: String, priority: Int? = nil, tags: [String]? = nil, dueDate: String? = nil, `class`: String? = nil, isUrgent: Bool? = nil) {
        self.title = title
        self.priority = priority
        self.tags = tags
        self.dueDate = dueDate
        self.class = `class`
        self.isUrgent = isUrgent
    }

    enum CodingKeys: String, CodingKey {
        case title = "title"
        case priority = "priority"
        case tags = "tags"
        case dueDate = "dueDate"
        case `class` = "class"
        case isUrgent = "is-urgent"
    }
}

/// 删除全部已完成的待办
public struct TodosClearParams: Codable, Sendable, Equatable {
    public init() {}
}

/// 按条件导出订单
public struct OrdersExportParams: Codable, Sendable, Equatable {
    /// 筛选条件（任意形式）
    /// 原始 JSON（不支持一般形式的 `oneOf`）
    public var filter: JSONValue?
    /// 附加标签
    public var labels: [String: String]?
    /// 透传给导出器的任意 JSON
    public var extra: JSONValue?
    /// 原始 JSON（不支持 `$ref`）
    public var template: JSONValue?

    public init(filter: JSONValue? = nil, labels: [String: String]? = nil, extra: JSONValue? = nil, template: JSONValue? = nil) {
        self.filter = filter
        self.labels = labels
        self.extra = extra
        self.template = template
    }

    enum CodingKeys: String, CodingKey {
        case filter = "filter"
        case labels = "labels"
        case extra = "extra"
        case template = "template"
    }
}

/// 统计周期
public enum StatsSummaryPeriod: String, Codable, Sendable, CaseIterable {
    case day = "day"
    case week = "week"
    case month = "month"
}

/// 查看销售概览
public struct StatsSummaryParams: Codable, Sendable, Equatable {
    /// 统计周期
    public var period: StatsSummaryPeriod?

    public init(period: StatsSummaryPeriod? = nil) {
        self.period = period
    }

    enum CodingKeys: String, CodingKey {
        case period = "period"
    }
}

/// 任意 JSON 值（用于 schema 未约束或不支持的构造）。
public enum JSONValue: Codable, Sendable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}

/// 示例商城 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
/// 返回值会编码为 JSON 作为工具结果（返回 String 时原样作为文本）。
public protocol ShopToolHandlers: Sendable {
    /// 按关键词搜索商品，返回商品 ID、名称、价格
    ///
    /// 工具 `catalog.search`「搜索商品」，风险：read
    func catalogSearch(_ params: CatalogSearchParams) async throws -> any Encodable & Sendable
    /// 把商品加入购物车
    ///
    /// 工具 `cart.add`「加入购物车」，风险：write
    func cartAdd(_ params: CartAddParams) async throws -> any Encodable & Sendable
    /// 从购物车移除一个条目
    ///
    /// 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
    func cartRemoveItem(_ params: CartRemoveItemParams) async throws -> any Encodable & Sendable
    /// 提交订单并支付
    ///
    /// 工具 `cart.checkout`「结算」，风险：payment
    func cartCheckout(_ params: CartCheckoutParams) async throws -> any Encodable & Sendable
    /// 新增一条待办
    ///
    /// 工具 `todos.add`「新增待办」，风险：write
    func todosAdd(_ params: TodosAddParams) async throws -> any Encodable & Sendable
    /// 删除全部已完成的待办
    ///
    /// 工具 `todos.clear`「清空待办」，风险：destructive
    func todosClear(_ params: TodosClearParams) async throws -> any Encodable & Sendable
    /// 按条件导出订单
    ///
    /// 工具 `orders.export`「导出订单」，风险：write
    func ordersExport(_ params: OrdersExportParams) async throws -> any Encodable & Sendable
    /// 查看销售概览
    ///
    /// 工具 `stats.summary`「销售概览」，风险：read
    func statsSummary(_ params: StatsSummaryParams) async throws -> any Encodable & Sendable
}

public enum ShopToolError: Error, Equatable {
    /// 未知的工具名。
    case unknownTool(String)
    /// 尚未设置 handler（原生意图框架调用时）。
    case handlersNotSet
}

/// 工具名与分派辅助。
public enum ShopTools {
    /// 清单中的全部工具名。
    public static let names: [String] = [
        "catalog.search",
        "cart.add",
        "cart.removeItem",
        "cart.checkout",
        "todos.add",
        "todos.clear",
        "orders.export",
        "stats.summary",
    ]

    /// 按工具名把调用分派到对应的 handler。`arguments` 为参数 JSON（空数据视为 `{}`）。
    public static func dispatch(_ handlers: any ShopToolHandlers, name: String, arguments: Data) async throws -> any Encodable & Sendable {
        let data = arguments.isEmpty ? Data("{}".utf8) : arguments
        let decoder = JSONDecoder()
        switch name {
        case "catalog.search":
            return try await handlers.catalogSearch(decoder.decode(CatalogSearchParams.self, from: data))
        case "cart.add":
            return try await handlers.cartAdd(decoder.decode(CartAddParams.self, from: data))
        case "cart.removeItem":
            return try await handlers.cartRemoveItem(decoder.decode(CartRemoveItemParams.self, from: data))
        case "cart.checkout":
            return try await handlers.cartCheckout(decoder.decode(CartCheckoutParams.self, from: data))
        case "todos.add":
            return try await handlers.todosAdd(decoder.decode(TodosAddParams.self, from: data))
        case "todos.clear":
            return try await handlers.todosClear(decoder.decode(TodosClearParams.self, from: data))
        case "orders.export":
            return try await handlers.ordersExport(decoder.decode(OrdersExportParams.self, from: data))
        case "stats.summary":
            return try await handlers.statsSummary(decoder.decode(StatsSummaryParams.self, from: data))
        default:
            throw ShopToolError.unknownTool(name)
        }
    }
}
