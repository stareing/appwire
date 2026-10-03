package dev.appmcp

import java.io.Closeable
import java.util.concurrent.atomic.AtomicBoolean

/** 用户正在操作期间写调用的处理方式（spec/protocol.md 5.3「用户正在操作」）：`REJECT`（默认）/ `QUEUE`。 */
typealias BusyPolicy = dev.appmcp.ffi.BusyPolicy

/**
 * 合并 [AppMcp.setBusy] 开关与 [AppMcp.beginBusy] 作用域计数。
 *
 * @invariant 交给核心的值 = 开关 ∨ 作用域计数 > 0；`setBusy(false)` 不结束仍在进行的作用域
 * @side-effect 每次变化都在锁内调用 [apply]，推给核心的顺序与状态变化顺序一致
 */
internal class BusyState(private val apply: (Boolean) -> Unit) {
    private val lock = Any()
    private var manual = false
    private var scopes = 0

    fun set(busy: Boolean) = synchronized(lock) {
        manual = busy
        push()
    }

    fun enter() = synchronized(lock) {
        scopes += 1
        push()
    }

    fun exit() = synchronized(lock) {
        scopes -= 1
        push()
    }

    private fun push() = apply(manual || scopes > 0)
}

/**
 * [AppMcp.beginBusy] 返回的作用域：[close] 归还一次计数（重复调用无效果）。
 */
class BusyHold internal constructor(private val state: BusyState) : Closeable {
    private val released = AtomicBoolean(false)

    val isReleased: Boolean get() = released.get()

    override fun close() {
        if (released.compareAndSet(false, true)) state.exit()
    }
}
