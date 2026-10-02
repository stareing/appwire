package dev.appmcp.sample.android

import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import dev.appmcp.ErrorKind
import dev.appmcp.NavigationResult
import dev.appmcp.Risk
import dev.appmcp.ToolCallException
import dev.appmcp.compose.navigationRouter
import dev.appmcp.compose.rememberViewTool
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/** 页面名（清单 `pages[].name` / 工具的 `page`）→ Navigation Compose 路由。 */
private val ROUTES = mapOf(PAGE_COUNTER to PAGE_COUNTER, PAGE_NOTES to PAGE_NOTES)
private const val PAGE_COUNTER = "counter"
private const val PAGE_NOTES = "notes"

/**
 * Compose 导航示例（第 4c 项）：页面 `counter`（`counter.increment` / `counter.read`）与 `notes`（`notes.add` /
 * `notes.list`，清空确认弹窗内 `notes.clear.confirm` / `notes.clear.cancel`）的 view 工具只在所在页面处于最上层时启用；
 * Host 的 `app/navigate` 经 [SampleApp] 的导航回调交给本界面的 Navigation Compose 路由。
 * notes 页输入框有未添加的草稿时拒绝离开、清空弹窗打开时拒绝任何导航（NAVIGATION_DENIED）。Host 的清单需在 `pages` 中声明这两个页面及其工具，
 * 才能调用不在当前页面的工具（spec/manifest.md 2.3）。
 */
class NavActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val app = application as SampleApp
        setContent { SampleNav(app) }
    }
}

@Composable
private fun SampleNav(app: SampleApp) {
    val nav = rememberNavController()
    val draft = rememberSaveable { mutableStateOf("") }
    val confirming = remember { mutableStateOf(false) }
    val router = remember(nav) { nav.navigationRouter(ROUTES) }
    val currentDraft by rememberUpdatedState(draft.value)
    val dialogOpen by rememberUpdatedState(confirming.value)
    // 界面在前台（Activity RESUMED）时登记导航函数；切到后台由 SampleApp 先把界面带回前台
    LifecycleResumeEffect(router) {
        app.foregroundNavigator.value = { page, params ->
            val leaving = nav.currentDestination?.route == PAGE_NOTES && page != PAGE_NOTES
            if (dialogOpen) {
                NavigationResult.Denied("清空确认弹窗打开中，先用 notes.clear.confirm / notes.clear.cancel 关闭")
            } else if (leaving && currentDraft.isNotBlank()) {
                NavigationResult.Denied("用户正在 notes 页输入，暂不切换页面")
            } else {
                router(page, params)
            }
        }
        onPauseOrDispose { app.foregroundNavigator.value = null }
    }
    NavHost(nav, startDestination = PAGE_COUNTER, modifier = Modifier.fillMaxSize().background(Color.White)) {
        composable(PAGE_COUNTER) { CounterScreen(app, nav) }
        composable(PAGE_NOTES) { NotesScreen(app, nav, draft, confirming) }
    }
}

@Composable
private fun CounterScreen(app: SampleApp, nav: NavHostController) {
    val count by app.counter.collectAsState()
    val incSchema = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") { putJsonObject("by") { put("type", "integer"); put("minimum", 1) } }
    }
    rememberViewTool(app.mcp, "counter.increment", "计数页：计数器加 by（默认 1），返回新值", incSchema, page = PAGE_COUNTER) { args, _ ->
        val by = args["by"]?.jsonPrimitive?.intOrNull ?: 1
        if (by <= 0) throw ToolCallException(ErrorKind.INVALID_INPUT, "by 必须为正")
        app.counter.value += by
        app.counter.value
    }
    rememberViewTool(app.mcp, "counter.read", "计数页：读取当前计数", page = PAGE_COUNTER, risk = Risk.READ) { _, _ ->
        app.counter.value
    }
    Screen("计数页") {
        Label("计数：$count")
        SampleButton("+1") { app.counter.value += 1 }
        SampleButton("去笔记页 →") { nav.navigate(PAGE_NOTES) { launchSingleTop = true } }
    }
}

@Composable
private fun NotesScreen(
    app: SampleApp,
    nav: NavHostController,
    draft: MutableState<String>,
    dialog: MutableState<Boolean>,
) {
    val notes by app.notes.collectAsState()
    var confirming by dialog
    val owner = LocalLifecycleOwner.current
    val textSchema = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") { putJsonObject("text") { put("type", "string") } }
        put("required", buildJsonArray { add(JsonPrimitive("text")) })
    }
    // 弹窗不改变本页的生命周期（仍 RESUMED），弹窗打开时由 enabled 压制下层工具
    rememberViewTool(
        app.mcp, "notes.add", "笔记页：添加一条笔记，返回笔记总数", textSchema, page = PAGE_NOTES, enabled = !confirming,
    ) { args, _ ->
        val text = args["text"]?.jsonPrimitive?.contentOrNull?.takeIf { it.isNotBlank() }
            ?: throw ToolCallException(ErrorKind.INVALID_INPUT, "缺少 text")
        app.notes.value += text
        app.notes.value.size
    }
    rememberViewTool(
        app.mcp, "notes.list", "笔记页：列出全部笔记", page = PAGE_NOTES, risk = Risk.READ, enabled = !confirming,
    ) { _, _ -> app.notes.value }
    Screen("笔记页") {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            BasicTextField(
                draft.value, { draft.value = it },
                textStyle = TextStyle(fontSize = 18.sp),
                modifier = Modifier.weight(1f).border(1.dp, Color.Gray).padding(8.dp),
            )
            SampleButton("添加", Modifier) {
                if (draft.value.isNotBlank()) app.notes.value += draft.value
                draft.value = ""
            }
        }
        notes.forEachIndexed { i, n -> Label("${i + 1}. $n") }
        SampleButton("清空…") {
            Log.i(TAG, "清空弹窗打开：notes 页生命周期=${owner.lifecycle.currentState}")
            confirming = true
        }
        SampleButton("← 回计数页") { nav.popBackStack(PAGE_COUNTER, inclusive = false) }
    }
    if (confirming) {
        Dialog(onDismissRequest = { confirming = false }) {
            rememberViewTool(app.mcp, "notes.clear.confirm", "清空确认弹窗：确认清空全部笔记") { _, _ ->
                app.notes.value = emptyList()
                confirming = false
            }
            rememberViewTool(app.mcp, "notes.clear.cancel", "清空确认弹窗：取消", risk = Risk.READ) { _, _ ->
                confirming = false
            }
            Column(Modifier.background(Color.White).padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Label("清空全部 ${notes.size} 条笔记？")
                SampleButton("确认") { app.notes.value = emptyList(); confirming = false }
                SampleButton("取消") { confirming = false }
            }
        }
    }
}

@Composable
private fun Screen(title: String, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        BasicText(title, style = TextStyle(fontSize = 26.sp))
        content()
    }
}

@Composable
private fun Label(text: String) = BasicText(text, style = TextStyle(fontSize = 18.sp))

@Composable
private fun SampleButton(text: String, modifier: Modifier = Modifier.fillMaxWidth(), onClick: () -> Unit) {
    Box(modifier.background(Color(0xFFE0E7FF)).clickable(onClick = onClick).padding(12.dp)) {
        BasicText(text, style = TextStyle(fontSize = 18.sp))
    }
}
