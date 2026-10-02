package dev.appmcp.binder

import android.content.Context
import android.os.Build
import android.os.ParcelFileDescriptor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.io.File

@RunWith(RobolectricTestRunner::class)
class FdChannelTest {
    private fun tempFd(text: String): ParcelFileDescriptor {
        val f = File.createTempFile("fdchannel", ".txt").apply { writeText(text); deleteOnExit() }
        return ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    @Test
    fun openReturnsTheServersFdAndCallerIdentity() {
        var seen: Pair<BinderCaller, String?>? = null
        val binder = FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { caller, instance ->
            seen = caller to instance
            tempFd("hello")
        }
        val fd = FdChannelClient.open(binder, FdChannel.APP_TOOLS_DESCRIPTOR, "w2")
        ParcelFileDescriptor.AutoCloseInputStream(fd).use { assertEquals("hello", it.readBytes().decodeToString()) }
        assertEquals("w2", seen?.second)
        assertEquals(android.os.Process.myUid(), seen?.first?.uid)
    }

    @Test
    fun refusalsCarryNamingCodes() {
        val cases = listOf(
            ChannelOpenException("HUB_NOT_TRUSTED", "不在可信列表") to "HUB_NOT_TRUSTED",
            ChannelOpenException("CHANNEL_LIMIT", "已有连接") to "CHANNEL_LIMIT",
        )
        for ((thrown, code) in cases) {
            val binder = FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { _, _ -> throw thrown }
            try {
                FdChannelClient.open(binder, FdChannel.APP_TOOLS_DESCRIPTOR)
                fail("应被拒绝")
            } catch (e: ChannelOpenException) {
                assertEquals(code, e.code)
                assertTrue(e.detail, e.detail == thrown.detail)
            }
        }
        // 未分类的异常 → ACTIVATION_DENIED；SecurityException → HUB_NOT_TRUSTED。
        val crashing = FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { _, _ -> error("boom") }
        assertEquals("ACTIVATION_DENIED", runCatching { FdChannelClient.open(crashing, FdChannel.APP_TOOLS_DESCRIPTOR) }
            .exceptionOrNull().let { (it as ChannelOpenException).code })
        val denied = FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { _, _ -> throw SecurityException("不认识") }
        assertEquals("HUB_NOT_TRUSTED", runCatching { FdChannelClient.open(denied, FdChannel.APP_TOOLS_DESCRIPTOR) }
            .exceptionOrNull().let { (it as ChannelOpenException).code })
    }

    @Test
    fun wrongDescriptorIsRejected() {
        val binder = FdChannelBinder(FdChannel.HUB_DESCRIPTOR) { _, _ -> tempFd("x") }
        val r = runCatching { FdChannelClient.open(binder, FdChannel.APP_TOOLS_DESCRIPTOR) }
        assertTrue("描述符不符应失败：$r", r.isFailure)
    }

    @Test
    fun remoteMessagesWithoutCodeFallBack() {
        assertEquals("CHANNEL_LIMIT", ChannelOpenException.fromRemote(IllegalStateException("CHANNEL_LIMIT：忙"), "X").code)
        val plain = ChannelOpenException.fromRemote(IllegalStateException("没有码"), "ACTIVATION_DENIED")
        assertEquals("ACTIVATION_DENIED", plain.code)
        assertEquals("没有码", plain.detail)
    }

    @Test
    fun bindFlagsKeepTargetAtLowPriority() {
        val base = Context.BIND_AUTO_CREATE or Context.BIND_WAIVE_PRIORITY or
            Context.BIND_ALLOW_OOM_MANAGEMENT or Context.BIND_NOT_FOREGROUND
        assertEquals(base, ServiceChannelDialer.lowImpactBindFlags(Build.VERSION_CODES.P))
        assertEquals(base or Context.BIND_NOT_PERCEPTIBLE, ServiceChannelDialer.lowImpactBindFlags(Build.VERSION_CODES.Q))
        assertEquals(0, ServiceChannelDialer.lowImpactBindFlags() and Context.BIND_IMPORTANT)
    }

    @Test
    fun digestsAreNormalized() {
        val hex = "AB".repeat(32)
        assertEquals("sha256:" + "ab".repeat(32), PackageIdentity.normalizeDigest(hex.chunked(2).joinToString(":")))
        assertEquals("sha256:" + "ab".repeat(32), PackageIdentity.normalizeDigest("sha256:$hex"))
        assertNull(PackageIdentity.normalizeDigest("abc"))
        assertTrue(PackageIdentity.digest(byteArrayOf(1)).startsWith("sha256:"))
    }
}
