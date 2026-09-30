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
 */
class WakeWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val token = inputData.getString(KEY_TOKEN)
        if (token == null || !WakeReceiver.isValidToken(token)) return Result.failure()
        val target = AppMcpAndroid.resolveTarget(applicationContext)
        if (target == null) {
            Log.w(TAG, "没有可用的 app-mcp 客户端：请在 Application 中实现 AppMcpProvider")
            return Result.failure()
        }
        val recognized = target.handleWakeAndAwaitSleep("app-mcp-wake:$token", AppMcpAndroid.wakeWorkTimeoutMillis)
        return if (recognized) Result.success() else Result.failure()
    }

    companion object {
        const val KEY_TOKEN = "token"
        const val TAG = "app-mcp-wake"
        const val UNIQUE_NAME = "app-mcp-wake"
    }
}
