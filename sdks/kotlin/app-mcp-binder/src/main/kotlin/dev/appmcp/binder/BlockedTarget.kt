package dev.appmcp.binder

import android.content.Intent
import android.net.Uri
import android.provider.Settings

/**
 * 被系统拦截绑定的目标 App（[NamingCodes.ACTIVATION_BLOCKED]）：已安装、组件存在，但系统（关联启动 / 自启动管控）
 * 不允许本应用绑定它。需要用户本人在系统设置中放行后重试。
 *
 * @security 只含包名与面向用户的应用名，不含组件类名等内部信息。
 */
data class BlockedTarget(
    /** 被拦截的包名。 */
    val packageName: String,
    /** 面向用户的应用名（`ApplicationInfo.loadLabel`）。 */
    val appLabel: String,
) {
    /** 面向用户的一句提示（Agent / 界面可直接展示）。 */
    val userMessage: String
        get() = "系统阻止了本应用启动『$appLabel』。请在系统设置中允许『$appLabel』自启动 / 关联启动后重试。"

    /** 打开该 App 的系统设置详情页（[settingsIntent]）。 */
    fun settingsIntent(): Intent = settingsIntent(packageName)

    companion object {
        /**
         * 系统标准的应用详情设置页（`Settings.ACTION_APPLICATION_DETAILS_SETTINGS`，`package:<包名>`）。不打开厂商私有页面：
         * 其动作 / 组件名未公开且随版本变化；自启动 / 关联启动开关在多数 ROM 上可从详情页进入。
         * 带 `FLAG_ACTIVITY_NEW_TASK`，可从非 Activity 上下文（Service、通知）启动。
         */
        @JvmStatic
        fun settingsIntent(packageName: String): Intent =
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
