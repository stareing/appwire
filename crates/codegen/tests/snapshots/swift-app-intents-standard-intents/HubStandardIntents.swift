// 由 app-mcp-codegen 生成（target: swift-app-intents --standard-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// Apple App Intents 系统 schema 版本（spec/intents.md 第 3 节）；依赖 HubTools.swift 与 HubAppIntents.swift（HubIntentRuntime）。
// 未在真实 AppIntents 框架上编译验证（本机无 iOS SDK），首次接入请在 Xcode 中确认。
// 需要 App 提供以下实体类型（public，与本文件同一模块；属性与 defaultQuery 见 Apple 文档对应的实体 schema）：
//   HubBrowserTab：@AppEntity(schema: .browser.tab)

import AppIntents
import Foundation

/// 系统 schema `.browser.openURLInTab`（标准意图 `link.open@1`），调用工具 browser.open。实体参数 `tab`（HubBrowserTab）由 App 提供，不传给工具。
@available(iOS 18.0, macOS 15.0, visionOS 2.0, *)
@available(tvOS, unavailable)
@available(watchOS, unavailable)
@AppIntent(schema: .browser.openURLInTab)
public struct BrowserOpenOpenURLInTabSchemaIntent: AppIntent {
    @Parameter
    public var url: URL

    @Parameter
    public var tab: HubBrowserTab

    public init() {}

    @MainActor
    public func perform() async throws -> some IntentResult {
        let params = BrowserOpenParams(url: url.absoluteString)
        _ = try await HubIntentRuntime.requireHandlers().browserOpen(params)
        return .result()
    }
}
