// 由 app-mcp-codegen 生成（target: swift-app-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// 依赖同目录下的 ShopTools.swift（参数类型与 ShopToolHandlers 协议）。
// 需要 iOS / macOS 26 起的 App Intents（supportedModes）；
// 位于 App 与 App Intents 扩展共用的 Swift 包 ShopIntents；App 的 init 与扩展的 init 各调用一次
// `ShopIntentRuntime.configure(ShopIntentHandlersProvider.self)`（扩展入口已生成），`ShopIntentHandlersProvider` 由开发者实现一次（`ShopIntentHandlersProviding`），
// 放在 App 与扩展都能访问的模块中；handler 在所在进程首次执行 intent 时才构造。
// allowedExecutionTargets 需 iOS / macOS 27（以 @available 限定，低版本不声明，由系统决定执行进程）。
// CancellableIntent 需 iOS / macOS 26.4（以 #available 限定）；取消原因经 `ShopIntentRuntime.onCancel` 通知 App。
// 以上可选输出未在真实 AppIntents SDK 上编译验证，首次接入请在 Xcode 中确认。
//
// ## 能力范围
// - 待办的增删改查
// - 商品搜索、购物车管理、结算

import AppIntents
import Foundation
import Synchronization

extension CatalogSearchCategory: AppEnum {
    public static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "商品分类")
    public static let caseDisplayRepresentations: [CatalogSearchCategory: DisplayRepresentation] = [
        .electronics: DisplayRepresentation(title: "electronics"),
        .homeGoods: DisplayRepresentation(title: "home-goods"),
        .food: DisplayRepresentation(title: "food"),
    ]
}

extension CartCheckoutShippingMethod: AppEnum {
    public static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "配送方式")
    public static let caseDisplayRepresentations: [CartCheckoutShippingMethod: DisplayRepresentation] = [
        .standard: DisplayRepresentation(title: "standard"),
        .express: DisplayRepresentation(title: "express"),
    ]
}

extension StatsSummaryPeriod: AppEnum {
    public static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "统计周期")
    public static let caseDisplayRepresentations: [StatsSummaryPeriod: DisplayRepresentation] = [
        .day: DisplayRepresentation(title: "day"),
        .week: DisplayRepresentation(title: "week"),
        .month: DisplayRepresentation(title: "month"),
    ]
}

/// App 与扩展共用的 handler 来源。由开发者实现一次（类型名 `ShopIntentHandlersProvider`，生成的扩展入口按此名引用），
/// 放在 App target 与扩展 target 都能访问的模块中。`Sendable` 使其元类型可存入 `Mutex`（Swift 6.2 `SendableMetatype`）。
public protocol ShopIntentHandlersProviding: Sendable {
    /// 构造 handler；在所在进程（App 或扩展）首次执行 intent 时调用一次。
    static func makeHandlers() -> any ShopToolHandlers
}

/// intent 被取消的原因（`CancellableIntent`，iOS / macOS 26.4 起）。
public enum ShopIntentCancellationReason: Sendable {
    /// 未报告进度且超过系统运行时限。
    case timeout
    /// 用户在 Siri、实时活动或快捷指令中取消。
    case userCancelled
    /// 其他原因（新系统版本可能新增）。
    case other
}

/// App Intents 调用 ShopToolHandlers 的入口。
public enum ShopIntentRuntime {
    private struct State {
        var provider: (any ShopIntentHandlersProviding.Type)?
        var handlers: (any ShopToolHandlers)?
    }

    private static let state = Mutex(State())

    /// 在 App 的 `init` 与扩展的 `init` 中各调用一次；只记录来源，首次执行 intent 时才构造 handler。
    public static func configure(_ provider: any ShopIntentHandlersProviding.Type) {
        state.withLock { $0.provider = provider }
    }

    /// 未调用 `configure` 时抛出 `ShopToolError.handlersNotSet`。
    static func requireHandlers() throws -> any ShopToolHandlers {
        try state.withLock { state in
            if let handlers = state.handlers { return handlers }
            guard let provider = state.provider else { throw ShopToolError.handlersNotSet }
            let handlers = provider.makeHandlers()
            state.handlers = handlers
            return handlers
        }
    }

    private static let cancelHook = Mutex<(@Sendable (_ tool: String, _ reason: ShopIntentCancellationReason) -> Void)?>(nil)

    /// 设置取消通知（可选），参数为工具名与原因；在系统给出的收尾时间内调用，应尽快返回。handler 所在 Task 同时被取消。
    public static func onCancel(_ hook: (@Sendable (_ tool: String, _ reason: ShopIntentCancellationReason) -> Void)?) {
        cancelHook.withLock { $0 = hook }
    }

    @available(iOS 26.4, macOS 26.4, *)
    static func cancelled(tool: String, reason: IntentCancellationReason) {
        let mapped: ShopIntentCancellationReason
        switch reason {
        case .timeout: mapped = .timeout
        case .userCancelled: mapped = .userCancelled
        default: mapped = .other
        }
        let hook = cancelHook.withLock { $0 }
        hook?(tool, mapped)
    }

    static func decode<T: Decodable>(_ type: T.Type, from json: String) throws -> T {
        try JSONDecoder().decode(type, from: Data(json.utf8))
    }

    static func decodeIfPresent<T: Decodable>(_ type: T.Type, from json: String?) throws -> T? {
        guard let json, !json.isEmpty else { return nil }
        return try decode(type, from: json)
    }

    /// 把 handler 的返回值转为 Siri / 快捷指令展示的文本：String 原样返回，其他值编码为 JSON。
    static func text(_ value: any Encodable & Sendable) throws -> String {
        if let string = value as? String { return string }
        let data = try JSONEncoder().encode(value)
        return String(decoding: data, as: UTF8.self)
    }
}

/// 按关键词搜索商品，返回商品 ID、名称、价格
///
/// 工具 `catalog.search`「搜索商品」，风险：read
public struct CatalogSearchIntent: AppIntent {
    public static let title: LocalizedStringResource = "搜索商品"
    public static let description = IntentDescription("按关键词搜索商品，返回商品 ID、名称、价格")
    public static let supportedModes: IntentModes = .background
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { [.main, .appIntentsExtension] }

    @Parameter(title: "keyword", description: "商品名或分类，如“耳机”")
    public var keyword: String?

    @Parameter(title: "category", description: "商品分类")
    public var category: CatalogSearchCategory?

    @Parameter(title: "limit", description: "最多返回条数。取值范围：≥ 1，≤ 100；默认值：20", default: 20)
    public var limit: Int?

    @Parameter(title: "inStock", description: "只看有货")
    public var inStock: Bool?

    @Parameter(title: "maxPrice", description: "价格上限（元）。取值范围：> 0")
    public var maxPrice: Double?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = CatalogSearchParams(keyword: keyword, category: category, limit: limit, inStock: inStock, maxPrice: maxPrice)
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().catalogSearch(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "catalog.search", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().catalogSearch(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension CatalogSearchIntent: CancellableIntent {}

/// 把商品加入购物车
///
/// 工具 `cart.add`「加入购物车」，风险：write
public struct CartAddIntent: AppIntent {
    public static let title: LocalizedStringResource = "加入购物车"
    public static let description = IntentDescription("把商品加入购物车")
    public static let supportedModes: IntentModes = .background
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { [.main, .appIntentsExtension] }

    @Parameter(title: "productId", description: "商品 ID")
    public var productId: String

    @Parameter(title: "qty", description: "数量。取值范围：≥ 1")
    public var qty: Int

    @Parameter(title: "note", description: "备注，可为 null")
    public var note: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = CartAddParams(productId: productId, qty: qty, note: note)
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().cartAdd(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "cart.add", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().cartAdd(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension CartAddIntent: CancellableIntent {}

/// 从购物车移除一个条目
///
/// 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
public struct CartRemoveItemIntent: AppIntent {
    public static let title: LocalizedStringResource = "移除购物车条目"
    public static let description = IntentDescription("从购物车移除一个条目")
    public static let supportedModes: IntentModes = .foreground(.immediate)
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { .main }

    @Parameter(title: "itemId")
    public var itemId: String

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        try await requestConfirmation(actionName: .`continue`, dialog: "确认移除购物车条目？")
        let params = CartRemoveItemParams(itemId: itemId)
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().cartRemoveItem(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "cart.removeItem", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().cartRemoveItem(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension CartRemoveItemIntent: CancellableIntent {}

/// 提交订单并支付
///
/// 工具 `cart.checkout`「结算」，风险：payment
public struct CartCheckoutIntent: AppIntent {
    public static let title: LocalizedStringResource = "结算"
    public static let description = IntentDescription("提交订单并支付")
    public static let supportedModes: IntentModes = .foreground(.immediate)
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { .main }

    @Parameter(title: "addressId", description: "收货地址 ID")
    public var addressId: String

    @Parameter(title: "coupon", description: "优惠码")
    public var coupon: String?

    // CartCheckoutShipping 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "shipping", description: "配送选项。以 JSON 字符串传入")
    public var shipping: String?

    // [CartCheckoutItemsItem] 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "items", description: "只结算这些条目；省略时结算全部。元素个数：≥ 1。以 JSON 字符串传入")
    public var items: String?

    @Parameter(title: "giftWrap", description: "默认值：false", default: false)
    public var giftWrap: Bool?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        try await requestConfirmation(actionName: .pay, dialog: "确认结算？")
        let params = CartCheckoutParams(addressId: addressId, coupon: coupon, shipping: try ShopIntentRuntime.decodeIfPresent(CartCheckoutShipping.self, from: shipping), items: try ShopIntentRuntime.decodeIfPresent([CartCheckoutItemsItem].self, from: items), giftWrap: giftWrap)
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().cartCheckout(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "cart.checkout", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().cartCheckout(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension CartCheckoutIntent: CancellableIntent {}

/// 新增一条待办
///
/// 工具 `todos.add`「新增待办」，风险：write
public struct TodosAddIntent: AppIntent {
    public static let title: LocalizedStringResource = "新增待办"
    public static let description = IntentDescription("新增一条待办")
    public static let supportedModes: IntentModes = .foreground(.immediate)
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { .main }

    @Parameter(title: "title", description: "待办内容。长度：1–200")
    public var titleValue: String

    @Parameter(title: "priority", description: "优先级。可选值：1, 2, 3")
    public var priority: Int?

    @Parameter(title: "tags", description: "标签")
    public var tags: [String]?

    @Parameter(title: "dueDate", description: "截止日期。格式：date")
    public var dueDate: Date?

    @Parameter(title: "class", description: "分类（属性名是保留字）")
    public var `class`: String?

    @Parameter(title: "is-urgent", description: "属性名含连字符")
    public var isUrgent: Bool?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = TodosAddParams(title: titleValue, priority: priority, tags: tags, dueDate: dueDate.map { $0.formatted(Date.ISO8601FormatStyle(timeZone: .current).year().month().day()) }, class: `class`, isUrgent: isUrgent)
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().todosAdd(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "todos.add", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().todosAdd(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension TodosAddIntent: CancellableIntent {}

/// 删除全部已完成的待办
///
/// 工具 `todos.clear`「清空待办」，风险：destructive
public struct TodosClearIntent: AppIntent {
    public static let title: LocalizedStringResource = "清空待办"
    public static let description = IntentDescription("删除全部已完成的待办")
    public static let supportedModes: IntentModes = .foreground(.immediate)
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { .main }

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        try await requestConfirmation(actionName: .`continue`, dialog: "确认清空待办？")
        let params = TodosClearParams()
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().todosClear(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "todos.clear", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().todosClear(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension TodosClearIntent: CancellableIntent {}

/// 按条件导出订单
///
/// 工具 `orders.export`「导出订单」，风险：write
public struct OrdersExportIntent: AppIntent {
    public static let title: LocalizedStringResource = "导出订单"
    public static let description = IntentDescription("按条件导出订单")
    public static let supportedModes: IntentModes = .foreground(.immediate)
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { .main }

    // JSONValue 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "filter", description: "筛选条件（任意形式）。原始 JSON（不支持一般形式的 `oneOf`）。以 JSON 字符串传入")
    public var filter: String?

    // [String: String] 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "labels", description: "附加标签。以 JSON 字符串传入")
    public var labels: String?

    // JSONValue 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "extra", description: "透传给导出器的任意 JSON。以 JSON 字符串传入")
    public var extra: String?

    // JSONValue 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "template", description: "原始 JSON（不支持 `$ref`）。以 JSON 字符串传入")
    public var template: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = OrdersExportParams(filter: try ShopIntentRuntime.decodeIfPresent(JSONValue.self, from: filter), labels: try ShopIntentRuntime.decodeIfPresent([String: String].self, from: labels), extra: try ShopIntentRuntime.decodeIfPresent(JSONValue.self, from: extra), template: try ShopIntentRuntime.decodeIfPresent(JSONValue.self, from: template))
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().ordersExport(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "orders.export", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().ordersExport(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension OrdersExportIntent: CancellableIntent {}

/// 查看销售概览
///
/// 工具 `stats.summary`「销售概览」，风险：read
public struct StatsSummaryIntent: AppIntent {
    public static let title: LocalizedStringResource = "销售概览"
    public static let description = IntentDescription("查看销售概览")
    public static let supportedModes: IntentModes = .foreground(.immediate)
    @available(iOS 27.0, macOS 27.0, *)
    public static var allowedExecutionTargets: IntentExecutionTargets { .main }

    @Parameter(title: "period", description: "统计周期")
    public var period: StatsSummaryPeriod?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = StatsSummaryParams(period: period)
        let result: any Encodable & Sendable
        if #available(iOS 26.4, macOS 26.4, *) {
            result = try await withIntentCancellationHandler {
                try await ShopIntentRuntime.requireHandlers().statsSummary(params)
            } onCancel: { reason in
                ShopIntentRuntime.cancelled(tool: "stats.summary", reason: reason)
            }
        } else {
            result = try await ShopIntentRuntime.requireHandlers().statsSummary(params)
        }
        return .result(value: try ShopIntentRuntime.text(result))
    }
}

@available(iOS 26.4, macOS 26.4, *)
extension StatsSummaryIntent: CancellableIntent {}

/// 只读 / 写入且没有必填参数的工具提供 App Shortcut（短语由工具描述生成，建议在 String Catalog 中本地化与精简）。
public struct ShopAppShortcuts: AppShortcutsProvider {
    public static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: CatalogSearchIntent(),
            phrases: ["\(.applicationName) 按关键词搜索商品，返回商品 ID、名称、价格"],
            shortTitle: "搜索商品",
            systemImageName: "magnifyingglass"
        )
        AppShortcut(
            intent: OrdersExportIntent(),
            phrases: ["\(.applicationName) 按条件导出订单"],
            shortTitle: "导出订单",
            systemImageName: "square.and.pencil"
        )
        AppShortcut(
            intent: StatsSummaryIntent(),
            phrases: ["\(.applicationName) 查看销售概览"],
            shortTitle: "销售概览",
            systemImageName: "magnifyingglass"
        )
    }
}

/// 共享包的 intent 声明；App 与扩展各自以 `includedPackages` 包含它（见 App/ShopAppIntentsPackage.swift 与扩展入口）。
public struct ShopIntentsPackage: AppIntentsPackage {}
