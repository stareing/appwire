package dev.appmcp

import java.io.Closeable
import java.util.concurrent.atomic.AtomicBoolean
import dev.appmcp.ffi.Hold as FfiHold

// 生命周期（spec/lifecycle.md）相关类型，直接复用 uniffi 生成的枚举与唤醒描述。
typealias LifecycleMode = dev.appmcp.ffi.LifecycleMode
typealias Residency = dev.appmcp.ffi.Residency
typealias WakeKind = dev.appmcp.ffi.WakeKind
typealias WakeReason = dev.appmcp.ffi.WakeReason
typealias SleepReason = dev.appmcp.ffi.SleepReason
typealias WakeDescriptor = dev.appmcp.ffi.WakeDescriptor

/**
 * 生命周期策略（spec/lifecycle.md 第 3 节）。
 *
 * @property mode `PERSISTENT`（默认，不休眠）/ `IDLE`（空闲后休眠，唤醒后回连）/ `ON_DEMAND`（启动时不连接）。
 * @property idleTimeoutMillis `IDLE` 模式下空闲多久进入休眠。
 * @property hiddenIdleTimeoutMillis 可见性为 hidden / frozen 时的空闲时间（与模式超时取较小值）。
 * @property graceMillis `ON_DEMAND` 模式下任务完成后保留连接的时间。
 * @property residency 休眠后的进程驻留策略；允许退出时回调 [AppMcpConfig.onIdleExit]。
 * @property wake 本实例的唤醒描述，随 `app/sleep` 上报；为空时 Host 回退到清单 `launch`。
 */
data class LifecyclePolicy(
    val mode: LifecycleMode = LifecycleMode.PERSISTENT,
    val idleTimeoutMillis: Long = 60_000,
    val hiddenIdleTimeoutMillis: Long = 15_000,
    val graceMillis: Long = 10_000,
    val residency: Residency = Residency.KEEP,
    val wake: WakeDescriptor? = null,
) {
    internal fun toFfi() = dev.appmcp.ffi.LifecyclePolicy(
        mode = mode,
        idleTimeoutMs = idleTimeoutMillis.coerceAtLeast(0).toULong(),
        hiddenIdleTimeoutMs = hiddenIdleTimeoutMillis.coerceAtLeast(0).toULong(),
        graceMs = graceMillis.coerceAtLeast(0).toULong(),
        residency = residency,
        wake = wake,
    )
}

/**
 * 阻止自动休眠的持有（[AppMcp.hold]、[ToolContext.hold]）。[close] / [release] 幂等。
 *
 * ```kotlin
 * client.hold().use { longRunningSync() }
 * ```
 */
class HoldHandle internal constructor(private val inner: FfiHold) : Closeable {
    private val released = AtomicBoolean(false)

    val isReleased: Boolean get() = released.get()

    /** 释放持有（同时释放原生对象）。 */
    fun release() {
        if (released.compareAndSet(false, true)) {
            inner.release()
            inner.close()
        }
    }

    override fun close() = release()
}

/** 从操作系统激活参数 / URL 中提取唤醒令牌；不是唤醒参数时返回 null。 */
fun parseWakeToken(args: String): String? = dev.appmcp.ffi.parseWakeToken(args)
