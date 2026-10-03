import AppMcpBindings
import Foundation

/// 用户正在操作期间写调用的处理方式（spec/protocol.md 5.3「用户正在操作」）：`.reject`（默认）/ `.queue`。
public typealias BusyPolicy = AppMcpBindings.BusyPolicy

/// 合并 `setBusy(_:)` 开关与 `beginBusy()` 作用域计数。
/// @invariant 交给核心的值 = 开关 ∨ 作用域计数 > 0；`setBusy(false)` 不结束仍在进行的作用域
/// @side-effect 每次变化都在锁内调用 `apply`，推给核心的顺序与状态变化顺序一致
final class BusyState: @unchecked Sendable {
    private let lock = NSLock()
    private let apply: (Bool) -> Void
    private var manual = false
    private var scopes = 0

    init(apply: @escaping (Bool) -> Void) { self.apply = apply }

    func set(_ busy: Bool) { update { manual = busy } }
    func enter() { update { scopes += 1 } }
    func exit() { update { scopes -= 1 } }

    private func update(_ change: () -> Void) {
        lock.lock()
        defer { lock.unlock() }
        change()
        apply(manual || scopes > 0)
    }
}

/// `AppMcpClient.beginBusy()` 返回的忙碌作用域：`release()` 归还一次计数（幂等）；对象释放（deinit）时自动归还。
public final class BusyHold: @unchecked Sendable {
    private let state: BusyState
    private let lock = NSLock()
    private var released = false

    init(_ state: BusyState) { self.state = state }

    public var isReleased: Bool {
        lock.lock()
        defer { lock.unlock() }
        return released
    }

    /// 归还作用域。重复调用无效果。
    public func release() {
        lock.lock()
        let first = !released
        released = true
        lock.unlock()
        if first { state.exit() }
    }

    deinit { release() }
}

extension AppMcpClient {
    /// 声明用户正在 / 不再在 App 内操作（何时算由 App 决定，如编辑框获得焦点、拖拽中）。期间写调用（生效注解不是
    /// `readOnlyHint: true` 的工具）按 `AppMcpConfig.busyPolicy` 拒绝或排队；只读调用不受影响。状态只在 SDK 内，不发给 Host。
    /// 与 `beginBusy()` / `withBusy` 作用域合并：生效值为「本开关 ∨ 仍有作用域未结束」，`setBusy(false)` 不结束进行中的作用域。
    public func setBusy(_ busy: Bool) { busyState.set(busy) }

    /// 核心当前是否处于忙碌状态。
    public var isBusy: Bool { inner.isBusy() }

    /// 修改忙碌期间写调用的处理方式，随即对排队中的调用生效（如由用户在 App 设置中选择）。
    public func setBusyPolicy(_ policy: BusyPolicy) { inner.setBusyPolicy(policy: policy) }

    /// 开始一段忙碌作用域，直到返回值 `release()`（或被释放）。可嵌套、可跨线程同时持有（引用计数）：
    /// 最后一个作用域结束且 `setBusy(_:)` 开关为关时恢复空闲。
    public func beginBusy() -> BusyHold {
        busyState.enter()
        return BusyHold(busyState)
    }

    /// 作用域写法：`body` 执行期间为忙碌（含抛错退出时归还），语义同 `beginBusy()`。
    @discardableResult
    public func withBusy<T>(_ body: () throws -> T) rethrows -> T {
        let hold = beginBusy()
        defer { hold.release() }
        return try body()
    }

    /// `withBusy` 的异步版本：`await` 期间（含取消、抛错退出）保持忙碌。
    @discardableResult
    public func withBusy<T>(_ body: () async throws -> T) async rethrows -> T {
        let hold = beginBusy()
        defer { hold.release() }
        return try await body()
    }
}
