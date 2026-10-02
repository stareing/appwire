package dev.appmcp.android.uifallback

import android.app.Activity
import android.app.AlertDialog
import android.os.Looper
import android.text.InputType
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.Spinner
import android.widget.TextView
import dev.appmcp.ToolCallException
import java.time.Duration
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** Android View 体系的兜底（spec/ui-fallback.md 8.2 Android View 列）：真实 View 树上的大纲、动作与密码类控件处理。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ViewUiFallbackTest {
    private lateinit var activity: Activity
    private lateinit var status: TextView
    private lateinit var note: EditText
    private lateinit var password: EditText
    private lateinit var agree: CheckBox
    private lateinit var color: Spinner
    private lateinit var volume: SeekBar
    private lateinit var scroller: ScrollView
    private val log = mutableListOf<String>()
    private lateinit var ui: UiInspector

    private fun idle() = shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(100))

    @Before
    fun setUp() {
        activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        activity.title = "示例"
        status = TextView(activity).apply { text = "等待"; accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE }
        val submit = Button(activity).apply {
            text = "提交"
            setOnClickListener { status.text = "已提交" }
            declareMcpTools("form.submit")
        }
        note = EditText(activity).apply {
            hint = "备注"
            imeOptions = EditorInfo.IME_ACTION_SEND
            setOnEditorActionListener { _, action, _ -> log += "ime:$action"; true }
        }
        password = EditText(activity).apply {
            hint = "密码"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            setText("secret")
        }
        agree = CheckBox(activity).apply { text = "同意" }
        val disabled = Button(activity).apply { text = "禁用"; isEnabled = false }
        color = Spinner(activity).apply {
            contentDescription = "颜色"
            adapter = ArrayAdapter(activity, android.R.layout.simple_spinner_item, listOf("红", "绿"))
        }
        volume = SeekBar(activity).apply {
            contentDescription = "音量"
            max = 10
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(s: SeekBar, p: Int, fromUser: Boolean) { log += "progress:$p:$fromUser" }
                override fun onStartTrackingTouch(s: SeekBar) = Unit
                override fun onStopTrackingTouch(s: SeekBar) = Unit
            })
        }
        scroller = ScrollView(activity).apply {
            contentDescription = "列表"
            addView(LinearLayout(activity).apply {
                orientation = LinearLayout.VERTICAL
                repeat(20) { i -> addView(Button(activity).apply { text = "项目 $i" }, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 50)) }
            })
        }
        activity.setContentView(LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            listOf(status, submit, note, password, agree, disabled, color, volume).forEach { addView(it, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 40)) }
            addView(scroller, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 120))
        })
        idle()
        val android = AndroidUiPlatform(emptyList(), 0)
        ui = UiInspector(object : UiPlatform by android {
            override suspend fun settle() = idle()
        })
    }

    private fun refOf(line: String): String {
        val text = ui.outline(null, null, null).text
        return text.lines().firstOrNull { line in it }?.trim()?.substringBefore(' ')?.removePrefix("» ")
            ?: throw AssertionError("大纲中没有「$line」：\n$text")
    }

    private fun groupRefOf(line: String): String =
        ui.outline(null, null, null).text.lines().first { line in it }.trim().removePrefix("» ").substringBefore(' ')

    private fun reason(block: suspend () -> Unit): JsonObject {
        val e = org.junit.Assert.assertThrows(ToolCallException::class.java) { runBlocking { block() } }
        return e.details as JsonObject
    }

    @Test
    fun outlineDescribesControlsAndMasksPasswords() {
        val text = ui.outline(null, null, null).text
        listOf(
            "» e1 窗口「示例」",
            "提示「等待」",
            "按钮「提交」 [已声明：form.submit]",
            "输入框「备注」",
            "密码框「密码」= \"••••\"",
            "复选框「同意」 unchecked",
            "按钮「禁用」 disabled",
            "下拉框「颜色」= \"红\"",
            "滑块「音量」= \"0\"",
            "滚动区「列表」",
            "按钮「项目 0」",
        ).forEach { assertTrue("缺少「$it」：\n$text", text.contains(it)) }
        assertFalse(text, text.contains("secret"))
        // 滚出视口的项不列出
        assertFalse(text, text.contains("项目 19"))
        val read = ui.read(null, null).text
        assertTrue(read, read.contains("••••"))
        assertFalse(read, read.contains("secret"))
    }

    @Test
    fun clickAndFillActOnRealViews() = runBlocking {
        assertEquals(listOf("${refOf("提示「等待」")} 等待 名称变为「已提交」"), ui.click(refOf("按钮「提交」")).changes)
        assertEquals("该元素已声明为工具 form.submit，下次可直接调用", ui.click(refOf("按钮「提交」")).hint)

        val changes = ui.fill(refOf("输入框「备注」"), UiFillValue.Text("尽快发货")).changes
        assertEquals("尽快发货", note.text.toString())
        assertTrue(changes.toString(), changes.single().endsWith("值变为 \"尽快发货\""))

        ui.fill(refOf("复选框「同意」"), UiFillValue.Bool(true))
        assertTrue(agree.isChecked)

        ui.fill(refOf("下拉框「颜色」"), UiFillValue.Text("绿"))
        assertEquals(1, color.selectedItemPosition)
        val missing = org.junit.Assert.assertThrows(ToolCallException::class.java) { runBlocking { ui.fill(refOf("下拉框「颜色」"), UiFillValue.Text("蓝")) } }
        assertTrue(missing.message, missing.message.contains("可选：红、绿"))

        ui.fill(refOf("滑块「音量」"), UiFillValue.Number(7.0))
        assertEquals(7, volume.progress)
        assertTrue(log.toString(), "progress:7:true" in log)
    }

    @Test
    fun refusesPasswordsAndDisabledControls() {
        assertEquals(JsonPrimitive("secure"), reason { ui.fill(refOf("密码框「密码」"), UiFillValue.Text("x")) }["reason"])
        assertEquals(JsonPrimitive("secure"), reason { ui.press(refOf("密码框「密码」"), "Enter") }["reason"])
        assertEquals("secret", password.text.toString())
        assertEquals(JsonPrimitive("TOOL_DISABLED"), reason { ui.click(refOf("按钮「禁用」")) }["reason"])
        // 引用的控件被移除：失效
        val submit = refOf("按钮「提交」")
        activity.findViewById<ViewGroup>(android.R.id.content).getChildAt(0).let { (it as ViewGroup).removeViewAt(1) }
        idle()
        assertEquals(JsonPrimitive(submit), reason { ui.click(submit) }["ref"])
    }

    @Test
    fun pressSubmitsTextAndMovesFocus() = runBlocking {
        ui.press(refOf("输入框「备注」"), "Enter")
        assertEquals(listOf("ime:${EditorInfo.IME_ACTION_SEND}"), log)
        assertTrue(note.isFocused)
        ui.press(null, "Tab")
        assertTrue("焦点应移到密码框", password.isFocused)
    }

    @Test
    fun scrollPagesAndBringsBackIntoView() = runBlocking {
        val first = refOf("按钮「项目 1」")
        val changes = ui.scroll(first, "down").changes
        assertTrue(scroller.scrollY > 0)
        assertTrue(changes.toString(), changes.any { it.contains("已消失") })
        assertFalse(ui.outline(null, null, null).text.contains("项目 1」"))
        // 滚出视口后按引用滚回可见
        assertEquals(JsonPrimitive("hidden"), reason { ui.click(first) }["reason"])
        ui.scroll(first, null)
        assertTrue(ui.outline(null, null, null).text.contains("$first 按钮「项目 1」"))
        // 滚动区自身向上已到顶
        scroller.scrollTo(0, 0)
        idle()
        assertEquals("已到尽头或不可滚动", ui.scroll(groupRefOf("滚动区「列表」"), "up").hint)
    }

    @Test
    fun modalDialogCoversMainWindowAndEscapeCancels() = runBlocking {
        val dialog = AlertDialog.Builder(activity).setTitle("确认").setPositiveButton("确定", null).setNegativeButton("取消", null).show()
        idle()
        val text = ui.outline(null, null, null).text
        assertTrue(text, text.contains("对话框「确认」"))
        assertTrue(text, text.contains("按钮「确定」"))
        assertFalse("下层窗口被模态对话框遮挡：\n$text", text.contains("提交"))
        val result = ui.press(null, "Escape")
        assertFalse(dialog.isShowing)
        assertNotNull(result.changes.firstOrNull { it.contains("对话框「确认」") && it.contains("已消失") })
        assertTrue(ui.outline(null, null, null).text.contains("按钮「提交」"))
    }
}
