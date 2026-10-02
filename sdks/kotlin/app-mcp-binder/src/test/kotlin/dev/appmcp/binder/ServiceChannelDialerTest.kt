package dev.appmcp.binder

import android.app.Application
import android.content.ComponentName
import android.content.ContextWrapper
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Looper
import android.provider.Settings
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import java.util.concurrent.Callable
import java.util.concurrent.Executors

/**
 * 拨号失败的区分（spec/naming.md 第 12 节）：组件不存在 → `NAME_NOT_FOUND`；未导出 / 缺权限 → `BIND_PERMISSION_DENIED`；
 * 存在但系统拒绝绑定（`bindService` 返回 false / `SecurityException`）→ `ACTIVATION_BLOCKED`（面向用户的提示 + 包名与应用名）。
 */
@RunWith(RobolectricTestRunner::class)
class ServiceChannelDialerTest {
    private lateinit var app: Application
    private val pkg = "dev.example.shop"
    private val component = ComponentName(pkg, "dev.appmcp.android.ToolsService")
    private val intent get() = Intent("dev.appmcp.TOOLS").setComponent(component)

    @Before
    fun setUp() {
        app = ApplicationProvider.getApplicationContext()
    }

    private fun install(exported: Boolean = true, permission: String? = null, label: String? = "小店") {
        val appInfo = ApplicationInfo().apply {
            packageName = pkg
            uid = 10_200
            enabled = true
            nonLocalizedLabel = label
        }
        val info = ServiceInfo().apply {
            packageName = pkg
            name = component.className
            this.exported = exported
            this.permission = permission
            applicationInfo = appInfo
        }
        val spm = shadowOf(app.packageManager)
        spm.installPackage(PackageInfo().apply { packageName = pkg; applicationInfo = appInfo })
        spm.addOrUpdateService(info)
    }

    private fun dialFailure(dialer: ServiceChannelDialer = ServiceChannelDialer(app)): ChannelOpenException {
        val pool = Executors.newSingleThreadExecutor()
        try {
            val f = pool.submit(Callable { runCatching { dialer.dial(intent, FdChannel.APP_TOOLS_DESCRIPTOR, 2_000) } })
            val looper = shadowOf(Looper.getMainLooper())
            while (!f.isDone) {
                looper.idle()
                Thread.sleep(5)
            }
            val e = f.get().exceptionOrNull()
            assertTrue("应失败：$e", e is ChannelOpenException)
            return e as ChannelOpenException
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun missingComponentIsNotFoundWithoutBinding() {
        val e = dialFailure()
        assertEquals(NamingCodes.NAME_NOT_FOUND, e.code)
        assertNull(e.blocked)
        assertTrue("不应发出绑定", shadowOf(app).boundServiceConnections.isEmpty())
    }

    @Test
    fun unexportedOrPermissionGuardedIsPermissionDenied() {
        install(exported = false)
        assertEquals(NamingCodes.BIND_PERMISSION_DENIED, dialFailure().code)
        install(exported = true, permission = "dev.example.shop.permission.BIND")
        val e = dialFailure()
        assertEquals(NamingCodes.BIND_PERMISSION_DENIED, e.code)
        assertTrue(e.message, e.message!!.contains("dev.example.shop.permission.BIND"))
        assertTrue("不应发出绑定", shadowOf(app).boundServiceConnections.isEmpty())
    }

    @Test
    fun bindReturningFalseIsBlockedWithUserFacingMessage() {
        install()
        shadowOf(app).declareComponentUnbindable(component)
        val e = dialFailure()
        assertEquals(NamingCodes.ACTIVATION_BLOCKED, e.code)
        assertEquals(BlockedTarget(pkg, "小店"), e.blocked)
        assertEquals("系统阻止了本应用启动『小店』。请在系统设置中允许『小店』自启动 / 关联启动后重试。", e.message)
        assertFalse("面向用户的提示不含内部组件名", e.message!!.contains("ComponentInfo") || e.message!!.contains("ToolsService"))
        // @why 字符串形式只带错误码，不带（release 构建中被混淆的）类名。
        assertEquals("ACTIVATION_BLOCKED：${e.message}", e.toString())
    }

    @Test
    fun securityExceptionFromBindIsBlockedAndFallsBackToPackageName() {
        install(label = null)
        var unbound = 0
        val throwing = object : ContextWrapper(app) {
            override fun bindService(service: Intent, conn: ServiceConnection, flags: Int): Boolean =
                throw SecurityException("Not allowed to bind to service Intent { cmp=$component }")

            override fun unbindService(conn: ServiceConnection) {
                unbound++
            }
        }
        val e = dialFailure(ServiceChannelDialer(throwing))
        assertEquals(NamingCodes.ACTIVATION_BLOCKED, e.code)
        assertEquals(pkg, e.blocked?.packageName)
        assertEquals(pkg, e.blocked?.appLabel)
        assertTrue("SecurityException 保留为 cause", e.cause is SecurityException)
        assertEquals(1, unbound)
    }

    @Test
    fun settingsIntentOpensStandardAppDetails() {
        val i = BlockedTarget.settingsIntent(pkg)
        assertEquals(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, i.action)
        assertEquals(Uri.fromParts("package", pkg, null), i.data)
        assertTrue(i.flags and Intent.FLAG_ACTIVITY_NEW_TASK != 0)
        assertNotNull(BlockedTarget(pkg, "小店").settingsIntent().data)
    }

    @Test
    fun wireFormatRoundTripsCodeWithoutPrefixInMessage() {
        val e = ChannelOpenException(NamingCodes.HUB_UNSUPPORTED, "缺少 mcp-server")
        assertEquals("缺少 mcp-server", e.message)
        val back = ChannelOpenException.fromRemote(IllegalStateException(e.wireMessage), NamingCodes.ACTIVATION_DENIED)
        assertEquals(NamingCodes.HUB_UNSUPPORTED, back.code)
        assertEquals("缺少 mcp-server", back.message)
    }
}
