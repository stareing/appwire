package dev.appmcp.android

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Looper
import androidx.test.core.app.ApplicationProvider
import androidx.work.Configuration
import androidx.work.ListenableWorker
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.testing.SynchronousExecutor
import androidx.work.testing.TestListenableWorkerBuilder
import androidx.work.testing.WorkManagerTestInitHelper
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** WakeReceiver → WorkManager → WakeWorker → WakeTarget 路径（不加载原生库，用假 WakeTarget）。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class WakeReceiverTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    private class FakeTarget : WakeTarget {
        val immediate = CopyOnWriteArrayList<String>()
        val awaited = CopyOnWriteArrayList<String>()
        @Volatile var linkActive = false
        override fun isLinkActive() = linkActive
        override fun handleWake(args: String): Boolean = immediate.add(args)
        override suspend fun handleWakeAndAwaitSleep(args: String, timeoutMillis: Long): Boolean {
            awaited += args
            delay(10) // 模拟回连 → 调用 → 再次休眠
            return true
        }
    }

    private val target = FakeTarget()

    @Before
    fun setUp() {
        WorkManagerTestInitHelper.initializeTestWorkManager(
            context,
            Configuration.Builder().setExecutor(SynchronousExecutor()).build(),
        )
        AppMcpAndroid.wakeTarget = target
    }

    @After
    fun tearDown() {
        AppMcpAndroid.wakeTarget = null
        AppMcpAndroid.wakeTokenMaxAgeMillis = 60_000
    }

    private fun worker(token: String, receivedAt: Long? = null) =
        TestListenableWorkerBuilder<WakeWorker>(context)
            .setInputData(
                androidx.work.Data.Builder().putString(WakeWorker.KEY_TOKEN, token).apply {
                    if (receivedAt != null) putLong(WakeWorker.KEY_RECEIVED_AT, receivedAt)
                }.build(),
            )
            .build()

    private fun wakeIntent(token: String?) = Intent(AppMcpAndroid.ACTION_WAKE).apply {
        component = ComponentName(context, WakeReceiver::class.java)
        if (token != null) putExtra(AppMcpAndroid.EXTRA_TOKEN, token)
    }

    private fun uniqueWork(): List<WorkInfo> =
        WorkManager.getInstance(context).getWorkInfosForUniqueWork(WakeWorker.UNIQUE_NAME).get()

    @Test
    fun manifestDeclaresExportedReceiver() {
        val info = context.packageManager.getReceiverInfo(ComponentName(context, WakeReceiver::class.java), 0)
        assertTrue(info.exported)
    }

    @Test
    fun broadcastRunsWorkerThatHandlesWake() {
        context.sendBroadcast(wakeIntent("tok-123_abc"))
        shadowOf(Looper.getMainLooper()).idle()

        // Worker 在后台协程里完成；等 WorkInfo 进入终态。
        val deadline = System.currentTimeMillis() + 5_000
        var infos = uniqueWork()
        while (infos.any { !it.state.isFinished } && System.currentTimeMillis() < deadline) {
            Thread.sleep(20)
            shadowOf(Looper.getMainLooper()).idle()
            infos = uniqueWork()
        }
        assertEquals(1, infos.size)
        assertEquals(WorkInfo.State.SUCCEEDED, infos[0].state)
        assertTrue(infos[0].tags.contains(WakeWorker.TAG))
        assertEquals(listOf("app-mcp-wake:tok-123_abc"), target.awaited.toList())
        assertTrue(target.immediate.isEmpty())
    }

    @Test
    fun invalidTokensAreIgnored() {
        context.sendBroadcast(wakeIntent("bad token!"))
        context.sendBroadcast(wakeIntent(null))
        context.sendBroadcast(Intent("other.ACTION").setComponent(ComponentName(context, WakeReceiver::class.java)).putExtra("token", "ok"))
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue(uniqueWork().isEmpty())
        assertTrue(target.awaited.isEmpty() && target.immediate.isEmpty())
    }

    @Test
    fun requestIsExpeditedOnlyOnAndroid12Plus() {
        assertTrue(WakeReceiver.buildRequest("t", Build.VERSION_CODES.S).workSpec.expedited)
        assertFalse(WakeReceiver.buildRequest("t", Build.VERSION_CODES.R).workSpec.expedited)
        assertEquals("t", WakeReceiver.buildRequest("t").workSpec.input.getString(WakeWorker.KEY_TOKEN))
    }

    @Test
    fun workerWithoutClientFails() = runBlocking {
        AppMcpAndroid.wakeTarget = null
        val worker = TestListenableWorkerBuilder<WakeWorker>(context)
            .setInputData(androidx.work.Data.Builder().putString(WakeWorker.KEY_TOKEN, "abc").build())
            .build()
        assertEquals(ListenableWorker.Result.failure(), worker.doWork())
    }

    @Test
    fun workerDelegatesToTarget() = runBlocking {
        val worker = TestListenableWorkerBuilder<WakeWorker>(context)
            .setInputData(androidx.work.Data.Builder().putString(WakeWorker.KEY_TOKEN, "xyz").build())
            .build()
        assertEquals(ListenableWorker.Result.success(), worker.doWork())
        assertEquals(listOf("app-mcp-wake:xyz"), target.awaited.toList())
    }

    @Test
    fun wakeDescriptorTargetsReceiver() {
        val d = AppMcpAndroid.wakeDescriptor(context)
        assertEquals("${context.packageName}/dev.appmcp.android.WakeReceiver", d.target)
        assertTrue(d.background)
        val p = AppMcpAndroid.defaultLifecycle(context)
        assertEquals(dev.appmcp.LifecycleMode.IDLE, p.mode)
        assertEquals(dev.appmcp.Residency.KEEP, p.residency)
    }

    // -- 拒绝无用唤醒 ---------------------------------------------------------

    @Test
    fun broadcastWhileLinkActiveIsHandledInlineWithoutWork() {
        target.linkActive = true
        context.sendBroadcast(wakeIntent("tok-live"))
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue("连接已在时不应排后台任务", uniqueWork().isEmpty())
        assertEquals(listOf("app-mcp-wake:tok-live"), target.immediate.toList())
        assertTrue(target.awaited.isEmpty())
    }

    @Test
    fun repeatedBroadcastsWhileLinkActiveNeverEnqueue() {
        target.linkActive = true
        repeat(3) { context.sendBroadcast(wakeIntent("tok-$it")) }
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue(uniqueWork().isEmpty())
        assertEquals(3, target.immediate.size)
    }

    @Test
    fun workerSkipsAwaitWhenLinkBecameActive() = runBlocking {
        target.linkActive = true
        assertEquals(ListenableWorker.Result.success(), worker("abc").doWork())
        assertEquals(listOf("app-mcp-wake:abc"), target.immediate.toList())
        assertTrue("不应等待再次休眠", target.awaited.isEmpty())
    }

    @Test
    fun expiredWorkIsDroppedBeforeResolvingClient() = runBlocking {
        // 没有客户端也返回 success：过期检查先于取得客户端（不会冷启动客户端）。
        AppMcpAndroid.wakeTarget = null
        val old = System.currentTimeMillis() - 61_000
        assertEquals(ListenableWorker.Result.success(), worker("abc", old).doWork())
        AppMcpAndroid.wakeTarget = target
        assertEquals(ListenableWorker.Result.success(), worker("abc", old).doWork())
        assertTrue(target.immediate.isEmpty() && target.awaited.isEmpty())
    }

    @Test
    fun freshWorkIsProcessed() = runBlocking {
        assertEquals(ListenableWorker.Result.success(), worker("fresh", System.currentTimeMillis() - 1_000).doWork())
        assertEquals(listOf("app-mcp-wake:fresh"), target.awaited.toList())
    }

    @Test
    fun maxAgeCanBeDisabled() = runBlocking {
        AppMcpAndroid.wakeTokenMaxAgeMillis = 0
        assertEquals(ListenableWorker.Result.success(), worker("old", 1L).doWork())
        assertEquals(listOf("app-mcp-wake:old"), target.awaited.toList())
    }

    @Test
    fun expiryRules() {
        assertFalse("旧版本任务没有时间戳", WakeWorker.isExpired(-1, 1_000_000, 60_000))
        assertFalse(WakeWorker.isExpired(1_000, 61_000, 60_000))
        assertTrue(WakeWorker.isExpired(1_000, 61_001, 60_000))
        assertFalse("时钟回拨按未过期处理", WakeWorker.isExpired(10_000, 5_000, 60_000))
        assertFalse("<= 0 关闭检查", WakeWorker.isExpired(0, Long.MAX_VALUE, 0))
    }

    @Test
    fun requestCarriesReceivedAt() {
        val r = WakeReceiver.buildRequest("t", Build.VERSION_CODES.S, 1234L)
        assertEquals(1234L, r.workSpec.input.getLong(WakeWorker.KEY_RECEIVED_AT, -1))
        assertFalse(WakeReceiver.buildRequest("t").workSpec.input.keyValueMap.containsKey(WakeWorker.KEY_RECEIVED_AT))
    }

    @Test
    fun linkActiveStatusTable() {
        val active = setOf(
            dev.appmcp.StateStatus.CONNECTED,
            dev.appmcp.StateStatus.CONNECTING,
            dev.appmcp.StateStatus.HANDSHAKING,
            dev.appmcp.StateStatus.PENDING_PAIRING,
            dev.appmcp.StateStatus.WAKING,
        )
        for (s in dev.appmcp.StateStatus.entries) {
            assertEquals(s.name, s in active, AppMcpWakeTarget.isLinkActiveStatus(s))
        }
    }
}
