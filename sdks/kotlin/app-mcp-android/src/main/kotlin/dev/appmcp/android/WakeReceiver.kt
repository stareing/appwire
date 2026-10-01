package dev.appmcp.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequest
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager

/**
 * 接收 Host 的唤醒广播（spec/lifecycle.md 第 5 节 Android 行）：
 *
 * ```
 * adb shell am broadcast -a dev.appmcp.action.WAKE -n <package>/dev.appmcp.android.WakeReceiver --es token <t>
 * ```
 *
 * - App 在前台，或客户端已连接 / 正在连接（[WakeTarget.isLinkActive]）：直接交给已运行的客户端（`handleWake`），
 *   立即返回，不排后台任务。客户端已连接时令牌被丢弃（Host 按实例 ID 认领现有连接，spec/lifecycle.md 第 3 节），
 *   休眠握手进行中则在休眠完成后回连；
 * - 否则 `goAsync()`，入队一个**加急** WorkManager 任务 [WakeWorker]（配额不足时按普通任务运行），
 *   入队完成后结束广播。Worker 回连、等调用完成且再次休眠后结束，进程交给系统回收。
 *
 * 不启动前台服务、不持有 WakeLock（Android 12+ 禁止后台启动前台服务，加急任务是官方替代）。
 *
 * 入队时记下收到广播的本机时刻（[WakeWorker.KEY_RECEIVED_AT]），过期的任务由 [WakeWorker] 丢弃。
 *
 * 安全：receiver 需要 `exported=true` 才能收到 Host（adb shell / 桌面伴侣进程）的显式广播。令牌格式不合法的广播
 * 直接丢弃；合法格式的伪造广播最多触发一次回连——Host 只接受自己签发的一次性令牌（≥128 位、60 秒有效），
 * 不会因此派发调用。需要进一步限制调用方时，在 App 的 manifest 中为本 receiver 加 `android:permission`
 * （见 README / AndroidManifest 注释）。
 */
class WakeReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != null && intent.action != AppMcpAndroid.ACTION_WAKE) return
        val token = intent.getStringExtra(AppMcpAndroid.EXTRA_TOKEN)
        if (token == null || !isValidToken(token)) {
            Log.w(TAG, "忽略格式不合法的唤醒令牌")
            return
        }
        val args = "app-mcp-wake:$token"
        // 有界面或连接已在：交给运行中的客户端，不需要后台任务。
        val target = AppMcpAndroid.wakeTarget
        if (target != null && (AppMcpAndroid.isForeground() || target.isLinkActive())) {
            target.handleWake(args)
            return
        }
        val pending = goAsync()
        try {
            val op = WorkManager.getInstance(context.applicationContext)
                .enqueueUniqueWork(
                    WakeWorker.UNIQUE_NAME,
                    ExistingWorkPolicy.REPLACE,
                    buildRequest(token, receivedAtMillis = System.currentTimeMillis()),
                )
            val result = op.result
            if (pending == null) return
            result.addListener({ pending.finish() }, { it.run() })
        } catch (e: Throwable) {
            Log.e(TAG, "无法入队唤醒任务", e)
            pending?.finish()
        }
    }

    companion object {
        private const val TAG = "AppMcpWake"
        private val TOKEN = Regex("[A-Za-z0-9._~-]{1,512}")

        /** 令牌格式：1–512 个 `[A-Za-z0-9._~-]`（与原生层 `parseWakeToken` 一致）。 */
        @JvmStatic
        fun isValidToken(token: String): Boolean = TOKEN.matches(token)

        /**
         * 唤醒任务。Android 12+ 为加急任务（配额不足时按普通任务运行）；Android 11 及以下加急任务需要前台服务
         * 通知（`getForegroundInfo`），这里不申请前台服务，改为普通任务（这些版本对后台任务限制较少，通常立即运行）。
         */
        @JvmStatic
        @JvmOverloads
        fun buildRequest(
            token: String,
            sdkInt: Int = Build.VERSION.SDK_INT,
            receivedAtMillis: Long? = null,
        ): OneTimeWorkRequest {
            val input = Data.Builder().putString(WakeWorker.KEY_TOKEN, token)
            if (receivedAtMillis != null) input.putLong(WakeWorker.KEY_RECEIVED_AT, receivedAtMillis)
            val builder = OneTimeWorkRequest.Builder(WakeWorker::class.java)
                .setInputData(input.build())
                .addTag(WakeWorker.TAG)
            if (sdkInt >= Build.VERSION_CODES.S) {
                builder.setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
            }
            return builder.build()
        }
    }
}
