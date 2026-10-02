package dev.appmcp.sample.agent

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.widget.ScrollView
import android.widget.TextView
import dev.appmcp.agent.HubClient
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * 最小 Agent：每次打开（或 `am start` 带 extras）执行一轮——绑定 Hub App → initialize → tools/list → tools/call → 关闭。
 * extras：`tool`（默认 `sample-android.demo.echo`）、`args`（JSON 对象，默认 `{"text":"hello from agent"}`）、
 * `holdMs`（调用后保持与 Hub App 的绑定多久再关闭，默认 0；用于观察 Hub 对目标 App 的宽限关闭，spec/naming.md 7.2）。
 */
class AgentActivity : Activity() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private lateinit var output: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        output = TextView(this).apply { setPadding(32, 32, 32, 32); setTextIsSelectable(true) }
        setContentView(ScrollView(this).apply { addView(output) })
        val tool = intent.getStringExtra(EXTRA_TOOL) ?: DEFAULT_TOOL
        val args = intent.getStringExtra(EXTRA_ARGS)
            ?.let { runCatching { Json.parseToJsonElement(it).jsonObject }.getOrNull() }
            ?: DEFAULT_ARGS
        val holdMs = intent.getLongExtra(EXTRA_HOLD_MS, 0L).coerceIn(0L, MAX_HOLD_MS)
        scope.launch { runOnce(tool, args, holdMs) }
    }

    private suspend fun runOnce(tool: String, args: JsonObject, holdMs: Long) {
        val t0 = System.nanoTime()
        try {
            HubClient.connect(this).use { hub ->
                show("已绑定 ${hub.component.flattenToShortString()}")
                val init = hub.initialize("appwire-sample-agent")
                show("initialize：${init["serverInfo"]}")
                val tools = hub.listTools().map { it["name"]?.jsonPrimitive?.content.orEmpty() }
                show("tools/list（${tools.size}）：${tools.joinToString()}")
                val result = hub.callTool(tool, args)
                show("tools/call $tool → $result")
                show("用时 ${(System.nanoTime() - t0) / 1_000_000} ms")
                if (holdMs > 0) {
                    show("保持绑定 $holdMs ms")
                    delay(holdMs)
                }
            }
            show("已解绑 Hub App")
        } catch (e: Exception) {
            show("失败：${e.javaClass.simpleName}：${e.message}")
        }
    }

    private fun show(line: String) {
        Log.i(TAG, line)
        output.append(line + "\n\n")
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private companion object {
        const val TAG = "AppMcpAgent"
        const val EXTRA_TOOL = "tool"
        const val EXTRA_ARGS = "args"
        const val EXTRA_HOLD_MS = "holdMs"
        const val MAX_HOLD_MS = 120_000L
        const val DEFAULT_TOOL = "sample-android.demo.echo"
        val DEFAULT_ARGS: JsonObject = Json.parseToJsonElement("""{"text":"hello from agent"}""").jsonObject
    }
}
