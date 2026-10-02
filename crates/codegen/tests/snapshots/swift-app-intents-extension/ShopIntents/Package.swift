// swift-tools-version: 6.0
// 由 app-mcp-codegen 生成（target: swift-app-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// App 与 App Intents 扩展共用的 Swift 包：参数类型、handler 协议、各 AppIntent。
// 在 Xcode 中以本地包加入工程，把 ShopIntents 库同时链接到 App target 与 App Intents 扩展 target。

import PackageDescription

let package = Package(
    name: "ShopIntents",
    platforms: [.iOS("26.0"), .macOS("26.0")],
    products: [
        .library(name: "ShopIntents", targets: ["ShopIntents"]),
    ],
    targets: [
        .target(name: "ShopIntents"),
    ]
)
