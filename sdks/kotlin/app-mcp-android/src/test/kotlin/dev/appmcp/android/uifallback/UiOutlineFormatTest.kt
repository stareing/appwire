package dev.appmcp.android.uifallback

import dev.appmcp.ToolCallException
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** 大纲与变化摘要格式、参数解析（spec/ui-fallback.md 第 2、4、7 节），与 UI 框架无关。 */
class UiOutlineFormatTest {
    private fun window(ref: String) = UiEntry(UiEntryKind.CONTAINER, ref, "window", "窗口", ref, "商城")

    private fun item(
        ref: String, role: String, name: String, value: String? = null, states: List<String> = emptyList(),
        declared: String? = null, depth: Int = 1, containers: List<String> = listOf("e1"),
    ) = UiEntry(
        UiEntryKind.ITEM, ref, role, UiOutlineFormat.label(role), ref, name, value, states, declared = declared,
        depth = depth, chain = listOf(0), containers = containers,
    )

    @Test
    fun rendersLinesItemsAndLimits() {
        val entries = listOf(
            window("e1"),
            item("e2", "button", "清空", declared = "cart.clear"),
            item("e3", "button", "结算", states = listOf("disabled")),
            item("e4", "textbox", "备注", value = "尽快"),
        )
        val o = UiOutlineFormat.render(entries, null, 60)
        assertEquals("» e1 窗口「商城」\n  e2 按钮「清空」 [已声明：cart.clear]\n  e3 按钮「结算」 disabled\n  e4 输入框「备注」= \"尽快\"", o.text)
        assertEquals(3, o.total)
        assertFalse(o.toJson().containsKey("remaining"))
        val limited = UiOutlineFormat.render(entries, null, 1)
        assertEquals(2, limited.remaining)
        assertTrue(limited.text.endsWith("…另有 2 个元素未列出，可用 query 或 within 缩小范围"))
        assertEquals("（没有与「付款」匹配的可交互元素）", UiOutlineFormat.render(entries, "付款", 60).text)
        assertEquals("（没有可见的可交互元素）", UiOutlineFormat.render(emptyList(), null, 60).text)
        // query：全部词匹配，含所在分组
        assertEquals(listOf("e3"), UiOutlineFormat.render(entries, "商城 结算", 60).items.map { it.ref })
    }

    @Test
    fun diffReportsChangesAddedGroupsAndRemovals() {
        val before = listOf(window("e1"), item("e2", "checkbox", "同意", states = listOf("unchecked")), item("e3", "button", "删除"))
        val dialog = UiEntry(UiEntryKind.CONTAINER, "e9", "dialog", "对话框", "e9", "确认", depth = 1, chain = listOf(0), containers = listOf("e1"))
        val after = listOf(
            window("e1"),
            item("e2", "checkbox", "同意", states = listOf("checked", "focused")),
            dialog,
            item("e10", "button", "确定", depth = 2, containers = listOf("e1", "e9")),
            item("e11", "button", "取消", depth = 2, containers = listOf("e1", "e9")),
        )
        assertEquals(
            listOf(
                "e2 同意 变为 checked",
                "新增对话框「确认」(e9)，含 2 个可交互元素（可用 ui.outline({ within: \"e9\" }) 查看）",
                "按钮「删除」(e3) 已消失",
            ),
            UiOutlineFormat.diff(before, after, "ui"),
        )
        val many = (1..20).map { item("e${it + 1}", "button", "b$it") }
        val changes = UiOutlineFormat.diff(listOf(window("e1")), listOf(window("e1")) + many, "ui")
        assertEquals(16, changes.size)
        assertEquals("…另有 5 项变化，请调用 ui.outline 查看", changes.last())
    }

    @Test
    fun truncatesCollapsesAndMatchesRefs() {
        assertEquals("abc", UiOutlineFormat.truncate("abc", 3))
        assertEquals("ab…", UiOutlineFormat.truncate("abcd", 3))
        assertEquals("😀😀…", UiOutlineFormat.truncate("😀😀😀😀", 3))
        assertEquals("a b", UiOutlineFormat.collapse("  a \n b "))
        assertTrue(UiOutlineFormat.refPattern.matches("e12"))
        assertFalse(UiOutlineFormat.refPattern.matches("e0"))
        assertEquals(UiReadResult("root", "确定 取消", false), UiOutlineFormat.readText(listOf("确定", " 确定 ", "取消"), null, 100))
    }

    @Test
    fun parsesArguments() {
        val args = JsonObject(
            mapOf("ref" to JsonPrimitive(" e3 "), "limit" to JsonPrimitive(2.7), "query" to JsonPrimitive("a"), "value" to JsonPrimitive(true)),
        )
        assertEquals("e3", UiFallbackInput.ref(args, "ref"))
        assertEquals(2, UiFallbackInput.int(args, "limit"))
        assertEquals(UiFillValue.Bool(true), UiFallbackInput.value(args))
        assertEquals(UiFillValue.Number(3.0), UiFallbackInput.value(JsonObject(mapOf("value" to JsonPrimitive(3)))))
        assertEquals(UiFillValue.Text("3"), UiFallbackInput.value(JsonObject(mapOf("value" to JsonPrimitive("3")))))
        assertNull(UiFallbackInput.ref(JsonObject(mapOf("within" to JsonNull)), "within", required = false))
        assertEquals("3", UiFallbackInput.text(UiFillValue.Number(3.0), "x", null))
        assertEquals(UiKey.SHIFT_TAB, UiFallbackInput.key("shift+TAB"))
        assertEquals(UiKey.SPACE, UiFallbackInput.key(" "))
        assertEquals(UiScrollDirection.LEFT, UiFallbackInput.direction("left"))
        listOf<() -> Unit>(
            { UiFallbackInput.ref(JsonObject(mapOf("ref" to JsonPrimitive("12"))), "ref") },
            { UiFallbackInput.ref(JsonObject(emptyMap()), "ref") },
            { UiFallbackInput.int(JsonObject(mapOf("limit" to JsonPrimitive("2"))), "limit") },
            { UiFallbackInput.string(JsonObject(mapOf("query" to JsonPrimitive(1))), "query") },
            { UiFallbackInput.value(JsonObject(emptyMap())) },
            { UiFallbackInput.value(JsonObject(mapOf("value" to JsonNull))) },
        ).forEach { assertThrows(ToolCallException::class.java) { it() } }
    }
}
