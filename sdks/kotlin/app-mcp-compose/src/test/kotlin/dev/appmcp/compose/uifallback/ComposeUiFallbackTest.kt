package dev.appmcp.compose.uifallback

import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.view.inspector.WindowInspector
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.password
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import dev.appmcp.ToolCallException
import dev.appmcp.android.uifallback.AndroidUiPlatform
import dev.appmcp.android.uifallback.UiFillValue
import dev.appmcp.android.uifallback.UiInspector
import dev.appmcp.android.uifallback.UiPlatform
import java.time.Duration
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/** Compose 的兜底（spec/ui-fallback.md 8.2 Compose 列）：真实 Compose 界面的语义树上的大纲、语义动作与密码类控件处理。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class ComposeUiFallbackTest {
    private var count by mutableStateOf(0)
    private var note by mutableStateOf("")
    private var secret by mutableStateOf("hunter2")
    private var agree by mutableStateOf(false)
    private val log = mutableListOf<String>()
    private lateinit var ui: UiInspector

    /**
     * 跑主线程任务与帧，再让 Compose 完成测量 / 布局。
     * @why Robolectric 不绘制，而 AndroidComposeView 在 dispatchDraw 中才执行待处理的布局（滚动后的位置）；
     *   compose ui-test 的 waitForIdle 同样调用 measureAndLayoutForTest。
     */
    private fun idle() {
        val looper = shadowOf(Looper.getMainLooper())
        looper.idleFor(Duration.ofMillis(100))
        WindowInspector.getGlobalWindowViews().forEach(::layoutCompose)
        looper.idle()
    }

    private fun layoutCompose(view: View) {
        if (view is ViewRootForTest) view.measureAndLayoutForTest()
        if (view is ViewGroup) (0 until view.childCount).forEach { layoutCompose(view.getChildAt(it)) }
    }

    @Before
    fun setUp() {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.title = "示例"
        activity.setContent {
            Column {
                BasicText("计数 $count", Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                BasicText("加一", Modifier.mcpDeclared("counter.increment").clickable(role = Role.Button) { count++ })
                BasicTextField(
                    note, { note = it },
                    Modifier.semantics { contentDescription = "备注" },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { log += "send:$note" }),
                )
                BasicTextField(
                    secret, { secret = it },
                    Modifier.semantics { contentDescription = "密码"; password() },
                    visualTransformation = PasswordVisualTransformation(),
                )
                Row(Modifier.toggleable(agree, role = Role.Checkbox) { agree = it }) { BasicText("同意") }
                BasicText("禁用", Modifier.clickable(enabled = false, role = Role.Button) {})
                Column(Modifier.height(100.dp).verticalScroll(rememberScrollState()).semantics { contentDescription = "列表" }) {
                    repeat(20) { i -> BasicText("项目 $i", Modifier.height(40.dp).clickable(role = Role.Button) { log += "item:$i" }) }
                }
            }
        }
        idle()
        val android = AndroidUiPlatform(listOf(ComposeUiExpander), 0)
        ui = UiInspector(object : UiPlatform by android {
            override suspend fun settle() = idle()
        })
    }

    private fun refOf(line: String): String {
        val text = ui.outline(null, null, null).text
        return text.lines().firstOrNull { line in it }?.trim()?.removePrefix("» ")?.substringBefore(' ')
            ?: throw AssertionError("大纲中没有「$line」：\n$text")
    }

    private fun details(block: suspend () -> Unit): JsonObject =
        assertThrows(ToolCallException::class.java) { runBlocking { block() } }.details as JsonObject

    @Test
    fun outlineReadsSemanticsAndMasksPasswords() {
        val text = ui.outline(null, null, null).text
        listOf(
            "» e1 窗口「示例」",
            "提示「计数 0」",
            "按钮「加一」 [已声明：counter.increment]",
            "输入框「备注」",
            "密码框「密码」= \"••••\"",
            "复选框「同意」 unchecked",
            "按钮「禁用」 disabled",
            "滚动区「列表」",
            "按钮「项目 0」",
        ).forEach { assertTrue("缺少「$it」：\n$text", text.contains(it)) }
        assertFalse(text, text.contains("hunter2"))
        assertFalse("滚出视口的项不列出：\n$text", text.contains("项目 19"))
        val read = ui.read(null, null).text
        assertTrue(read, read.contains("••••"))
        assertFalse(read, read.contains("hunter2"))
    }

    @Test
    fun semanticsActionsDriveTheUi() = runBlocking {
        val result = ui.click(refOf("按钮「加一」"))
        assertEquals(1, count)
        assertEquals(listOf("${refOf("提示「计数 1」")} 计数 0 名称变为「计数 1」"), result.changes)
        assertEquals("该元素已声明为工具 counter.increment，下次可直接调用", result.hint)

        ui.fill(refOf("输入框「备注」"), UiFillValue.Text("尽快发货"))
        assertEquals("尽快发货", note)
        assertTrue(ui.outline(null, null, null).text.contains("输入框「备注」= \"尽快发货\""))
        ui.press(refOf("输入框「备注」"), "Enter")
        assertEquals(listOf("send:尽快发货"), log)

        assertEquals(listOf("${refOf("复选框「同意」")} 同意 变为 checked"), ui.fill(refOf("复选框「同意」"), UiFillValue.Bool(true)).changes)
        assertTrue(agree)
    }

    @Test
    fun refusesPasswordsAndDisabledControls() {
        assertEquals(JsonPrimitive("secure"), details { ui.fill(refOf("密码框「密码」"), UiFillValue.Text("x")) }["reason"])
        assertEquals("hunter2", secret)
        assertEquals(JsonPrimitive("TOOL_DISABLED"), details { ui.click(refOf("按钮「禁用」")) }["reason"])
    }

    @Test
    fun scrollPagesAndBringsBackIntoView() = runBlocking {
        val item = refOf("按钮「项目 1」")
        val changes = ui.scroll(item, "down").changes
        assertTrue(changes.toString(), changes.any { it.contains("已消失") })
        assertEquals(JsonPrimitive("hidden"), details { ui.click(item) }["reason"])
        ui.scroll(item, null)
        assertTrue(ui.outline(null, null, null).text.contains("$item 按钮「项目 1」"))
        ui.click(item)
        assertEquals(listOf("item:1"), log)
    }
}
