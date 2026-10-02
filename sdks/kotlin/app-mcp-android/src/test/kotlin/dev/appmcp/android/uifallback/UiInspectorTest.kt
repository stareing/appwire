package dev.appmcp.android.uifallback

import dev.appmcp.ErrorKind
import dev.appmcp.ToolCallException
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** 兜底引擎（spec/ui-fallback.md 第 3–7 节）在假控件树上的行为：引用、核对、动作映射、变化摘要。 */
class UiInspectorTest {
    private val clear = FakeElement("button", "清空", declared = "cart.clear")
    private val pay = FakeElement("button", "结算", states = mutableListOf("disabled"), enabled = false)
    private val note = FakeElement("textbox", "备注", value = "尽快")
    private val password = FakeElement("textbox", "支付密码", value = "123456", secure = true)
    private val agree = FakeElement("checkbox", "同意", states = mutableListOf("unchecked"))
    private val radio = FakeElement("radio", "余额", states = mutableListOf("checked"))
    private val window = FakeWindow("商城").apply {
        children += FakeElement(null, declared = "cart.pay").apply { children += FakeElement("button", "支付") }
        children += clear
        children += pay
        children += FakeElement("heading", "支付方式", UiEntryKind.HEADING)
        children += note
        children += password
        children += agree
        children += radio
    }
    private val platform = FakePlatform(window)
    private val ui = UiInspector(platform)

    private fun invalidError(block: suspend () -> Unit): ToolCallException {
        val e = assertThrows(ToolCallException::class.java) { runBlocking { block() } }
        assertEquals(ErrorKind.INVALID_INPUT, e.kind)
        return e
    }

    private fun invalid(block: suspend () -> Unit): JsonObject = (invalidError(block).details as? JsonObject) ?: JsonObject(emptyMap())

    @Test
    fun outlineListsItemsGroupsHeadingsAndMasksSecureValues() {
        val o = ui.outline(null, null, null)
        assertEquals(
            """
            » e1 窗口「商城」
              e2 按钮「支付」 [已声明：cart.pay]
              e3 按钮「清空」 [已声明：cart.clear]
              e4 按钮「结算」 disabled
              ## 支付方式
              e5 输入框「备注」= "尽快"
              e6 密码框「支付密码」= "••••"
              e7 复选框「同意」 unchecked
              e8 单选框「余额」 checked
            """.trimIndent(),
            o.text,
        )
        val json = o.toJson()
        assertEquals("窗口「商城」 › 支付方式", json["items"]!!.let { (it as kotlinx.serialization.json.JsonArray)[3].jsonObject["group"]!!.jsonPrimitive.content })
        assertTrue(json["hint"]!!.jsonPrimitive.content.contains("已声明"))
        // 引用在同一实例内稳定
        assertEquals(o.text, ui.outline(null, null, null).text)
        assertEquals("e3 按钮「清空」 [已声明：cart.clear]", ui.outline("清空", null, null).text.lines()[1].trim())
    }

    @Test
    fun clickReportsChangesAndDeclaredHint() = runBlocking {
        ui.outline(null, null, null)
        clear.onClick = {
            note.value = null
            window.children += FakeWindow("确认", "dialog").apply { children += FakeElement("button", "确定"); children += FakeElement("button", "取消") }
        }
        val r = ui.click("e3")
        assertEquals(listOf("e5 备注 值已清空", "新增对话框「确认」(e9)，含 2 个可交互元素（可用 ui.outline({ within: \"e9\" }) 查看）"), r.changes)
        assertEquals("该元素已声明为工具 cart.clear，下次可直接调用", r.hint)
        assertEquals(1, platform.settles)
        assertEquals("» e9 对话框「确认」\n  e10 按钮「确定」\n  e11 按钮「取消」", ui.outline(null, "e9", null).text)
    }

    @Test
    fun preflightChecksStaleHiddenDisabledAndSecure() {
        ui.outline(null, null, null)
        assertEquals(JsonPrimitive("TOOL_DISABLED"), invalid { ui.click("e4") }["reason"])
        assertEquals(JsonPrimitive("secure"), invalid { ui.fill("e6", UiFillValue.Text("x")) }["reason"])
        assertEquals(JsonPrimitive("secure"), invalid { ui.press("e6", "Enter") }["reason"])
        assertTrue(password.log.isEmpty())
        clear.hidden = true
        assertEquals(JsonPrimitive("hidden"), invalid { ui.click("e3") }["reason"])
        window.children.remove(clear)
        val stale = invalid { ui.click("e3") }
        assertEquals(JsonPrimitive("e3"), stale["ref"])
        assertNull(stale["reason"])
        // 未分配过的引用同样按失效处理
        assertEquals(JsonPrimitive("e99"), invalid { ui.click("e99") }["ref"])
        assertTrue(clear.log.isEmpty())
    }

    @Test
    fun fillMapsValuesByRole() = runBlocking {
        ui.outline(null, null, null)
        assertEquals(listOf("e5 备注 值变为 \"尽快发货\""), ui.fill("e5", UiFillValue.Text("尽快发货")).changes)
        assertEquals("e5 输入框「备注」需要文本值", invalidError { ui.fill("e5", UiFillValue.Bool(true)) }.message)
        agree.onClick = { agree.states = mutableListOf("checked") }
        assertEquals(listOf("e7 同意 变为 checked"), ui.fill("e7", UiFillValue.Bool(true)).changes)
        // 已是目标状态：不点击
        ui.fill("e7", UiFillValue.Bool(true))
        assertEquals(listOf("click"), agree.log)
        assertEquals(JsonPrimitive("unsupported"), invalid { ui.fill("e8", UiFillValue.Bool(false)) }["reason"])
        assertEquals(JsonPrimitive("unsupported"), invalid { ui.fill("e3", UiFillValue.Text("x")) }["reason"])
        note.states = mutableListOf("readonly")
        assertEquals(JsonPrimitive("unsupported"), invalid { ui.fill("e5", UiFillValue.Text("y")) }["reason"])
    }

    @Test
    fun pressRoutesKeys() = runBlocking {
        ui.outline(null, null, null)
        assertThrows(ToolCallException::class.java) { runBlocking { ui.press(null, "F5") } }
        ui.press("e5", "Enter")
        assertEquals(listOf("focus", "ime"), note.log)
        ui.press(null, "Tab")
        ui.press(null, "Shift+Tab")
        assertEquals(listOf(UiKey.TAB, UiKey.SHIFT_TAB), window.keys)
        // 窗口未处理 Space：激活目标控件
        ui.press("e3", "Space")
        assertEquals(listOf("focus", "click"), clear.log)
        // 主窗口不处理 Escape：提示没有被处理
        assertEquals("按键 Escape 没有被任何控件处理", ui.press(null, "Escape").hint)
        // 焦点在密码框上时不按键
        password.states = mutableListOf("focused")
        ui.outline(null, null, null)
        assertEquals(JsonPrimitive("secure"), invalid { ui.press(null, "Enter") }["reason"])
    }

    @Test
    fun scrollBringsIntoViewOrPagesTheScroller() = runBlocking {
        ui.outline(null, null, null)
        assertEquals(emptyList<String>(), ui.scroll("e3", null).changes)
        assertTrue(clear.log.isEmpty())
        clear.hidden = true
        assertEquals(listOf("新增按钮「清空」(e3)"), ui.scroll("e3", null).changes)
        assertEquals("已到尽头或不可滚动", ui.scroll("e3", "down").hint)
        clear.scrollable = true
        assertNull(ui.scroll("e3", "up").hint)
        assertEquals(listOf("scrollIntoView", "scroll:down", "scroll:up"), clear.log)
        assertEquals("direction 应为 up / down / left / right", invalidError { ui.scroll("e3", "sideways") }.message)
    }

    @Test
    fun readMasksSecureValuesAndTruncates() {
        val all = ui.read(null, null)
        assertTrue(all.text.contains("支付密码 ••••"))
        assertFalse(all.text.contains("123456"))
        ui.outline(null, null, null)
        assertEquals(UiReadResult("e5", "备注 尽快", false), ui.read("e5", null))
        assertEquals(UiReadResult("root", "商城 支…", true), ui.read(null, 5))
    }

    @Test
    fun refsSurviveRecreationByUniqueFingerprintButNotReuse() {
        ui.outline(null, null, null)
        // 框架重建了控件对象（同角色、名称、分组）：沿用原引用
        clear.id = Any()
        assertTrue(ui.outline(null, null, null).text.contains("e3 按钮「清空」"))
        // 普通控件改名称：引用不变
        clear.name = "删除"
        assertTrue(ui.outline(null, null, null).text.contains("e3 按钮「删除」"))
        // 列表复用的控件对象显示了另一项：旧引用作废，分配新引用
        clear.reusable = true
        clear.name = "清空全部"
        val text = ui.outline(null, null, null).text
        assertTrue(text, text.contains("e9 按钮「清空全部」"))
        assertFalse(text.contains("e3 "))
    }

    @Test
    fun clearDropsAllRefs() {
        ui.outline(null, null, null)
        ui.clear()
        assertEquals(JsonPrimitive("e3"), invalid { ui.click("e3") }["ref"])
        assertFalse(ui.outline(null, null, null).text.contains("e3 "))
    }
}
