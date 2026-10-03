package dev.appmcp

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** 用户正在操作（spec/protocol.md 5.3）：开关、策略与引用计数作用域；不需要 Host。拒绝 / 排队行为见一致性用例 call-busy-*。 */
class BusyTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-unit", "忙碌", hostUrl = "ws://127.0.0.1:9"))

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun switchReachesCore() {
        assertFalse(client.isBusy)
        client.setBusy(true)
        assertTrue(client.isBusy)
        client.setBusy(false)
        assertFalse(client.isBusy)
        client.setBusyPolicy(BusyPolicy.QUEUE)
        client.setBusyPolicy(BusyPolicy.REJECT)
    }

    @Test
    fun busyPolicyMapsToFfi() {
        assertEquals(null, AppMcpConfig("kotlin-unit", "忙碌").toFfi().busyPolicy, "为空时交给核心缺省（REJECT）")
        assertEquals(BusyPolicy.QUEUE, AppMcpConfig("kotlin-unit", "忙碌", busyPolicy = BusyPolicy.QUEUE).toFfi().busyPolicy)
        AppMcp.create(AppMcpConfig("kotlin-unit", "忙碌", hostUrl = "ws://127.0.0.1:9", busyPolicy = BusyPolicy.QUEUE)).close()
    }

    @Test
    fun nestedScopesAreReferenceCounted() {
        client.busy {
            assertTrue(client.isBusy)
            client.busy { assertTrue(client.isBusy) }
            assertTrue(client.isBusy, "内层结束不得提前结束外层")
        }
        assertFalse(client.isBusy)
        assertFailsWith<IllegalStateException> { client.busy { error("用户操作中出错") } }
        assertFalse(client.isBusy, "异常退出也要归还计数")
        val hold = client.beginBusy()
        hold.close()
        hold.close()
        assertTrue(hold.isReleased)
        client.busy { assertTrue(client.isBusy, "重复 close 不得多归还计数") }
        assertFalse(client.isBusy)
    }

    @Test
    fun scopeCombinesWithSwitch() {
        client.busy {
            client.setBusy(false)
            assertTrue(client.isBusy, "setBusy(false) 不结束进行中的作用域")
        }
        assertFalse(client.isBusy)
        client.setBusy(true)
        client.busy { }
        assertTrue(client.isBusy, "作用域结束不清除显式开关")
        client.setBusy(false)
        assertFalse(client.isBusy)
    }

    @Test
    fun scopesAcrossThreads() {
        val entered = CountDownLatch(2)
        val release = CountDownLatch(1)
        val workers = List(2) {
            thread { client.busy { entered.countDown(); release.await(5, TimeUnit.SECONDS) } }
        }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        assertTrue(client.isBusy)
        release.countDown()
        workers.forEach { it.join(5_000) }
        assertFalse(client.isBusy)
    }

    @Test
    fun closedClientIgnoresBusy() {
        val hold = client.beginBusy()
        client.close()
        hold.close()
        client.setBusy(true)
        client.setBusyPolicy(BusyPolicy.QUEUE)
        assertFalse(client.isBusy)
    }
}
