package dev.appmcp.hubapp

import android.content.Intent
import android.os.ParcelFileDescriptor
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.FdChannelClient
import kotlinx.coroutines.CompletableDeferred
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import java.io.File
import java.util.concurrent.TimeUnit

/** TASKS 4g d：Agent 绑定 → open() 返回 fd、另一端交给 Hub 的 MCP 出口；Hub 惰性启动、解绑销毁时关闭。 */
@RunWith(RobolectricTestRunner::class)
class HubServiceTest {
    private class FakeBackend : McpBackend {
        val served = mutableListOf<ParcelFileDescriptor>()
        val firstServed = CompletableDeferred<Unit>()
        var closed = false
        override suspend fun serve(end: ParcelFileDescriptor) {
            synchronized(served) { served += end }
            firstServed.complete(Unit)
            end.close()
        }

        override fun close() {
            closed = true
        }
    }

    private val backends = mutableListOf<FakeBackend>()

    @Before
    fun setUp() {
        HubService.backendFactory = { FakeBackend().also { backends += it } }
        HubService.channelPairs = ChannelPairs {
            val hub = File.createTempFile("hub-end", ".bin").apply { deleteOnExit() }
            val agent = File.createTempFile("agent-end", ".bin").apply { writeText("agent-end"); deleteOnExit() }
            ParcelFileDescriptor.open(hub, ParcelFileDescriptor.MODE_READ_WRITE) to
                ParcelFileDescriptor.open(agent, ParcelFileDescriptor.MODE_READ_ONLY)
        }
    }

    @After
    fun tearDown() {
        HubService.backendFactory = ::EmbeddedHub
        HubService.channelPairs = ChannelPairs {
            val p = ParcelFileDescriptor.createSocketPair()
            p[0] to p[1]
        }
    }

    @Test
    fun onlyTheHubActionIsAnswered() {
        val controller = Robolectric.buildService(HubService::class.java).create()
        assertNull(controller.get().onBind(Intent("other")))
        assertNotNull(controller.get().onBind(Intent(HubService.ACTION_HUB)))
        assertTrue("onBind 不启动 Hub（惰性）", backends.isEmpty())
    }

    @Test
    fun openServesMcpOnOneEndAndReturnsTheOther() {
        val controller = Robolectric.buildService(HubService::class.java).create()
        val binder = controller.get().onBind(Intent(HubService.ACTION_HUB))!!
        val fd = FdChannelClient.open(binder, FdChannel.HUB_DESCRIPTOR)
        ParcelFileDescriptor.AutoCloseInputStream(fd).use { assertEquals("agent-end", it.readBytes().decodeToString()) }
        FdChannelClient.open(binder, FdChannel.HUB_DESCRIPTOR).close()
        assertEquals("同一进程内只启动一个 Hub", 1, backends.size)
        val backend = backends.single()
        kotlinx.coroutines.runBlocking { kotlinx.coroutines.withTimeout(TimeUnit.SECONDS.toMillis(5)) { backend.firstServed.await() } }
        assertEquals(android.os.Process.myUid(), AgentLog.recent().last().uid)

        // 最后一个 Agent 解绑 → 系统销毁 Service → Hub 关闭。
        controller.destroy()
        assertTrue(backend.closed)
    }
}
