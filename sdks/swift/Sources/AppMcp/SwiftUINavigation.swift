#if canImport(SwiftUI)
import SwiftUI

// 第 4c 项 E（spec/protocol.md 3.4）：view 工具可见性与 NavigationStack 导航适配。
// @compat 本机没有 Swift 工具链，本文件未经编译与测试（按 SwiftUI iOS 16 / macOS 13 API 编写）。

/// 把 `view` 工具的启用状态绑定到视图：出现（`onAppear`）且场景处于 `active` 时启用，消失或场景进入
/// `inactive` / `background` 时禁用（禁用的工具不同步给 Host）。
@available(iOS 15.0, macOS 12.0, tvOS 15.0, watchOS 8.0, *)
struct ViewToolModifier: ViewModifier {
    let handles: [ToolHandle]
    @Environment(\.scenePhase) private var scenePhase
    @State private var appeared = false

    func body(content: Content) -> some View {
        content
            .onAppear {
                appeared = true
                apply(scenePhase == .active)
            }
            .onDisappear {
                appeared = false
                apply(false)
            }
            .onChange(of: scenePhase) { phase in
                apply(appeared && phase == .active)
            }
    }

    private func apply(_ enabled: Bool) {
        for handle in handles {
            try? handle.setEnabled(enabled)
        }
    }
}

extension View {
    /// `view` 工具只在本视图可见、所在场景 `active` 时启用：
    ///
    /// ```swift
    /// CartView()
    ///     .viewTool(checkoutTool)   // try client.tool("cart.checkout", …, enabled: false, surface: .view, page: "cart")
    /// ```
    ///
    /// `NavigationStack` 中被压到下层的页面会收到 `onDisappear`，因此只有最上层页面的工具启用；
    /// `.sheet` / `.fullScreenCover` 不会让下层视图 `onDisappear`——需要压制时由 App 自行 `setEnabled(false)`。
    @available(iOS 15.0, macOS 12.0, tvOS 15.0, watchOS 8.0, *)
    public func viewTool(_ handles: ToolHandle...) -> some View {
        modifier(ViewToolModifier(handles: handles))
    }
}

/// `NavigationStack(path:)` 的导航适配：页面名 → 压入路径的值。
///
/// ```swift
/// @StateObject var router = NavigationRouter()
/// …
/// NavigationStack(path: $router.path) { HomeView().navigationDestination(for: Route.self) { … } }
///     .onAppear {
///         router.route("cart") { _ in Route.cart }
///         router.route("orders.detail") { req in Route.order(try req.params(OrderParams.self)?.id ?? "") }
///         client.setNavigationHandler(router.handler)   // 最好在 client.start() 之前
///     }
/// ```
///
/// 收到导航时把目标值压入 `path`（`resetPath: true` 时先清空，即从根页面导航）；未登记的页面以失败回复。
@available(iOS 16.0, macOS 13.0, tvOS 16.0, watchOS 9.0, *)
@MainActor
public final class NavigationRouter: ObservableObject {
    @Published public var path = NavigationPath()
    private let router = PageRouter()
    private let resetPath: Bool

    public nonisolated init(resetPath: Bool = true) {
        self.resetPath = resetPath
    }

    /// 登记页面：`destination` 由页面参数得到要压入路径的值（抛错 → 导航失败）。
    public func route<Value: Hashable>(_ page: String, _ destination: @escaping (NavigationRequest) throws -> Value) {
        router.page(page) { [weak self] request in
            guard let self else { return }
            let value = try destination(request)
            if self.resetPath { self.path = NavigationPath() }
            self.path.append(value)
        }
    }

    /// 登记根页面：清空路径。
    public func root(_ page: String) {
        router.page(page) { [weak self] _ in self?.path = NavigationPath() }
    }

    /// 作为 `AppMcpClient.setNavigationHandler(_:)` 的回调。
    public nonisolated var handler: NavigateFunction { router.handler }
}
#endif
