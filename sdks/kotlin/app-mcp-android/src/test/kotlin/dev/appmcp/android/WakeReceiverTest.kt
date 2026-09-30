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
    }

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
}
