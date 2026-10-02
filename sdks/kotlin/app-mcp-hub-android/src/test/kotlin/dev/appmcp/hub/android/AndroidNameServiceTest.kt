package dev.appmcp.hub.android

import android.app.Application
import android.content.ComponentName
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.content.pm.ServiceInfo
import android.os.Bundle
import android.os.Looper
import android.os.ParcelFileDescriptor
import androidx.test.core.app.ApplicationProvider
import dev.appmcp.binder.ChannelOpenException
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.FdChannelBinder
import dev.appmcp.hub.ffi.DialOutcome
import dev.appmcp.hub.NamedApp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import java.io.File
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.Future

/** spec/naming.md 4.2：发现过滤（导出 / 清单 / appId / 重复声明）、包变更增量、拨号与宽限后解绑。 */
@RunWith(RobolectricTestRunner::class)
class AndroidNameServiceTest {
    private lateinit var app: Application
    private lateinit var names: AndroidNameService
    private val manifests = mutableMapOf<String, String?>()

    /** 交给"Rust Hub"的 fd（Robolectric 不支持 detachFd：以下标代替）。 */
    private val handed = mutableListOf<ParcelFileDescriptor>()

    private fun manifest(appId: String) = """{"manifestVersion":1,"appId":"$appId","name":"x","tools":[]}"""

    private fun addService(pkg: String, cls: String = "dev.appmcp.android.ToolsService", exported: Boolean = true, uid: Int = 10_100) {
        val info = ServiceInfo().apply {
            packageName = pkg
            name = cls
            this.exported = exported
            metaData = Bundle().apply { putInt(AndroidNameService.META_MANIFEST, 1) }
            applicationInfo = ApplicationInfo().apply { packageName = pkg; this.uid = uid; enabled = true }
        }
        val spm = shadowOf(app.packageManager)
        spm.installPackage(android.content.pm.PackageInfo().apply { packageName = pkg; applicationInfo = info.applicationInfo })
        spm.addOrUpdateService(info)
        spm.addIntentFilterForService(ComponentName(pkg, cls), IntentFilter(AndroidNameService.ACTION_TOOLS))
    }

    private class Events : AndroidNameService.NameEventSink {
        val installed = mutableListOf<String>()
        val removed = mutableListOf<String>()
        override fun installed(app: NamedApp) { installed += app.appId }
        override fun removed(appId: String) { removed += appId }
    }

    @Before
    fun setUp() {
        app = ApplicationProvider.getApplicationContext()
        names = AndroidNameService(app)
        names.manifestReader = { info -> manifests[info.packageName] }
        names.detach = { pfd -> handed += pfd; handed.size - 1 }
    }

    @Test
    fun discoveryReadsMetadataAndFilters() {
        addService("dev.example.shop"); manifests["dev.example.shop"] = manifest("shop")
        addService("dev.example.nomanifest"); manifests["dev.example.nomanifest"] = null
        addService("dev.example.badid"); manifests["dev.example.badid"] = manifest("Bad_Id")
        addService("dev.example.hidden", exported = false); manifests["dev.example.hidden"] = manifest("hidden")
        // 同一 appId 由两个包声明：按包名排序只认第一个（spec/naming.md 10.3）。
        addService("dev.example.zz.clone"); manifests["dev.example.zz.clone"] = manifest("shop")
        val found = names.discover()
        assertEquals(listOf("shop"), found.map { it.appId })
        assertEquals("dev.example.shop/dev.appmcp.android.ToolsService", found.single().detail)
        assertEquals(manifest("shop"), found.single().manifestJson)
        assertTrue(found.single().activatable)
    }

    @Test
    fun packageEventsUpdateIncrementally() {
        addService("dev.example.shop"); manifests["dev.example.shop"] = manifest("shop")
        names.discover()
        val events = Events()
        names.attach(events)
        addService("dev.example.notes"); manifests["dev.example.notes"] = manifest("notes")
        names.onPackageChanged("dev.example.notes", removed = false)
        assertEquals(listOf("notes"), events.installed)
        names.onPackageChanged("dev.example.shop", removed = true)
        assertEquals(listOf("shop"), events.removed)
        assertEquals(DialOutcome.Failed::class, names.dial("shop", 1_000u)::class)
        names.close()
    }

    private fun <T> offMain(block: () -> T): T {
        val pool = Executors.newSingleThreadExecutor()
        try {
            val f: Future<T> = pool.submit(Callable { block() })
            val looper = shadowOf(Looper.getMainLooper())
            while (!f.isDone) {
                looper.idle()
                Thread.sleep(5)
            }
            return f.get()
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun dialBindsOpensAndReleaseUnbinds() {
        addService("dev.example.shop", uid = 10_123); manifests["dev.example.shop"] = manifest("shop")
        names.discover()
        val component = ComponentName("dev.example.shop", "dev.appmcp.android.ToolsService")
        val file = File.createTempFile("chan", ".bin").apply { writeText("app-end"); deleteOnExit() }
        val binder = FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { _, _ ->
            ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
        }
        shadowOf(app).setComponentNameAndServiceForBindService(component, binder)

        val outcome = offMain { names.dial("shop", 5_000u) }
        assertTrue("$outcome", outcome is DialOutcome.Channel)
        outcome as DialOutcome.Channel
        assertEquals(10_123u, outcome.peerUid)
        ParcelFileDescriptor.AutoCloseInputStream(handed[outcome.fd]).use {
            assertEquals("app-end", it.readBytes().decodeToString())
        }
        assertEquals(1, names.boundCount)
        assertTrue(shadowOf(app).unboundServiceConnections.isEmpty())

        // Hub 宽限后关闭通道 → release → unbindService（spec/naming.md 7.2）；重复释放无效果。
        names.release(outcome.lease)
        names.release(outcome.lease)
        assertEquals(0, names.boundCount)
        assertEquals(1, shadowOf(app).unboundServiceConnections.size)
    }

    @Test
    fun dialFailuresCarryCodesAndUnbind() {
        addService("dev.example.shop"); manifests["dev.example.shop"] = manifest("shop")
        names.discover()
        val component = ComponentName("dev.example.shop", "dev.appmcp.android.ToolsService")
        // App 拒绝（不可信的 Hub）：码原样回传，附设置指引；已解绑。
        shadowOf(app).setComponentNameAndServiceForBindService(
            component,
            FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { _, _ -> throw ChannelOpenException("HUB_NOT_TRUSTED", "不认识") },
        )
        val refused = offMain { names.dial("shop", 5_000u) }
        assertEquals("HUB_NOT_TRUSTED", (refused as DialOutcome.Failed).code)
        assertTrue(refused.message, refused.message.contains("可信"))
        assertEquals(0, names.boundCount)
        assertEquals(1, shadowOf(app).unboundServiceConnections.size)

        // 系统拒绝绑定（组件不可绑定 / OEM 拦截）→ ACTIVATION_DENIED。
        shadowOf(app).declareComponentUnbindable(component)
        val denied = offMain { names.dial("shop", 5_000u) }
        assertEquals("ACTIVATION_DENIED", (denied as DialOutcome.Failed).code)
        assertTrue(denied.message, denied.message.contains("关联启动"))

        assertEquals("NAME_NOT_FOUND", (names.dial("ghost", 1_000u) as DialOutcome.Failed).code)
    }

    @Test
    fun manifestAppIdParsing() {
        assertEquals("shop", AndroidNameService.appIdOf(manifest("shop")))
        assertEquals(null, AndroidNameService.appIdOf(manifest("Shop")))
        assertEquals(null, AndroidNameService.appIdOf("not json"))
        assertEquals(null, AndroidNameService.appIdOf("""{"name":"x"}"""))
    }
}
