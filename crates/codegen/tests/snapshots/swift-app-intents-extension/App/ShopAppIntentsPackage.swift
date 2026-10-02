// 由 app-mcp-codegen 生成（target: swift-app-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// 加入 App target。另需在 App 的 init 中调用（与扩展入口相同）：
//     ShopIntentRuntime.configure(ShopIntentHandlersProvider.self)

import AppIntents
import ShopIntents

/// 让 App 包含 ShopIntents 包中的 intent。
struct ShopAppIntentsPackage: AppIntentsPackage {
    static var includedPackages: [any AppIntentsPackage.Type] {
        [ShopIntentsPackage.self]
    }
}
