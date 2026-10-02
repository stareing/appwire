package dev.appmcp.android

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import dev.appmcp.NavigationResult
import dev.appmcp.UserActionReason

/**
 * 后台导航的"点此继续"通知（spec/protocol.md 3.4「后台与前台」）：Android 不允许后台 App 自行打开界面，
 * App 设了 `AppMcpConfig.navigateInBackground = true` 后，可在导航回调里发一条通知，用户点开即进入目标页面，
 * 再以 [userActionRequired] 回复 Agent。是否发通知、发什么由 App 决定（策略属于 App），本类只提供平台 API 的薄封装。
 *
 * ```kotlin
 * client.setNavigationHandler { page, params ->
 *     if (!inForeground) {
 *         val intent = Intent(Intent.ACTION_VIEW, Uri.parse("shop://$page"), context, MainActivity::class.java)
 *         AppMcpContinueNotification.post(context, "AI 助手请求打开「$page」", "点此继续", intent)
 *         return@setNavigationHandler AppMcpContinueNotification.userActionRequired("已发通知，请点开 App 继续", intent)
 *     }
 *     router(page, params)
 * }
 * ```
 *
 * @security Android 13（API 33）起需要 App 在 manifest 中声明并在运行时获得 `android.permission.POST_NOTIFICATIONS`；
 *   本库不声明该权限。未授予或用户关闭了通知时 [post] 返回 false、不发通知。
 */
object AppMcpContinueNotification {
    /** 缺省通知渠道 ID（API 26+ 不存在时以 [DEFAULT_CHANNEL_NAME] 创建，重要性 HIGH）。 */
    const val DEFAULT_CHANNEL_ID = "app_mcp_continue"
    const val DEFAULT_CHANNEL_NAME = "AI 助手：继续操作"
    /** 缺省通知 ID：同一时刻只保留最新一条"继续"通知。 */
    const val DEFAULT_NOTIFICATION_ID = 0x41_4D_43_50 // "AMCP"

    /**
     * 当前能否发通知：API 33+ 已授予 `POST_NOTIFICATIONS`，且用户没有关闭本 App 的通知；
     * [channelId] 已存在且被用户关闭（重要性 NONE）时也为 false。
     */
    @JvmStatic
    @JvmOverloads
    fun canPost(context: Context, channelId: String = DEFAULT_CHANNEL_ID): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        val manager = context.getSystemService(NotificationManager::class.java) ?: return false
        if (!manager.areNotificationsEnabled()) return false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return true
        val channel = manager.getNotificationChannel(channelId) ?: return true
        return channel.importance != NotificationManager.IMPORTANCE_NONE
    }

    /**
     * 发一条点击即打开 [intent] 的通知（点击后自动消失）。
     *
     * @input intent 打开目标页面的 Activity Intent；自动加 `FLAG_ACTIVITY_NEW_TASK`，以 `FLAG_IMMUTABLE` 包装为 PendingIntent
     * @input smallIcon 状态栏图标资源；缺省用 App 图标
     * @output 已发出返回 true；不能发（见 [canPost]）返回 false
     * @side-effect API 26+ 且 [channelId] 不存在时创建该渠道
     */
    @JvmStatic
    @JvmOverloads
    fun post(
        context: Context,
        title: String,
        text: String,
        intent: Intent,
        channelId: String = DEFAULT_CHANNEL_ID,
        notificationId: Int = DEFAULT_NOTIFICATION_ID,
        smallIcon: Int = context.applicationInfo.icon,
    ): Boolean {
        val launch = Intent(intent).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val pending = PendingIntent.getActivity(
            context, notificationId, launch, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return post(context, title, text, pending, channelId, notificationId, smallIcon)
    }

    /** 同上，由 App 自备 [PendingIntent]（如经 TaskStackBuilder 构造返回栈）。 */
    @JvmStatic
    @JvmOverloads
    fun post(
        context: Context,
        title: String,
        text: String,
        intent: PendingIntent,
        channelId: String = DEFAULT_CHANNEL_ID,
        notificationId: Int = DEFAULT_NOTIFICATION_ID,
        smallIcon: Int = context.applicationInfo.icon,
    ): Boolean {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return false
        ensureChannel(manager, channelId)
        if (!canPost(context, channelId)) return false
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, channelId)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(context).setPriority(Notification.PRIORITY_HIGH)
        }
        val notification = builder
            .setSmallIcon(smallIcon.takeIf { it != 0 } ?: android.R.drawable.ic_dialog_info)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(intent)
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_REMINDER)
            .build()
        return try {
            manager.notify(notificationId, notification)
            true
        } catch (_: SecurityException) {
            // @why 权限可能在检查后被撤销
            false
        }
    }

    /**
     * 导航回调的回复：`USER_ACTION_REQUIRED`，reason `foreground`，uri 取 [intent] 的 data（没有则不带）。
     *
     * @input message 面向用户的说明（Agent 应转告用户），如"已发通知，请点开 App 继续"
     */
    @JvmStatic
    @JvmOverloads
    fun userActionRequired(message: String, intent: Intent? = null): NavigationResult.UserActionRequired =
        NavigationResult.UserActionRequired(message, UserActionReason.FOREGROUND, intent?.data?.toString())

    private fun ensureChannel(manager: NotificationManager, channelId: String) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        if (manager.getNotificationChannel(channelId) != null) return
        val name = if (channelId == DEFAULT_CHANNEL_ID) DEFAULT_CHANNEL_NAME else channelId
        manager.createNotificationChannel(NotificationChannel(channelId, name, NotificationManager.IMPORTANCE_HIGH))
    }
}
