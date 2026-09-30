// swift-tools-version:5.9
//
// app-mcp Swift SDK。
//
// 先运行 `bash bindings/uniffi/scripts/generate.sh` 生成：
//   Sources/AppMcpBindings/AppMcpBindings.swift          uniffi 生成的 Swift 绑定
//   Sources/app_mcp_uniffiFFI/include/*.h, module.modulemap
//   lib/libapp_mcp_uniffi.{so,dylib}                     原生库（本机开发 / 测试用）
//
// Hub SDK（Agent 端）另由 `bash bindings/hub-uniffi/scripts/generate.sh` 生成：
//   Sources/AppMcpHubBindings/AppMcpHubBindings.swift、Sources/app_mcp_hub_uniffiFFI/include/*、
//   lib/libapp_mcp_hub_uniffi.{so,dylib}
//
// Apple 平台发布时应把各架构静态库打包为 XCFramework，用 binaryTarget 替换 lib/ 的链接设置。
import Foundation
import PackageDescription

let libDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("lib").path

let package = Package(
    name: "AppMcp",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        .library(name: "AppMcp", targets: ["AppMcp"]),
        .library(name: "AppMcpHub", targets: ["AppMcpHub"]),
    ],
    targets: [
        // uniffi 生成的 C 头文件与 modulemap（shim.c 只为让 SwiftPM 把它当作 C 目标）。
        .target(
            name: "app_mcp_uniffiFFI",
            path: "Sources/app_mcp_uniffiFFI",
            linkerSettings: [
                .linkedLibrary("app_mcp_uniffi"),
                .unsafeFlags(["-L", libDir, "-Xlinker", "-rpath", "-Xlinker", libDir]),
            ]
        ),
        // uniffi 生成的 Swift 绑定。
        .target(
            name: "AppMcpBindings",
            dependencies: ["app_mcp_uniffiFFI"],
            path: "Sources/AppMcpBindings"
        ),
        // 惯用封装：@MainActor 切换、async handler、Codable 参数。
        .target(
            name: "AppMcp",
            dependencies: ["AppMcpBindings"],
            path: "Sources/AppMcp"
        ),
        // Hub SDK：uniffi 生成的 C 头文件、Swift 绑定与惯用封装。
        .target(
            name: "app_mcp_hub_uniffiFFI",
            path: "Sources/app_mcp_hub_uniffiFFI",
            linkerSettings: [
                .linkedLibrary("app_mcp_hub_uniffi"),
                .unsafeFlags(["-L", libDir, "-Xlinker", "-rpath", "-Xlinker", libDir]),
            ]
        ),
        .target(
            name: "AppMcpHubBindings",
            dependencies: ["app_mcp_hub_uniffiFFI"],
            path: "Sources/AppMcpHubBindings"
        ),
        .target(
            name: "AppMcpHub",
            dependencies: ["AppMcpHubBindings"],
            path: "Sources/AppMcpHub"
        ),
        .testTarget(
            name: "AppMcpHubTests",
            dependencies: ["AppMcpHub", "AppMcp"],
            path: "Tests/AppMcpHubTests"
        ),
        .testTarget(
            name: "AppMcpTests",
            dependencies: ["AppMcp"],
            path: "Tests/AppMcpTests"
        ),
    ]
)
