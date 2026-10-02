package dev.appmcp.android

import android.Manifest
import android.app.Application
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import dev.appmcp.NavigationResult
import dev.appmcp.UserActionReason
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** "点此继续"通知：权限授予 / 未授予 / 通知被关闭，以及导航回复。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class AppMcpContinueNotificationTest {
    private val app: Application = ApplicationProvider.getApplicationContext()
    private val manager = app.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    private val intent = Intent(Intent.ACTION_VIEW, Uri.parse("shop://cart"))

    @Test
    fun postsWhenPermissionGranted() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        assertTrue(AppMcpContinueNotification.post(app, "打开购物车", "点此继续", intent))
        val posted = shadowOf(manager).allNotifications
        assertEquals(1, posted.size)
        assertEquals(AppMcpContinueNotification.DEFAULT_CHANNEL_ID, posted[0].channelId)
        assertNotNull(posted[0].contentIntent)
        assertNotNull(manager.getNotificationChannel(AppMcpContinueNotification.DEFAULT_CHANNEL_ID))
    }

    @Test
    fun refusesWithoutPermission() {
        shadowOf(app).denyPermissions(Manifest.permission.POST_NOTIFICATIONS)
        assertFalse(AppMcpContinueNotification.canPost(app))
        assertFalse(AppMcpContinueNotification.post(app, "打开购物车", "点此继续", intent))
        assertTrue(shadowOf(manager).allNotifications.isEmpty())
    }

    @Test
    fun refusesWhenNotificationsDisabled() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        shadowOf(manager).setNotificationsEnabled(false)
        assertFalse(AppMcpContinueNotification.post(app, "打开购物车", "点此继续", intent))
        assertTrue(shadowOf(manager).allNotifications.isEmpty())
    }

    @Test
    @Config(sdk = [28])
    fun postsBeforeRuntimePermissionExisted() {
        assertTrue(AppMcpContinueNotification.post(app, "打开购物车", "点此继续", intent, channelId = "custom"))
        assertEquals("custom", shadowOf(manager).allNotifications.single().channelId)
    }

    @Test
    fun navigationReplyCarriesForegroundAndUri() {
        assertEquals(
            NavigationResult.UserActionRequired("请点开通知", UserActionReason.FOREGROUND, "shop://cart"),
            AppMcpContinueNotification.userActionRequired("请点开通知", intent),
        )
        assertNull(AppMcpContinueNotification.userActionRequired("请点开通知", Intent()).uri)
    }
}
