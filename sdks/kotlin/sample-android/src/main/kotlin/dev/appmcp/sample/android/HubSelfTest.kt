package dev.appmcp.sample.android

import android.util.Log
import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.hub.Hub
import dev.appmcp.hub.HubConfig
import dev.appmcp.hub.ffi.HubEvent
import dev.appmcp.hub.Risk
import dev.appmcp.hub.ToolFilter
import dev.appmcp.hub.ToolFormat
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.async
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.util.concurrent.atomic.AtomicInteger
import kotlin.time.Duration.Companion.seconds

/**
 * 进程内 Hub 自检（Hub SDK :app-mcp-hub-android + App SDK 同进程）：验证两个 uniffi 绑定在 R8 混淆后
 * 仍可用——事件监听、审批回调（Kotlin → Rust → Kotlin 的 callback interface）、async 调用、格式导出。
 *
 *   adb shell am start -n dev.appmcp.sample.android/.MainActivity --ez hubSelfTest true
 *
 * 结果写 logcat（tag AppMcpSample）：`HUB_SELFTEST OK …` 或 `HUB_SELFTEST FAIL …`。
 */
object HubSelfTest {
    suspend fun run(): Boolean = withContext(Dispatchers.Default) {
        runCatching { runInner() }
            .onSuccess { Log.i(TAG, "HUB_SELFTEST OK $it") }
            .onFailure { Log.e(TAG, "HUB_SELFTEST FAIL", it) }
            .isSuccess
    }

    private suspend fun runInner(): String = coroutineScope {
        Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false, approvalMinRisk = Risk.DESTRUCTIVE)).use { hub ->
            val approvals = AtomicInteger()
            hub.setApprovalHandler { req ->
                approvals.incrementAndGet()
                req.appId == "selftest"
            }
            val connected = async(start = CoroutineStart.UNDISPATCHED) {
                hub.events.first { it is HubEvent.AppConnected && it.appId == "selftest" }
            }
            val app = AppMcp.create(
                AppMcpConfig("selftest", "Hub 自检", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default),
            )
            app.tool("reset", "需审批的工具", risk = dev.appmcp.Risk.DESTRUCTIVE) { args, _ ->
                buildJsonObject { put("ok", true); put("echo", args["text"]?.jsonPrimitive?.content) }
            }
            try {
                app.start()
                withTimeout(10.seconds) {
                    connected.await()
                    while (hub.tools(ToolFilter(apps = listOf("selftest"))).isEmpty()) delay(20)
                }
                val r = hub.callTool("selftest.reset", buildJsonObject { put("text", "r8") }, timeout = 5.seconds)
                val data = r.getOrThrow() as JsonObject
                check(data["echo"]?.jsonPrimitive?.content == "r8") { "回显不符：$data" }
                check(approvals.get() == 1) { "审批回调次数 ${approvals.get()}" }
                val exported = hub.exportTools(ToolFormat.OPEN_AI_CHAT, ToolFilter(apps = listOf("selftest")))
                "listen=${hub.listenAddr} result=$data approvals=${approvals.get()} exported=${exported.jsonArray.size}"
            } finally {
                app.close()
            }
        }
    }
}
