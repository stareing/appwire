package dev.appmcp.android

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.LifecycleMode
import dev.appmcp.LifecyclePolicy
import dev.appmcp.LogLevel
import dev.appmcp.Residency
import dev.appmcp.StateStatus
import dev.appmcp.Visibility
import dev.appmcp.WakeDescriptor
import dev.appmcp.WakeKind
import dev.appmcp.WakeOutcome
import dev.appmcp.WakeReason
import kotlinx.coroutines.Dispatchers

/**
 * 由 `Application` 实现：进程因唤醒广播冷启动、且还没有客户端时，[WakeWorker] 通过它取得客户端。
 *
 * ```kotlin
 * class App : Application(), AppMcpProvider {
 *     val mcp by lazy { AppMcpAndroid.create(this, AppMcpConfig("shop", "Shop")).start() }
 *     override fun appMcp() = mcp
 * }
 * ```
 */
fun interface AppMcpProvider {
    fun appMcp(): AppMcp
}

/**
 * 唤醒的接收方。默认由 [AppMcpAndroid.create] 注册为包装 [AppMcp] 的实现；测试或自定义宿主可替换。
 */
interface WakeTarget {
    /** 立即处理唤醒参数（App 在前台时由 [WakeReceiver] 直接调用）。 */
    fun handleWake(args: String): Boolean

    /** 处理唤醒参数并挂起到任务完成、再次休眠（由 [WakeWorker] 调用）。不是唤醒参数时返回 false。 */
    suspend fun handleWakeAndAwaitSleep(args: String, timeoutMillis: Long): Boolean

    /**
     * 连接是否已建立或正在建立（已连接、连接中、握手中、等待配对、回连中）。为 true 时唤醒令牌没有用途：
     * Host 按实例 ID 认领这条连接，[WakeReceiver] / [WakeWorker] 不再排后台任务、不等待休眠。
     * 默认 false（按休眠处理，兼容旧实现）。
     */
    fun isLinkActive(): Boolean = false
}

/** 把 [AppMcp] 适配为 [WakeTarget]。 */
class AppMcpWakeTarget(val client: AppMcp) : WakeTarget {
    override fun handleWake(args: String) = client.handleWake(args)
    override suspend fun handleWakeAndAwaitSleep(args: String, timeoutMillis: Long) =
        client.handleWakeAndAwaitSleep(args, timeoutMillis) != WakeOutcome.NOT_A_WAKE

    override fun isLinkActive(): Boolean = isLinkActiveStatus(client.currentState().status)

    companion object {
        /**
         * 该状态下连接是否已建立或正在建立。`BACKOFF` 不算：令牌会让客户端立即重连（Host 刚恢复时有用）；
         * `DORMANT` / `IDLE` / `STOPPED` / `REJECTED` / `HOST_MISMATCH` 也不算（交给客户端按状态处理）。
         */
        @JvmStatic
        fun isLinkActiveStatus(status: StateStatus): Boolean = when (status) {
            StateStatus.CONNECTED,
            StateStatus.CONNECTING,
            StateStatus.HANDSHAKING,
            StateStatus.PENDING_PAIRING,
            StateStatus.WAKING -> true
            StateStatus.IDLE,
            StateStatus.BACKOFF,
            StateStatus.REJECTED,
            StateStatus.STOPPED,
            StateStatus.DORMANT,
            StateStatus.HOST_MISMATCH -> false
        }
    }
}

/**
 * Android 接入辅助。
 *
 * - handler 默认在 `Dispatchers.Main` 上执行（可在配置中覆盖）；
 * - 未设置 `instanceTitle` 时使用应用名；
 * - 生命周期默认 `ON_DEMAND` + `sleepOnBackground` + `KEEP`（spec/lifecycle.md 第 13 节 B1「平台默认」），
 *   唤醒描述自动填为 `android-intent` → `<package>/dev.appmcp.android.WakeReceiver`（可后台唤醒）；
 *   显式传入的 `lifecycle` 原样使用（只在 `wake` 为空时补唤醒描述）；
 * - 跟随进程前后台切换上报可见性（`ProcessLifecycleOwner`）；进入前台（含启动后第一次 `ON_START`）时以 `visible`
 *   原因回连，`ON_DEMAND` 由此在前台连上，进入后台空闲即休眠；
 * - 注册为进程级 [wakeTarget]，供 [WakeReceiver] / [WakeWorker] 使用。
 *
 * 不申请前台服务或 WakeLock：休眠后进程交给系统回收；Host 需要时通过显式广播唤醒（见 [WakeReceiver]）。
 */
object AppMcpAndroid {
    /** 唤醒广播的 action。 */
    const val ACTION_WAKE = "dev.appmcp.action.WAKE"

    /** 唤醒令牌所在的 extra。 */
    const val EXTRA_TOKEN = "token"

    /** [WakeReceiver] 的类名（WakeDescriptor.target 的组件部分）。 */
    const val RECEIVER_CLASS = "dev.appmcp.android.WakeReceiver"

    /**
     * 唤醒广播在本机的最长有效时间：[WakeWorker] 开始运行时距 [WakeReceiver] 收到广播超过此值则丢弃，
     * 不创建 / 唤醒客户端（如被强制停止后 WorkManager 重新排入的旧任务）。默认 60 秒，与 Host 唤醒令牌有效期
     * （spec/lifecycle.md 4.4、`HubConfig.wake_token_ttl`）一致；Host 改了有效期时同步调整。`<= 0` 关闭检查。
     */
    @JvmStatic
    @Volatile
    var wakeTokenMaxAgeMillis: Long = 60_000

    /** [WakeWorker] 等待再次休眠的最长时间。 */
    @JvmStatic
    @Volatile
    var wakeWorkTimeoutMillis: Long = 120_000

    /** 进程级唤醒接收方；[AppMcpAndroid.create] 自动设置。 */
    @JvmStatic
    @Volatile
    var wakeTarget: WakeTarget? = null

    /** 最近一次 [create] 创建的客户端（已关闭时为 null）。 */
    @JvmStatic
    val client: AppMcp?
        get() = (wakeTarget as? AppMcpWakeTarget)?.client?.takeUnless { it.isClosed }

    /** 本 App 的唤醒描述：显式广播到 [WakeReceiver]，可在后台唤醒。 */
    @JvmStatic
    fun wakeDescriptor(context: Context): WakeDescriptor =
        WakeDescriptor(WakeKind.ANDROID_INTENT, "${context.packageName}/$RECEIVER_CLASS", true)

    /** Android 默认生命周期策略：`ON_DEMAND`、`sleepOnBackground`、`KEEP`、自动唤醒描述。 */
    @JvmStatic
    fun defaultLifecycle(context: Context): LifecyclePolicy = LifecyclePolicy(
        mode = LifecycleMode.ON_DEMAND,
        residency = Residency.KEEP,
        wake = wakeDescriptor(context),
        sleepOnBackground = true,
    )

    /** 生效的策略：未提供时为 [defaultLifecycle]；提供时原样使用，只在 `wake` 为空时补 [wakeDescriptor]。 */
    @JvmStatic
    fun resolveLifecycle(context: Context, configured: LifecyclePolicy?): LifecyclePolicy =
        configured?.let { if (it.wake == null) it.copy(wake = wakeDescriptor(context)) else it }
            ?: defaultLifecycle(context)

    /**
     * 创建客户端并跟随进程前后台。`ON_DEMAND` 下 `start()` 不连接，由之后第一次进入前台连上：请在创建后立即
     * `start()`（同一轮主循环内，如 `create(...).start()`）；延后启动且已在前台时改用 `connectNow()`。
     */
    @JvmStatic
    @JvmOverloads
    fun create(context: Context, config: AppMcpConfig, trackVisibility: Boolean = true): AppMcp {
        val app = context.applicationContext
        val title = config.instanceTitle ?: app.applicationInfo.loadLabel(app.packageManager).toString()
        val lifecycle = resolveLifecycle(app, config.lifecycle)
        val client = AppMcp.create(
            config.copy(
                instanceTitle = title,
                dispatcher = config.dispatcher ?: Dispatchers.Main,
                lifecycle = lifecycle,
                onLog = config.onLog ?: ::logcat,
            ),
        )
        wakeTarget = AppMcpWakeTarget(client)
        if (trackVisibility) {
            val observer = object : DefaultLifecycleObserver {
                override fun onStart(owner: LifecycleOwner) {
                    if (client.isClosed) return
                    client.setVisibility(Visibility.VISIBLE, true)
                    // 休眠中（含 ON_DEMAND 启动后未连接）进入前台：回连；已连接时只重新计时。
                    if (lifecycle.mode != LifecycleMode.PERSISTENT) client.wake(WakeReason.VISIBLE)
                }

                override fun onStop(owner: LifecycleOwner) {
                    if (!client.isClosed) client.setVisibility(Visibility.HIDDEN, false)
                }
            }
            // @why 总是投递到下一轮主循环：已在前台时 addObserver 会同步派发 ON_START，若在 `create(...).start()`
            // 的 start() 之前派发，ON_DEMAND 的 wake 落在未启动状态被丢弃，前台永远连不上。
            Handler(Looper.getMainLooper()).post { ProcessLifecycleOwner.get().lifecycle.addObserver(observer) }
        }
        return client
    }

    /** 进程是否在前台（至少一个 Activity 处于 started）。只能在主线程调用。 */
    internal fun isForeground(): Boolean = runCatching {
        ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
    }.getOrDefault(false)

    /** [AppMcpConfig.onLog] 的缺省实现：写入 Logcat（标签 [LOG_TAG]）。 */
    internal fun logcat(level: LogLevel, message: String) {
        val priority = when (level) {
            LogLevel.DEBUG -> Log.DEBUG
            LogLevel.INFO -> Log.INFO
            LogLevel.WARN -> Log.WARN
            LogLevel.ERROR -> Log.ERROR
        }
        Log.println(priority, LOG_TAG, message)
    }

    /** 缺省日志（[logcat]）使用的 Logcat 标签。 */
    const val LOG_TAG = "AppMcp"

    /** 取得唤醒接收方：已注册的 [wakeTarget]，否则由 `Application`（[AppMcpProvider]）创建。 */
    internal fun resolveTarget(context: Context): WakeTarget? {
        wakeTarget?.let { t -> if (t !is AppMcpWakeTarget || !t.client.isClosed) return t }
        val provider = context.applicationContext as? AppMcpProvider ?: return null
        val c = runCatching { provider.appMcp() }.getOrNull() ?: return null
        if (c.isClosed) return null
        return (wakeTarget as? AppMcpWakeTarget)?.takeIf { it.client === c } ?: AppMcpWakeTarget(c).also { wakeTarget = it }
    }
}
