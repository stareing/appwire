package dev.appmcp.sample.agent

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import dev.appmcp.agent.HubClient
import dev.appmcp.binder.ChannelOpenException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
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
    private lateinit var settingsButton: Button

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        output = TextView(this).apply { setPadding(32, 32, 32, 32); setTextIsSelectable(true) }
        settingsButton = Button(this).apply { text = "去设置"; visibility = View.GONE }
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            // @why 按钮在输出之上：工具结果文本很长时不会被推到屏幕外
            addView(settingsButton)
            addView(output)
        }
        setContentView(ScrollView(this).apply { addView(column) })
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
                // Hub 拨号目标 App 被系统拦截：USER_ACTION_REQUIRED（os-permission），details 带被拦 App 的包名。
                osPermissionTarget(result)?.let { (message, pkg) -> offerSettings(message, pkg) }
                show("用时 ${(System.nanoTime() - t0) / 1_000_000} ms")
                if (holdMs > 0) {
                    show("保持绑定 $holdMs ms")
                    delay(holdMs)
                }
            }
            show("已解绑 Hub App")
        } catch (e: ChannelOpenException) {
            // 到 Hub App 的绑定被系统拦截（ACTIVATION_BLOCKED）：message 是面向用户的提示，blocked 给出 Hub App 的包名。
            val blocked = e.blocked
            if (blocked != null) offerSettings(e.message.orEmpty(), blocked.packageName)
            else show("失败（${e.code}）：${e.message}")
        } catch (e: Exception) {
            show("失败：${e.message ?: e.toString()}")
        }
    }

    /** 展示需要用户本人处理的提示，并给出打开该 App 系统设置的按钮（只给入口，由用户决定）。 */
    private fun offerSettings(message: String, packageName: String) {
        show(message)
        settingsButton.visibility = View.VISIBLE
        settingsButton.setOnClickListener {
            runCatching { startActivity(HubClient.settingsIntent(packageName)) }
                .onFailure { show("无法打开系统设置：${it.message}") }
        }
    }

    /** 工具结果为 USER_ACTION_REQUIRED 且 `reason` 为 `os-permission` 时，取（面向用户的消息，被拦 App 的包名）。 */
    private fun osPermissionTarget(result: JsonObject): Pair<String, String>? {
        val error = (result["structuredContent"] as? JsonObject)?.get("error") as? JsonObject ?: return null
        val details = error["details"] as? JsonObject ?: return null
        if ((error["kind"] as? JsonPrimitive)?.content != "USER_ACTION_REQUIRED") return null
        if ((details["reason"] as? JsonPrimitive)?.content != "os-permission") return null
        val pkg = (details["packageName"] as? JsonPrimitive)?.content ?: return null
        return (error["message"] as? JsonPrimitive)?.content.orEmpty() to pkg
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
