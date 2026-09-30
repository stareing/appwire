#if canImport(SwiftUI)
import SwiftUI

extension AppPhase {
    /// SwiftUI `ScenePhase` → `AppPhase`。未知的新阶段按 `inactive` 处理。
    @available(iOS 14.0, macOS 11.0, tvOS 14.0, watchOS 7.0, *)
    public init(_ phase: ScenePhase) {
        switch phase {
        case .active: self = .active
        case .background: self = .background
        default: self = .inactive
        }
    }
}

@available(iOS 15.0, macOS 12.0, tvOS 15.0, watchOS 8.0, *)
struct AppMcpLifecycleModifier: ViewModifier {
    let client: AppMcpClient
    let onOtherURL: ((URL) -> Void)?
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content
            .onAppear { client.setPhase(AppPhase(scenePhase)) }
            .onChange(of: scenePhase) { client.setPhase(AppPhase($0)) }
            .onOpenURL { url in
                if !client.handleWake(url: url) { onOtherURL?(url) }
            }
    }
}

extension View {
    /// 把场景生命周期接入 app-mcp：
    ///
    /// - `scenePhase` → 可见性（`active` 可见有焦点、`inactive` 可见无焦点、`background` 隐藏）；
    ///   `idle` / `onDemand` 模式下回到 `active` 时以原因 `visible` 回连。iOS 默认策略
    ///   （`LifecyclePolicy.iOSDefault()`）在进入后台时立即休眠。
    /// - `onOpenURL` → `handleWake(url:)`：Host 通过 `<scheme>://app-mcp/wake?token=` 唤醒
    ///   （macOS 上 Host 用 `open -g`，不把窗口带到前台）。不是唤醒 URL 时交给 `onOtherURL`。
    ///
    /// ```swift
    /// WindowGroup { ContentView().appMcpLifecycle(service.client) }
    /// ```
    ///
    /// 挂在根视图上即可；SDK 不申请 `beginBackgroundTask`，进程是否驻留交给系统。
    @available(iOS 15.0, macOS 12.0, tvOS 15.0, watchOS 8.0, *)
    public func appMcpLifecycle(_ client: AppMcpClient, onOtherURL: ((URL) -> Void)? = nil) -> some View {
        modifier(AppMcpLifecycleModifier(client: client, onOtherURL: onOtherURL))
    }
}
#endif
