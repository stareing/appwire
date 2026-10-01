package dev.appmcp.android

import android.content.Context
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters

/**
 * 由 [WakeReceiver] 入队的（加急）任务：把唤醒令牌交给客户端，挂起到回连、调用完成、再次休眠后结束。
 *
 * 客户端来源：[AppMcpAndroid.wakeTarget]（进程仍在），否则 `Application` 实现的 [AppMcpProvider]（冷启动）。
 * 都没有时任务失败（App 未接入唤醒）。等待超过 [AppMcpAndroid.wakeWorkTimeoutMillis] 也视为完成（不重试）。
 *
 * 不产生多余回连：
 * - 距收到广播超过 [AppMcpAndroid.wakeTokenMaxAgeMillis]（Host 侧令牌已过期，如被强制停止后 WorkManager 重新排入的
 *   旧任务）：直接结束，不创建 / 唤醒客户端；
 * - 开始时客户端已连接或正在连接（[WakeTarget.isLinkActive]）：令牌交给客户端后立即结束，不等待休眠。
 */
class WakeWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val token = inputData.getString(KEY_TOKEN)
        if (token == null || !WakeReceiver.isValidToken(token)) return Result.failure()
        if (isExpired(inputData.getLong(KEY_RECEIVED_AT, NO_TIME), System.currentTimeMillis())) {
            Log.i(TAG, "唤醒已过期，丢弃")
            return Result.success()
        }
        val target = AppMcpAndroid.resolveTarget(applicationContext)
        if (target == null) {
            Log.w(TAG, "没有可用的 app-mcp 客户端：请在 Application 中实现 AppMcpProvider")
            return Result.failure()
        }
        val args = "app-mcp-wake:$token"
        if (target.isLinkActive()) {
            target.handleWake(args)
            return Result.success()
        }
        val recognized = target.handleWakeAndAwaitSleep(args, AppMcpAndroid.wakeWorkTimeoutMillis)
        return if (recognized) Result.success() else Result.failure()
    }

    companion object {
        const val KEY_TOKEN = "token"

        /** 收到广播的本机时刻（`System.currentTimeMillis()`）；旧版本入队的任务没有此键，不做过期检查。 */
        const val KEY_RECEIVED_AT = "receivedAt"
        private const val NO_TIME = -1L

        /** 距收到广播是否已超过 [AppMcpAndroid.wakeTokenMaxAgeMillis]。时钟回拨（负值）按未过期处理。 */
        @JvmStatic
        fun isExpired(receivedAtMillis: Long, nowMillis: Long, maxAgeMillis: Long = AppMcpAndroid.wakeTokenMaxAgeMillis): Boolean =
            receivedAtMillis >= 0 && maxAgeMillis > 0 && nowMillis - receivedAtMillis > maxAgeMillis

        const val TAG = "app-mcp-wake"
        const val UNIQUE_NAME = "app-mcp-wake"
    }
}
