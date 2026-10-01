import AppMcpBindings
import Foundation

/// 生命周期策略（spec/lifecycle.md 第 3 节）。
///
/// - `persistent`：不休眠（未给唤醒方式的桌面平台默认）。
/// - `idle`：启动即连接；空闲后与 Host 完成 `app/sleep` 握手并断开；被唤醒后回连（给了唤醒方式的桌面平台默认）。
/// - `onDemand`：启动时不连接；回到前台（`setPhase(.active)`）、`handleWake` / `connectNow()` 时连接，
///   调用完成后经过合并窗口、连上后无调用经过 `graceMs` 休眠（iOS / tvOS / visionOS 默认）。
public struct LifecyclePolicy: Sendable, Equatable {
    public var mode: LifecycleMode
    /// 空闲多久进入休眠（`idle`）。
    public var idleTimeoutMs: UInt64
    /// 可见性为 hidden / frozen 时使用的空闲时间（与模式对应的超时取较小值）。
    public var hiddenIdleTimeoutMs: UInt64
    /// `onDemand` 下任务完成后保留连接的时间。
    public var graceMs: UInt64
    public var residency: Residency
    /// 本实例的唤醒描述，随 `app/sleep` 上报；为 `nil` 时 Host 回退到清单 `launch`。
    /// 见 `WakeDescriptor.urlScheme(_:background:)`。
    public var wake: WakeDescriptor?
    /// `idle` / `onDemand` 下连续多少次"Host 不在"后停止重连、进入 `dormant`；0 = 一直重连（第 11 节 A2）。
    public var hostAbsentRetries: UInt32
    /// 回退到 4e 之前的定时器行为（第 11、13 节）。
    public var legacyTimers: Bool
    /// 调用 / 资源读取完成后的合并窗口：之后是否在线只由 Host 租约决定（第 13 节 B1）。
    public var mergeWindowMs: UInt64
    /// `idle` / `onDemand` 下进入后台且空闲时立即休眠，不等租约（第 13 节 B4）。iOS 默认开启。
    public var sleepOnBackground: Bool

    public init(
        mode: LifecycleMode = .persistent,
        idleTimeoutMs: UInt64 = 60_000,
        hiddenIdleTimeoutMs: UInt64 = 15_000,
        graceMs: UInt64 = 10_000,
        residency: Residency = .keep,
        wake: WakeDescriptor? = nil,
        hostAbsentRetries: UInt32 = 3,
        legacyTimers: Bool = false,
        mergeWindowMs: UInt64 = 2_000,
        sleepOnBackground: Bool = false
    ) {
        self.mode = mode
        self.idleTimeoutMs = idleTimeoutMs
        self.hiddenIdleTimeoutMs = hiddenIdleTimeoutMs
        self.graceMs = graceMs
        self.residency = residency
        self.wake = wake
        self.hostAbsentRetries = hostAbsentRetries
        self.legacyTimers = legacyTimers
        self.mergeWindowMs = mergeWindowMs
        self.sleepOnBackground = sleepOnBackground
    }

    /// 不休眠。
    public static let persistent = LifecyclePolicy()

    /// iOS 推荐（spec/lifecycle.md 第 13 节 B1「平台默认」）：`onDemand` + `sleepOnBackground`，进入后台（`hidden`）
    /// 立即休眠——iOS 没有后台唤醒，后台期间的调用走 App Intents。
    public static func iOSDefault(wake: WakeDescriptor? = nil) -> LifecyclePolicy {
        LifecyclePolicy(mode: .onDemand, hiddenIdleTimeoutMs: 0, wake: wake, sleepOnBackground: true)
    }

    /// 当前平台的默认策略：iOS / tvOS / visionOS 为 `iOSDefault(wake:)`；其他平台给了 `wake` 时为 `idle`
    /// （默认 2 s 合并窗口），否则为 `persistent`。
    ///
    /// @why 桌面 `idle` 休眠后只能经唤醒描述回连；没有唤醒描述时 Host 只能按清单 `launch` 冷启动新实例或失败，
    /// 所以不给 `wake` 的桌面默认保持 `persistent`。
    public static func platformDefault(wake: WakeDescriptor?) -> LifecyclePolicy {
        #if os(iOS) || os(tvOS) || os(visionOS)
        return iOSDefault(wake: wake)
        #else
        guard let wake else { return .persistent }
        return LifecyclePolicy(mode: .idle, wake: wake)
        #endif
    }

    /// `platformDefault(wake: nil)`。
    public static var platformDefault: LifecyclePolicy { platformDefault(wake: nil) }

    var ffi: AppMcpBindings.LifecyclePolicy {
        AppMcpBindings.LifecyclePolicy(
            mode: mode, idleTimeoutMs: idleTimeoutMs, hiddenIdleTimeoutMs: hiddenIdleTimeoutMs,
            graceMs: graceMs, residency: residency, wake: wake,
            hostAbsentRetries: hostAbsentRetries, legacyTimers: legacyTimers,
            mergeWindowMs: mergeWindowMs, sleepOnBackground: sleepOnBackground
        )
    }
}

extension WakeDescriptor {
    /// 自定义 URL scheme 唤醒：Host 打开 `<scheme>://app-mcp/wake?token=<t>`，App 在 `onOpenURL`
    /// （或 `.appMcpLifecycle(client)` 修饰器）里交给 `handleWake(url:)`。
    ///
    /// - macOS：Host 用 `open -g <scheme>://app-mcp/wake?token=<t>` 投递（`-g` 不激活 App、不把窗口带到前台），
    ///   因此 `background` 应为 `true`；App 需在 Info.plist 的 `CFBundleURLTypes` 中注册该 scheme。
    ///   AppKit App 可改用 `NSAppleEventManager` 处理 `kAEGetURL`，同样把 URL 交给 `handleWake(url:)`。
    /// - iOS：系统打开 URL 时总会把 App 带到前台，`background` 应为 `false`。
    public static func urlScheme(_ scheme: String, background: Bool? = nil) -> WakeDescriptor {
        #if os(macOS)
        let bg = background ?? true
        #else
        let bg = background ?? false
        #endif
        return WakeDescriptor(kind: .uri, target: scheme, background: bg)
    }
}

/// 阻止自动休眠的持有（`AppMcpClient.hold()`、`ToolContext.hold()`）。
/// `release()` 幂等；对象释放（deinit）时自动释放。
public final class SleepHold: @unchecked Sendable {
    private let inner: Hold

    init(_ inner: Hold) { self.inner = inner }

    /// 释放持有。重复调用无效果。
    public func release() { inner.release() }

    deinit { inner.release() }
}

/// 平台无关的前后台阶段（SwiftUI `ScenePhase` / UIKit / AppKit 通知都可映射到它）。
public enum AppPhase: Sendable, Equatable {
    /// 前台、可交互。
    case active
    /// 可见但不可交互（被系统界面遮挡、切换中、窗口失焦）。
    case inactive
    /// 不可见（进入后台、最小化）。
    case background
}

/// 某个阶段对应的可见性上报与是否主动回连。
struct PhaseAction: Equatable {
    let visibility: Visibility
    let focused: Bool
    /// 回到前台时回连（原因 `visible`）；客户端未休眠时原生层 `wake` 无效果。
    let wake: Bool
}

func phaseAction(_ phase: AppPhase, mode: LifecycleMode) -> PhaseAction {
    switch phase {
    case .active: return PhaseAction(visibility: .visible, focused: true, wake: mode != .persistent)
    case .inactive: return PhaseAction(visibility: .visible, focused: false, wake: false)
    case .background: return PhaseAction(visibility: .hidden, focused: false, wake: false)
    }
}

extension AppMcpClient {
    /// 上报前后台阶段：设置可见性；`idle` / `onDemand` 模式下进入前台时以原因 `visible` 回连（`onDemand` 由此在前台
    /// 连上；须在 `start()` 之后，未启动时的唤醒被忽略——已在前台时再启动请用 `connectNow()`）。
    public func setPhase(_ phase: AppPhase) {
        let action = phaseAction(phase, mode: lifecycle.mode)
        setVisibility(action.visibility, focused: action.focused)
        if action.wake { _ = wake(reason: .visible) }
    }
}
