package dev.appmcp

import dev.appmcp.ffi.AppMcpException
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertNull

/** 工具弃用声明（spec/protocol.md 3.7）：缺省、透传、更新替换 / 清除、补丁型 update 与启停不丢、非法拒绝。不需要 Host。 */
class DeprecationTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-deprecated", "弃用声明", hostUrl = "ws://127.0.0.1:9"))
    private val full = Deprecation("改用 d.new", replacement = "d.new", until = "2027-06-30")
    private val onlyMessage = Deprecation("即将移除")

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun deprecationReachesSpec() {
        assertNull(client.tool("d.plain", "缺省") { _, _ -> 1 }.specForTest().deprecated)
        assertEquals(full, client.tool("d.full", "完整", deprecated = full) { _, _ -> 1 }.specForTest().deprecated)
        val typed = client.typedTool<Unit, Int>("d.typed", "带类型", deprecated = onlyMessage) { _, _ -> 1 }
        assertEquals(onlyMessage, typed.specForTest().deprecated)
        assertNull(onlyMessage.replacement)
        assertNull(onlyMessage.until)
    }

    @Test
    fun updateKeepsReplacesAndClears() {
        val handle = client.tool("d.t", "旧", deprecated = full) { _, _ -> 1 }
        handle.update(description = "新描述")
        handle.setEnabled(false)
        handle.setEnabled(true)
        assertEquals(full, handle.specForTest().deprecated, "补丁型 update / setEnabled 不得丢失 deprecated")
        val withFull = client.toolsHash
        handle.update(deprecated = onlyMessage)
        assertEquals(onlyMessage, handle.specForTest().deprecated)
        val replaced = client.toolsHash
        assertNotEquals(withFull, replaced, "替换后的声明同步到原生层（进 toolsHash）")
        handle.update { deprecated = null }
        assertNull(handle.specForTest().deprecated)
        assertNotEquals(replaced, client.toolsHash, "清除后的声明同步到原生层")
        assertNotEquals(withFull, client.toolsHash)
        handle.update { deprecated = full }
        assertEquals(withFull, client.toolsHash)
    }

    @Test
    fun invalidDeprecationRejected() {
        val bad = listOf(
            Deprecation(""),
            Deprecation("   "),
            Deprecation("x".repeat(501)),
            Deprecation("m", replacement = "bad name!"),
            Deprecation("m", replacement = "d.bad"),
            Deprecation("m", until = "2027-02-30"),
            Deprecation("m", until = "2027/06/30"),
        )
        for (d in bad) {
            assertFailsWith<AppMcpException.InvalidConfig>("$d") { client.tool("d.bad", "非法", deprecated = d) { _, _ -> 1 } }
        }
        val ok = Deprecation("x".repeat(500), replacement = "d.missing", until = "2028-02-29")
        val handle = client.tool("d.ok", "合法", deprecated = ok) { _, _ -> 1 }
        assertFailsWith<AppMcpException.InvalidConfig> { handle.update(deprecated = Deprecation("m", replacement = "d.ok")) }
        assertEquals(ok, handle.specForTest().deprecated, "更新失败时保留原声明")
    }
}
