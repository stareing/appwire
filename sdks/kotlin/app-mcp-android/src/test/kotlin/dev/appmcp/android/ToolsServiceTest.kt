package dev.appmcp.android

import android.content.Intent
import android.os.ParcelFileDescriptor
import androidx.test.core.app.ApplicationProvider
import dev.appmcp.ChannelOffer
import dev.appmcp.binder.BinderCaller
import dev.appmcp.binder.ChannelOpenException
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.FdChannelClient
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import java.io.File

/** spec/naming.md 4.2：ToolsService 的 onBind / open() 交换 fd、各拒绝原因、Hub 信任判定。 */
@RunWith(RobolectricTestRunner::class)
class ToolsServiceTest {
    /** 用文件模拟 socketpair 两端（Robolectric 没有真实 socketpair）；记录创建与交出的 fd。 */
    private class FakePairs : SocketPairFactory {
        val hubEnds = mutableListOf<File>()
        override fun create(): Pair<ParcelFileDescriptor, ParcelFileDescriptor> {
            val app = File.createTempFile("app-end", ".bin").apply { deleteOnExit() }
            val hub = File.createTempFile("hub-end", ".bin").apply { writeText("hub-end-${hubEnds.size}"); deleteOnExit() }
            hubEnds += hub
            return ParcelFileDescriptor.open(app, ParcelFileDescriptor.MODE_READ_WRITE) to
                ParcelFileDescriptor.open(hub, ParcelFileDescriptor.MODE_READ_ONLY)
        }
    }

    /** 依次给出预设结果的接收方；接管（关闭）收到的 App 一端，模拟原生层取得所有权。 */
    private class ScriptedTarget(vararg outcomes: ChannelOffer?) : ChannelTarget {
        val queue = ArrayDeque(outcomes.toList())
        val received = mutableListOf<ParcelFileDescriptor>()
        var settles = 0
        override fun acceptChannel(appEnd: ParcelFileDescriptor): ChannelOffer? {
            received += appEnd
            val next = queue.removeFirst()
            if (next != null) appEnd.close()
            return next
        }

        override fun awaitSettled(timeoutMillis: Long): Boolean {
            settles++
            return true
        }
    }

    private val pairs = FakePairs()

    @Before
    fun setUp() {
        AppMcpAndroid.socketPairFactory = pairs
    }

    @After
    fun tearDown() {
        AppMcpAndroid.channelTarget = null
        AppMcpAndroid.socketPairFactory = SocketPairFactory {
            val p = ParcelFileDescriptor.createSocketPair()
            p[0] to p[1]
        }
        AppMcpAndroid.trustedHubCertificates = emptySet()
    }

    private fun bind(action: String?) =
        Robolectric.buildService(ToolsService::class.java).create().get().onBind(Intent(action))

    private fun service() = Robolectric.buildService(ToolsService::class.java).create().get()

    private fun read(fd: ParcelFileDescriptor) = ParcelFileDescriptor.AutoCloseInputStream(fd).use { it.readBytes().decodeToString() }

    @Test
    fun onBindOnlyAnswersTheToolsAction() {
        assertNull(bind("other.ACTION"))
        assertNull(bind(null))
        assertNotNull(bind(AppMcpAndroid.ACTION_TOOLS))
    }

    @Test
    fun openHandsOneEndToTheClientAndReturnsTheOther() {
        val target = ScriptedTarget(dev.appmcp.ffi.ChannelOffer.Accepted)
        AppMcpAndroid.channelTarget = target
        val binder = bind(AppMcpAndroid.ACTION_TOOLS)!!
        // 同进程调用：调用方 uid = 本 App，可信（spec/naming.md 10.2 ①）。
        val fd = FdChannelClient.open(binder, FdChannel.APP_TOOLS_DESCRIPTOR)
        assertEquals("hub-end-0", read(fd))
        assertEquals(1, target.received.size)
        assertEquals(0, target.settles)
    }

    @Test
    fun busyClientIsRetriedOnceAfterItSettles() {
        val target = ScriptedTarget(dev.appmcp.ffi.ChannelOffer.Busy("连接中"), dev.appmcp.ffi.ChannelOffer.Accepted)
        AppMcpAndroid.channelTarget = target
        val fd = service().open(BinderCaller(android.os.Process.myUid(), 1), null)
        assertEquals("hub-end-1", read(fd))
        assertEquals(1, target.settles)
        assertEquals(2, target.received.size)
    }

    @Test
    fun refusalsMapToNamingCodes() {
        fun codeOf(vararg outcomes: ChannelOffer?): String {
            AppMcpAndroid.channelTarget = ScriptedTarget(*outcomes)
            return try {
                service().open(BinderCaller(android.os.Process.myUid(), 1), null)
                fail("应被拒绝"); ""
            } catch (e: ChannelOpenException) {
                e.code
            }
        }
        assertEquals("CHANNEL_LIMIT", codeOf(dev.appmcp.ffi.ChannelOffer.Busy("a"), dev.appmcp.ffi.ChannelOffer.Busy("b")))
        assertEquals("ACTIVATION_DENIED", codeOf(dev.appmcp.ffi.ChannelOffer.Stopped("已停止")))
        assertEquals("ACTIVATION_DENIED", codeOf(dev.appmcp.ffi.ChannelOffer.Invalid("坏 fd")))
        assertEquals("ACTIVATION_DENIED", codeOf(null))
        // 没有客户端（Application 未实现 AppMcpProvider）。
        AppMcpAndroid.channelTarget = null
        val e = runCatching { service().open(BinderCaller(android.os.Process.myUid(), 1), null) }.exceptionOrNull()
        assertEquals("ACTIVATION_DENIED", (e as ChannelOpenException).code)
        // 只提供默认名字。
        AppMcpAndroid.channelTarget = ScriptedTarget(dev.appmcp.ffi.ChannelOffer.Accepted)
        val inst = runCatching { service().open(BinderCaller(android.os.Process.myUid(), 1), "w2") }.exceptionOrNull()
        assertEquals("NAME_NOT_FOUND", (inst as ChannelOpenException).code)
    }

    @Test
    fun unknownCallerIsNotTrusted() {
        val target = ScriptedTarget(dev.appmcp.ffi.ChannelOffer.Accepted)
        AppMcpAndroid.channelTarget = target
        val e = runCatching { service().open(BinderCaller(android.os.Process.myUid() + 7, 1), null) }.exceptionOrNull()
        assertEquals("HUB_NOT_TRUSTED", (e as ChannelOpenException).code)
        assertTrue("不可信时不创建通道", target.received.isEmpty() && pairs.hubEnds.isEmpty())
    }

    @Test
    fun trustRules() {
        val cert = "sha256:" + "ab".repeat(32)
        assertTrue(HubTrust.decide(10_001, 10_001, false, emptySet(), emptySet()))
        assertTrue(HubTrust.decide(10_002, 10_001, true, emptySet(), emptySet()))
        assertTrue(HubTrust.decide(10_002, 10_001, false, setOf(cert), setOf(cert)))
        assertFalse(HubTrust.decide(10_002, 10_001, false, setOf(cert), emptySet()))
        assertEquals(listOf(cert), HubTrust.parseList(" ${"AB".repeat(32)} , 不合法 "))

        val ctx = ApplicationProvider.getApplicationContext<android.content.Context>()
        AppMcpAndroid.trustedHubCertificates = setOf("AB:".repeat(31) + "AB")
        assertTrue(cert in HubTrust.trustedCertificates(ctx))
        val other = "sha256:" + "cd".repeat(32)
        assertTrue(AppMcpAndroid.confirmHub(ctx, other))
        assertTrue(other in HubTrust.trustedCertificates(ctx))
        assertTrue(AppMcpAndroid.revokeHub(ctx, other))
        assertFalse(other in HubTrust.trustedCertificates(ctx))
        assertFalse(AppMcpAndroid.confirmHub(ctx, "nope"))
    }
}
