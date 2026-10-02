package dev.appmcp.binder

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Build
import android.os.IBinder
import android.os.Looper
import android.os.ParcelFileDescriptor
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 绑定一个导出的 Service 并换得一条 fd 通道（spec/naming.md 4.2 拨号）：`bindService` → 等 `onServiceConnected` →
 * [FdChannelClient.open] 一次 → 返回 fd 与解绑句柄。之后不再使用该 `IBinder` 代理。
 *
 * 绑定标志见 [lowImpactBindFlags]：不把目标抬到调用方的优先级，内存紧张时目标照常被回收（spec/naming.md 第 8 节）。
 */
class ServiceChannelDialer(private val context: Context) {

    /** 一次成功的拨号。[release] 解除绑定（幂等）；fd 由调用方持有 / 交出。 */
    class Dialed internal constructor(
        val fd: ParcelFileDescriptor,
        private val unbind: () -> Unit,
    ) {
        private val released = AtomicBoolean(false)

        /** 解除绑定（只生效一次）。 */
        fun release() {
            if (released.compareAndSet(false, true)) unbind()
        }

        val isReleased: Boolean get() = released.get()
    }

    /**
     * 阻塞直到得到通道或失败（不得在主线程调用：绑定回调在主线程送达）。
     *
     * @error `bindService` 返回 false（组件不存在、被系统 / OEM 拦截）→ `ACTIVATION_DENIED`；`SecurityException` →
     * `BIND_PERMISSION_DENIED`；[timeoutMillis] 内未连上 → `ACTIVATION_TIMEOUT`；Service 返回空 Binder → `NAME_NOT_FOUND`；
     * 打开被拒 → 对端给出的码（如 `HUB_NOT_TRUSTED`、`CHANNEL_LIMIT`）。失败时已解除绑定。
     */
    fun dial(intent: Intent, descriptor: String, timeoutMillis: Long, instance: String? = null): Dialed {
        check(Looper.myLooper() != Looper.getMainLooper()) { "ServiceChannelDialer.dial 不能在主线程调用" }
        val connection = OneShotConnection()
        val unbind = { runCatching { context.unbindService(connection) }; Unit }
        val bound = try {
            context.bindService(intent, connection, lowImpactBindFlags())
        } catch (e: SecurityException) {
            throw ChannelOpenException("BIND_PERMISSION_DENIED", "无权绑定 ${intent.component ?: intent.action}：${e.message}", e)
        }
        if (!bound) {
            unbind()
            throw ChannelOpenException(
                "ACTIVATION_DENIED",
                "系统拒绝绑定 ${intent.component ?: intent.action}（组件不存在，或被系统的关联启动 / 自启动管控拦截）",
            )
        }
        val binder = try {
            connection.await(timeoutMillis)
        } catch (e: ChannelOpenException) {
            unbind()
            throw e
        }
        val fd = try {
            FdChannelClient.open(binder, descriptor, instance)
        } catch (e: ChannelOpenException) {
            unbind()
            throw e
        }
        return Dialed(fd, unbind)
    }

    /** 只接受第一次连接结果的 [ServiceConnection]；不保存连接之后的 Binder。 */
    private class OneShotConnection : ServiceConnection {
        private val latch = CountDownLatch(1)

        @Volatile private var binder: IBinder? = null

        @Volatile private var failure: ChannelOpenException? = null

        override fun onServiceConnected(name: ComponentName, service: IBinder) {
            binder = service
            latch.countDown()
        }

        override fun onServiceDisconnected(name: ComponentName) {
            // 通道已建立时由 fd EOF 感知；建立之前断开按激活失败处理。
            if (latch.count > 0) failure = ChannelOpenException("ACTIVATION_DENIED", "$name 在连接前断开")
            latch.countDown()
        }

        override fun onNullBinding(name: ComponentName) {
            failure = ChannelOpenException("NAME_NOT_FOUND", "$name 没有返回 Binder（未提供该 Intent 动作的服务）")
            latch.countDown()
        }

        override fun onBindingDied(name: ComponentName) {
            if (latch.count > 0) failure = ChannelOpenException("ACTIVATION_DENIED", "$name 的绑定已失效")
            latch.countDown()
        }

        fun await(timeoutMillis: Long): IBinder {
            if (!latch.await(timeoutMillis, TimeUnit.MILLISECONDS)) {
                throw ChannelOpenException("ACTIVATION_TIMEOUT", "$timeoutMillis ms 内没有连上（进程启动过慢或被系统挂起）")
            }
            failure?.let { throw it }
            // @why 释放引用：返回后连接对象仍被系统持有到解绑，不让它钉住远端代理。
            return binder.also { binder = null } ?: throw ChannelOpenException("ACTIVATION_DENIED", "没有得到 Binder")
        }
    }

    companion object {
        /**
         * `BIND_AUTO_CREATE | BIND_WAIVE_PRIORITY | BIND_ALLOW_OOM_MANAGEMENT | BIND_NOT_FOREGROUND`，API 29+ 加
         * `BIND_NOT_PERCEPTIBLE`（spec/naming.md 第 8 节）。不加 `BIND_ALLOW_ACTIVITY_STARTS`。
         */
        @JvmStatic
        @JvmOverloads
        fun lowImpactBindFlags(sdkInt: Int = Build.VERSION.SDK_INT): Int {
            var flags = Context.BIND_AUTO_CREATE or Context.BIND_WAIVE_PRIORITY or
                Context.BIND_ALLOW_OOM_MANAGEMENT or Context.BIND_NOT_FOREGROUND
            if (sdkInt >= Build.VERSION_CODES.Q) flags = flags or Context.BIND_NOT_PERCEPTIBLE
            return flags
        }
    }
}
