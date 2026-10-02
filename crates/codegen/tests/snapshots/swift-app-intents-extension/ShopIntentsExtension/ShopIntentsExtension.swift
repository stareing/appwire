// 由 app-mcp-codegen 生成（target: swift-app-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// App Intents 扩展的入口：App 未运行时，系统在扩展进程中执行 ShopIntents 包里的 intent。
// 需要开发者提供 `ShopIntentHandlersProvider`（实现 `ShopIntentHandlersProviding`），并对本扩展 target 可见。

import AppIntents
import ShopIntents

@main
struct ShopIntentsExtension: AppIntentsExtension {
    init() {
        ShopIntentRuntime.configure(ShopIntentHandlersProvider.self)
    }
}

/// 让扩展包含 ShopIntents 包中的 intent。
struct ShopIntentsExtensionPackage: AppIntentsPackage {
    static var includedPackages: [any AppIntentsPackage.Type] {
        [ShopIntentsPackage.self]
    }
}
