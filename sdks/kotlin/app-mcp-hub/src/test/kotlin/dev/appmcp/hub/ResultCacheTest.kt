package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.CachePolicy
import dev.appmcp.CacheScope
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import dev.appmcp.Risk as AppRisk

/**
 * 只读结果缓存（第 16 项 O3，spec/hub-api.md 3.20）：真实 App 声明 `cache` 的只读工具与资源 → 第二次命中（`cachedAgeMs`、
 * App 只执行一次）、`cacheBypass` 照常执行、`resultCache.maxEntries = 0` 关闭、`status().cache` 计数。
 */
class ResultCacheTest {
    private class Kv(hub: Hub) : AutoCloseable {
        val gets = AtomicInteger()
        val reads = AtomicInteger()
        val app: AppMcp = AppMcp.create(AppMcpConfig("kv", "KV", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default))

        init {
            app.tool("get", "读取", risk = AppRisk.READ, cache = CachePolicy(60_000u)) { _, _ -> mapOf("n" to gets.incrementAndGet()) }
            app.resource("snapshot", "快照", cache = CachePolicy(60_000u, CacheScope.SHARED)) { mapOf("v" to reads.incrementAndGet()) }
        }

        suspend fun start(hub: Hub) {
            app.start()
            withTimeout(10.seconds) { while (hub.tools(ToolFilter(apps = listOf("kv"))).none { it.name == "kv.get" }) delay(20) }
        }

        override fun close() = app.close()
    }

    @Test
    fun cacheHitBypassAndStatus() = runBlocking {
        Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false)).use { hub ->
            Kv(hub).use { kv ->
                kv.start(hub)
                val first = hub.callTool("kv.get")
                assertNull(first.error, first.toString())
                assertNull(first.cachedAgeMs)
                val second = hub.callTool("kv.get")
                assertNotNull(second.cachedAgeMs, second.toString())
                assertEquals(first.data, second.data, "命中返回原结果")
                assertFalse(second.woke)
                assertEquals(1, kv.gets.get(), "命中不转发给 App")

                val fresh = hub.callTool("kv.get", cacheBypass = true)
                assertNull(fresh.cachedAgeMs)
                assertEquals(2, kv.gets.get(), "绕过照常调用")
                assertEquals(fresh.data, hub.callTool("kv.get").data, "绕过的新结果覆盖缓存")

                repeat(2) { hub.readResource("app-mcp://kv/snapshot") }
                assertEquals(1, kv.reads.get(), "声明了 cache 的资源第二次读取命中")

                val cache = assertNotNull(hub.status().cache)
                assertEquals(Triple(2uL, 3uL, 2uL), Triple(cache.entries, cache.hits, cache.misses), cache.toString())
                assertTrue(cache.bytes > 0u, cache.toString())
            }
        }
    }

    @Test
    fun zeroMaxEntriesDisablesCache() = runBlocking {
        Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false, resultCache = CacheLimitOverrides(maxEntries = 0u))).use { hub ->
            Kv(hub).use { kv ->
                kv.start(hub)
                repeat(2) { assertNull(hub.callTool("kv.get").cachedAgeMs) }
                assertEquals(2, kv.gets.get())
                val cache = hub.status().cache
                assertTrue(cache == null || (cache.entries == 0uL && cache.hits == 0uL), cache.toString())
            }
        }
    }
}
