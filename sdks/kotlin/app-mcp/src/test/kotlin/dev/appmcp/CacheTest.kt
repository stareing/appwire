package dev.appmcp

import dev.appmcp.ffi.AppMcpException
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertNull

/** 结果缓存声明（spec/protocol.md 3.6）：缺省、透传、更新替换 / 清除、补丁型 update 与启停不丢、越界拒绝。不需要 Host。 */
class CacheTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-cache", "缓存声明", hostUrl = "ws://127.0.0.1:9"))
    private val private5s = CachePolicy(ttlMs = 5_000u)
    private val shared1s = CachePolicy(ttlMs = 1_000u, scope = CacheScope.SHARED)

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun cacheReachesSpec() {
        assertNull(client.tool("c.plain", "缺省", risk = Risk.READ) { _, _ -> 1 }.specForTest().cache)
        assertEquals(private5s, client.tool("c.p", "私有", risk = Risk.READ, cache = private5s) { _, _ -> 1 }.specForTest().cache)
        val typed = client.typedTool<Unit, Int>("c.typed", "带类型", risk = Risk.READ, cache = shared1s) { _, _ -> 1 }
        assertEquals(shared1s, typed.specForTest().cache)
        client.resource("r.cached", "资源", cache = shared1s) { mapOf("v" to 1) }
    }

    @Test
    fun updateKeepsReplacesAndClears() {
        val handle = client.tool("c.t", "读", risk = Risk.READ, cache = private5s) { _, _ -> 1 }
        handle.update(description = "新描述")
        handle.setEnabled(false)
        handle.setEnabled(true)
        assertEquals(private5s, handle.specForTest().cache, "补丁型 update / setEnabled 不得丢失 cache")
        val withCache = client.toolsHash
        handle.update(cache = shared1s)
        assertEquals(shared1s, handle.specForTest().cache)
        val replaced = client.toolsHash
        assertNotEquals(withCache, replaced, "替换后的声明同步到原生层（进 toolsHash）")
        handle.update { cache = null }
        assertNull(handle.specForTest().cache)
        assertNotEquals(replaced, client.toolsHash, "清除后的声明同步到原生层")
        assertNotEquals(withCache, client.toolsHash)
        handle.update { cache = private5s }
        assertEquals(withCache, client.toolsHash)
    }

    @Test
    fun outOfRangeTtlRejected() {
        for (ttl in listOf(0uL, 86_400_001uL)) {
            assertFailsWith<AppMcpException.InvalidConfig> {
                client.tool("c.bad", "越界", risk = Risk.READ, cache = CachePolicy(ttl)) { _, _ -> 1 }
            }
            assertFailsWith<AppMcpException.InvalidConfig> { client.resource("r.bad", "越界", cache = CachePolicy(ttl)) { 1 } }
        }
        val handle = client.tool("c.ok", "上限", risk = Risk.READ, cache = CachePolicy(86_400_000uL)) { _, _ -> 1 }
        assertFailsWith<AppMcpException.InvalidConfig> { handle.update(cache = CachePolicy(0uL)) }
        assertEquals(CachePolicy(86_400_000uL), handle.specForTest().cache, "更新失败时保留原声明")
    }
}
