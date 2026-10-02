package dev.appmcp.hubapp

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import dev.appmcp.binder.BlockedTarget

/**
 * Hub → App 的绑定被系统拦截时的通知（可选提示）：点开即该 App 的系统设置详情页（[BlockedTarget.settingsIntent]）。
 *
 * - 只在已有通知权限时发送；没有权限不申请、不弹界面（Agent 已从调用错误得到同样的提示，spec/protocol.md 第 4 节）。
 * - 同一个包只保留一条（以包名为通知 ID，重复失败只更新），不常驻、不加定时器。
 */
internal object BlockedNotice {
    private const val TAG = "AppWireHub"
    private const val CHANNEL_ID = "blocked-launch"

    fun post(context: Context, target: BlockedTarget) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        if (!canNotify(context, nm)) {
            Log.i(TAG, "没有通知权限，不提示 ${target.packageName} 被拦截（调用方已收到 USER_ACTION_REQUIRED）")
            return
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "被系统拦截的启动", NotificationManager.IMPORTANCE_DEFAULT),
            )
        }
        val open = PendingIntent.getActivity(
            context,
            target.packageName.hashCode(),
            target.settingsIntent(),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(context)
        }
        val message = "系统阻止了 AppWire Hub 启动『${target.appLabel}』。点此打开其设置，允许自启动 / 关联启动后重试。"
        val notification = builder
            .setSmallIcon(android.R.drawable.stat_notify_error)
            .setContentTitle("需要允许『${target.appLabel}』被启动")
            .setContentText(message)
            .setStyle(Notification.BigTextStyle().bigText(message))
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        runCatching { nm.notify(target.packageName, NOTIFICATION_ID, notification) }
            .onFailure { Log.w(TAG, "无法发出通知：${it.message}") }
    }

    /** 已有通知权限（API 33+ 运行时权限已授予，且用户没有关闭本应用的通知）。 */
    internal fun canNotify(context: Context, nm: NotificationManager): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        return nm.areNotificationsEnabled()
    }

    /** 以包名为 tag 区分，ID 固定。 */
    private const val NOTIFICATION_ID = 1
}
