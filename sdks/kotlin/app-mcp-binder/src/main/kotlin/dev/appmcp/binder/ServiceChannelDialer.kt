package dev.appmcp.binder

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.Process
import android.util.Log
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
     * 先按组件查安装信息（不启动进程），区分"不存在"与"存在但系统拒绝绑定"：
     *
     * @error 组件不存在（未安装 / 未声明该 Service）→ `NAME_NOT_FOUND`；Service 未导出（且不是本应用）或要求本应用未获得的
     * 权限 → `BIND_PERMISSION_DENIED`；组件存在但 `bindService` 返回 false 或抛 `SecurityException`（关联启动 / 自启动管控、
     * OEM 拦截）→ `ACTIVATION_BLOCKED`（带 [BlockedTarget]，[ChannelOpenException.message] 为面向用户的提示）；
     * [timeoutMillis] 内未连上 → `ACTIVATION_TIMEOUT`；Service 返回空 Binder → `NAME_NOT_FOUND`；打开被拒 → 对端给出的码
     * （如 `HUB_NOT_TRUSTED`、`CHANNEL_LIMIT`、`HUB_UNSUPPORTED`）。失败时已解除绑定。
     */
    fun dial(intent: Intent, descriptor: String, timeoutMillis: Long, instance: String? = null): Dialed {
        check(Looper.myLooper() != Looper.getMainLooper()) { "ServiceChannelDialer.dial 不能在主线程调用" }
        val target = installedService(intent)
        val connection = OneShotConnection()
        val unbind = { runCatching { context.unbindService(connection) }; Unit }
        val bound = try {
            context.bindService(intent, connection, lowImpactBindFlags())
        } catch (e: SecurityException) {
            unbind()
            throw blocked(target, "绑定时被系统拒绝（SecurityException：${e.message}）", e)
        }
        if (!bound) {
            unbind()
            throw blocked(target, "bindService 返回 false（可能被关联启动 / 自启动管控拦截）", null)
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

    /**
     * 绑定前确认组件已安装且本应用有权绑定（只读安装信息，不启动进程）。
     *
     * @error 不存在 → `NAME_NOT_FOUND`；未导出（且不是本应用）/ 缺少所需权限 → `BIND_PERMISSION_DENIED`。
     */
    private fun installedService(intent: Intent): ServiceInfo {
        val pm = context.packageManager
        val what = intent.component?.flattenToShortString() ?: intent.action ?: intent.toString()
        val info = lookupService(pm, intent)
            ?: throw ChannelOpenException(NamingCodes.NAME_NOT_FOUND, "没有找到 $what（未安装，或未声明该 Service）")
        if (!info.exported && info.applicationInfo?.uid != Process.myUid()) {
            throw ChannelOpenException(NamingCodes.BIND_PERMISSION_DENIED, "$what 未导出，其他应用不能绑定")
        }
        val permission = info.permission
        if (permission != null && context.checkSelfPermission(permission) != PackageManager.PERMISSION_GRANTED) {
            throw ChannelOpenException(NamingCodes.BIND_PERMISSION_DENIED, "绑定 $what 需要权限 $permission，本应用未获得")
        }
        return info
    }

    private fun blocked(info: ServiceInfo, detail: String, cause: Throwable?): ChannelOpenException {
        val pm = context.packageManager
        val label = runCatching { info.applicationInfo?.loadLabel(pm)?.toString() }.getOrNull()
            ?.trim()?.takeIf { it.isNotEmpty() } ?: info.packageName
        val target = BlockedTarget(info.packageName, label)
        Log.w(TAG, "系统拒绝绑定 ${info.packageName}/${info.name}：$detail")
        return ChannelOpenException(NamingCodes.ACTIVATION_BLOCKED, target.userMessage, cause, target)
    }

    /** 只接受第一次连接结果的 [ServiceConnection]；不保存连接之后的 Binder。 */
    private class OneShotConnection : ServiceConnection {
        private val latch = CountDownLatch(1)

        @Volatile private var binder: IBinder? = null

        @Volatile private var failure: ChannelOpenException? = null

        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            binder = service
            latch.countDown()
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            // 通道已建立时由 fd EOF 感知；建立之前断开按激活失败处理。
            if (latch.count > 0) failure = ChannelOpenException(NamingCodes.ACTIVATION_DENIED, "$name 在连接前断开")
            latch.countDown()
        }

        override fun onNullBinding(name: ComponentName?) {
            failure = ChannelOpenException(NamingCodes.NAME_NOT_FOUND, "$name 没有返回 Binder（未提供该 Intent 动作的服务）")
            latch.countDown()
        }

        override fun onBindingDied(name: ComponentName?) {
            if (latch.count > 0) failure = ChannelOpenException(NamingCodes.ACTIVATION_DENIED, "$name 的绑定已失效")
            latch.countDown()
        }

        fun await(timeoutMillis: Long): IBinder {
            if (!latch.await(timeoutMillis, TimeUnit.MILLISECONDS)) {
                throw ChannelOpenException(NamingCodes.ACTIVATION_TIMEOUT, "$timeoutMillis ms 内没有连上（进程启动过慢或被系统挂起）")
            }
            failure?.let { throw it }
            // @why 释放引用：返回后连接对象仍被系统持有到解绑，不让它钉住远端代理。
            return binder.also { binder = null } ?: throw ChannelOpenException(NamingCodes.ACTIVATION_DENIED, "没有得到 Binder")
        }
    }

    companion object {
        private const val TAG = "AppMcpBinder"

        /** 按组件（或隐式 Intent 的唯一解析结果）查已安装的 Service；查不到为 null。 */
        @Suppress("DEPRECATION")
        private fun lookupService(pm: PackageManager, intent: Intent): ServiceInfo? {
            val component = intent.component
            if (component != null) {
                return try {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        pm.getServiceInfo(component, PackageManager.ComponentInfoFlags.of(0))
                    } else {
                        pm.getServiceInfo(component, 0)
                    }
                } catch (_: PackageManager.NameNotFoundException) {
                    null
                }
            }
            return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                pm.resolveService(intent, PackageManager.ResolveInfoFlags.of(0))?.serviceInfo
            } else {
                pm.resolveService(intent, 0)?.serviceInfo
            }
        }

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
