// 由 app-mcp-codegen 生成（target: swift-app-intents），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
//
// 依赖同目录下的 LegacyTools.swift（参数类型与 LegacyToolHandlers 协议）。
// 需要 iOS / macOS 26 起的 App Intents（supportedModes）；
// 启动时设置 `LegacyIntentRuntime.handlers = <实现 LegacyToolHandlers 的对象>`（可直接复用 MCP 的业务实现）。

import AppIntents
import Foundation

/// App Intents 调用 LegacyToolHandlers 的入口。
public enum LegacyIntentRuntime {
    /// 由 App 在启动时设置（如 `App.init`）；未设置时 intent 抛出 `LegacyToolError.handlersNotSet`。
    @MainActor public static var handlers: (any LegacyToolHandlers)?

    @MainActor static func requireHandlers() throws -> any LegacyToolHandlers {
        guard let handlers else { throw LegacyToolError.handlersNotSet }
        return handlers
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

/// 按状态列出订单（旧版）
///
/// 工具 `orders.list`「列出订单」，风险：read
@available(*, deprecated, message: "旧版 \"列表\" 接口 */ 不再维护：$x ${y} \\(z) 'q' <b>&amp; #{w}\n请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）")
public struct OrdersListIntent: AppIntent {
    public static let title: LocalizedStringResource = "列出订单"
    public static let description = IntentDescription("按状态列出订单（旧版）")
    public static let supportedModes: IntentModes = .background

    // 参数已弃用（inputSchema 中 deprecated: true）
    @Parameter(title: "status", description: "订单状态")
    public var status: String

    // 参数已弃用（inputSchema 中 deprecated: true）
    @Parameter(title: "page", description: "页码。取值范围：≥ 1")
    public var page: Int?

    @Parameter(title: "limit", description: "最多返回条数")
    public var limit: Int?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = OrdersListParams(status: status, page: page, limit: limit)
        let result = try await LegacyDeprecatedToolCaller.shared.ordersList(LegacyIntentRuntime.requireHandlers(), params)
        return .result(value: try LegacyIntentRuntime.text(result))
    }
}

/// 按状态查找订单
///
/// 工具 `orders.find`「查找订单」，风险：read
public struct OrdersFindIntent: AppIntent {
    public static let title: LocalizedStringResource = "查找订单"
    public static let description = IntentDescription("按状态查找订单")
    public static let supportedModes: IntentModes = .background

    @Parameter(title: "state", description: "订单状态")
    public var state: String

    @Parameter(title: "cursor", description: "分页游标")
    public var cursor: String?

    // OrdersFindFilter 不能直接作为 App Intent 参数，降级为 JSON 字符串
    @Parameter(title: "filter", description: "过滤条件。以 JSON 字符串传入")
    public var filter: String?

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = OrdersFindParams(state: state, cursor: cursor, filter: try LegacyIntentRuntime.decodeIfPresent(OrdersFindFilter.self, from: filter))
        let result = try await LegacyIntentRuntime.requireHandlers().ordersFind(params)
        return .result(value: try LegacyIntentRuntime.text(result))
    }
}

/// 清空购物车（旧版）
///
/// 工具 `cart.legacyClear`「清空购物车」，风险：write
@available(*, deprecated, message: "改用 cart.clear")
public struct CartLegacyClearIntent: AppIntent {
    public static let title: LocalizedStringResource = "清空购物车"
    public static let description = IntentDescription("清空购物车（旧版）")
    public static let supportedModes: IntentModes = .background

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult & ReturnsValue<String> {
        let params = CartLegacyClearParams()
        let result = try await LegacyDeprecatedToolCaller.shared.cartLegacyClear(LegacyIntentRuntime.requireHandlers(), params)
        return .result(value: try LegacyIntentRuntime.text(result))
    }
}
