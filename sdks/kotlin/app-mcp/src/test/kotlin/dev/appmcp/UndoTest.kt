package dev.appmcp

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** 撤销（spec/protocol.md 3.8）：`undoable` 缺省、透传、补丁型 update 与启停不丢；[UndoAction] 到 FFI 记录的转换。不需要 Host。 */
class UndoTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-undo", "撤销", hostUrl = "ws://127.0.0.1:9"))

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun undoableReachesSpec() {
        assertFalse(client.tool("u.plain", "缺省") { _, _ -> 1 }.specForTest().undoable)
        assertTrue(client.tool("u.t", "可撤销", undoable = true) { _, _ -> 1 }.specForTest().undoable)
        val typed = client.typedTool<Unit, Int>("u.typed", "带类型", undoable = true) { _, _ -> 1 }
        assertTrue(typed.specForTest().undoable)
    }

    @Test
    fun updateKeepsAndClearsUndoable() {
        val handle = client.tool("u.t", "旧", undoable = true) { _, _ -> 1 }
        handle.update(description = "新描述")
        handle.setEnabled(false)
        handle.setEnabled(true)
        assertTrue(handle.specForTest().undoable, "补丁型 update / setEnabled 不得丢失 undoable")
        val with = client.toolsHash
        handle.update { undoable = false }
        assertFalse(handle.specForTest().undoable)
        assertNotEquals(with, client.toolsHash, "清除后的声明同步到原生层（进 toolsHash）")
        handle.update(undoable = true)
        assertEquals(with, client.toolsHash)
    }

    @Test
    fun undoActionToFfi() {
        val full = UndoAction("todo.remove", buildJsonObject { put("id", 3) }, label = "删除刚添加的待办").toFfi()
        assertEquals(dev.appmcp.ffi.UndoAction("todo.remove", """{"id":3}""", "删除刚添加的待办"), full)
        val min = UndoAction("t").toFfi()
        assertEquals("t", min.tool)
        assertNull(min.argumentsJson, "缺省参数 = {}（由原生层补）")
        assertNull(min.label)
        // 非对象参数不在封装层拒绝：交给核心去掉并记警告。
        assertEquals("[1]", UndoAction("t", JsonArray(listOf(JsonPrimitive(1)))).toFfi().argumentsJson)
    }
}
