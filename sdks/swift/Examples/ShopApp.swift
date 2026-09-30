// SwiftUI 使用示例（仅作参考代码，不参与 SwiftPM 构建；需要 iOS 17 / macOS 14 的 Observation）。
//
// 要点：
// - 工具 handler 默认在 @MainActor 上执行，可以直接读写 @Observable 模型；
// - 参数用 Codable 结构体声明，解码失败自动返回 INVALID_INPUT；
// - 页面级工具放进 ToolScope，页面消失时 dispose 一次性注销；
// - handler 闭包被原生层强引用，引用模型时用 [weak model] 避免循环引用。

import AppMcp
import Observation
import SwiftUI

@Observable
@MainActor
final class CartModel {
    var items: [String: Int] = [:]
    var status: String = "未连接"

    func add(_ sku: String, qty: Int) { items[sku, default: 0] += qty }
    func clear() { items.removeAll() }
}

struct AddItemArgs: Codable {
    let sku: String
    let qty: Int?
}

@MainActor
final class AppMcpService {
    let client: AppMcpClient

    init(model: CartModel) throws {
        client = try AppMcpClient(config: AppMcpConfig(
            appId: "shop",
            appName: "网店",
            token: UserDefaults.standard.string(forKey: "appMcpToken"),
            overview: AppOverview(
                summary: "网店：浏览商品、管理购物车、结账",
                body: "## 典型流程\n1. cart.add 加入商品\n2. 读取资源 cart 确认\n3. cart.checkout 结账（需要用户确认）",
                locale: "zh-CN"
            ),
            onStateChange: { [weak model] state in
                Task { @MainActor in model?.status = "\(state.status)" }
            },
            onPaired: { token in UserDefaults.standard.set(token, forKey: "appMcpToken") },
            // 空闲休眠；Host 通过 shop://app-mcp/wake?token=… 唤醒（macOS 上用 `open -g`，不前置窗口）。
            // iOS 不配置时默认即为 iOSDefault()：进入后台立即休眠。
            lifecycle: LifecyclePolicy(mode: .idle, wake: .urlScheme("shop"))
        ))

        let schema = #"{"type":"object","properties":{"sku":{"type":"string"},"qty":{"type":"integer","minimum":1}},"required":["sku"]}"#
        try client.tool("cart.add", description: "把商品加入购物车", inputSchema: schema) { [weak model] (args: AddItemArgs, ctx) in
            guard let model else { throw ToolCallError(ErrorKind.appNotResponding, "界面已关闭") }
            model.add(args.sku, qty: args.qty ?? 1)
            ctx.addStateHint("cart")
            return model.items
        }
        try client.tool("cart.clear", description: "清空购物车", risk: .destructive) { [weak model] (_: NoArguments, ctx) in
            model?.clear()
            ctx.addStateHint("cart")
        }
        try client.resource("cart", description: "当前购物车") { [weak model] in model?.items ?? [:] }
        client.start()
    }
}

/// 页面级工具：出现时注册，消失时整体注销。
struct CheckoutPage: View {
    let service: AppMcpService
    @State private var scope: ToolScope?

    var body: some View {
        Text("结账")
            .onAppear {
                scope = try? service.client.scope("checkout-page")
                _ = try? scope?.tool("cart.checkout", description: "提交订单", risk: .payment) { (_: NoArguments, ctx) in
                    try await Task.sleep(nanoseconds: 300_000_000) // 模拟下单；被 Host 取消时抛 CancellationError
                    return ["orderId": "A-1001"]
                }
            }
            .onDisappear { scope?.dispose() }
    }
}

@main
struct ShopApp: App {
    @State private var model = CartModel()
    @State private var service: AppMcpService?

    var body: some Scene {
        WindowGroup {
            VStack {
                Text("app-mcp：\(model.status)")
                List(model.items.sorted(by: { $0.key < $1.key }), id: \.key) { sku, qty in
                    Text("\(sku) × \(qty)")
                }
                if let service { NavigationLink("结账") { CheckoutPage(service: service) } }
            }
            .task { service = try? AppMcpService(model: model) }
            // scenePhase → 可见性 / 回前台回连；onOpenURL → handleWake
            .modifier(OptionalLifecycle(client: service?.client))
        }
    }
}

/// 客户端创建前不挂修饰器。
struct OptionalLifecycle: ViewModifier {
    let client: AppMcpClient?
    func body(content: Content) -> some View {
        if let client { content.appMcpLifecycle(client) } else { content }
    }
}
