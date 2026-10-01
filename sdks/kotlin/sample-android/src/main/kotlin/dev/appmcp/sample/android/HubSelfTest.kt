package dev.appmcp.sample.android

import android.util.Log
import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.hub.Hub
import dev.appmcp.hub.HubConfig
import dev.appmcp.hub.InstanceState
import dev.appmcp.hub.UpstreamSpec
import dev.appmcp.hub.ffi.HubEvent
// 嵌套的错误类别（Unsupported 等）不能经 typealias 访问，直接用生成的类型。
import dev.appmcp.hub.ffi.HubException
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
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.util.concurrent.atomic.AtomicInteger
import kotlin.time.Duration.Companion.seconds

/**
 * 进程内 Hub 自检（Hub SDK :app-mcp-hub-android + App SDK 同进程）：验证两个 uniffi 绑定在 R8 混淆后
 * 仍可用——事件监听、审批回调（Kotlin → Rust → Kotlin 的 callback interface）、async 调用、格式导出、
 * `status()`（实例连接 ID、SDK 诊断上报）、`HubEvent.AppDiagnostic`（以网页 SDK 身份发 `app/diagnostic`），
 * 以及本构建未包含的能力（Android 默认 `mobile,schema-validation`：无 MCP 出口、无上游聚合）返回明确错误而不崩溃。
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
                val cid = checkStatus(hub)
                val diag = checkDiagnostic(hub)
                val unsupported = checkUnsupportedFeatures(hub)
                "listen=${hub.listenAddr} result=$data approvals=${approvals.get()} exported=${exported.jsonArray.size} " +
                    "cid=$cid diagnostic=$diag unsupported=[$unsupported]"
            } finally {
                app.close()
            }
        }
    }

    /** `status()`：selftest 实例已连接且带连接 ID。返回连接 ID。 */
    private fun checkStatus(hub: Hub): String {
        val st = hub.status()
        val inst = st.apps.firstOrNull { it.appId == "selftest" }?.instances?.firstOrNull()
            ?: error("status() 中没有 selftest 实例：${st.apps.map { it.appId }}")
        check(inst.state == InstanceState.CONNECTED) { "selftest 实例状态 ${inst.state}" }
        return inst.info.connectionId ?: error("selftest 实例没有 connectionId")
    }

    /** 以网页 SDK 身份连接并上报 `app/diagnostic` → `HubEvent.AppDiagnostic` 与 `status().reports`。返回 `code×count`。 */
    private suspend fun checkDiagnostic(hub: Hub): String = coroutineScope {
        val listenAddr = hub.listenAddr ?: error("Hub 未开 HTTP 监听")
        val event = async(start = CoroutineStart.UNDISPATCHED) {
            hub.events.first { it is HubEvent.AppDiagnostic && it.appId == DIAG_APP } as HubEvent.AppDiagnostic
        }
        withContext(Dispatchers.IO) {
            RawAppConnection.open(listenAddr, timeoutMs = 5_000).use { ws ->
                ws.send(
                    buildJsonObject {
                        put("jsonrpc", "2.0"); put("id", 1); put("method", "app/hello")
                        put("params", buildJsonObject {
                            put("appId", DIAG_APP); put("appName", "诊断自检"); put("protocolVersion", "1")
                            put("sdkVersion", "selftest"); put("clientKind", "web"); put("instanceId", "$DIAG_APP-1")
                        })
                    },
                )
                while (true) {
                    val msg = ws.receive()
                    if (msg["id"]?.jsonPrimitive?.int != 1) continue
                    val status = msg["result"]?.jsonObject?.get("status")?.jsonPrimitive?.content
                    check(status == "paired") { "hello 未配对：$msg" }
                    break
                }
                ws.send(
                    buildJsonObject {
                        put("jsonrpc", "2.0"); put("method", "app/diagnostic")
                        put("params", buildJsonObject {
                            put("code", DIAG_CODE); put("message", "自检：本地网络访问未授权"); put("count", 2)
                        })
                    },
                )
                val e = withTimeout(5.seconds) { event.await() }
                check(e.instanceId == "$DIAG_APP-1" && e.code == DIAG_CODE && e.count == 2u) { "AppDiagnostic 不符：$e" }
                val report = hub.status().reports.firstOrNull { it.appId == DIAG_APP }
                    ?: error("status().reports 中没有 $DIAG_APP 的上报")
                check(report.code == DIAG_CODE && report.count == 2u && report.connectionId.isNotEmpty()) { "上报不符：$report" }
            }
        }
        "${DIAG_CODE}x2"
    }

    /**
     * 本构建未包含的能力应在调用时抛 [HubException.Unsupported]（说明缺哪个 cargo feature，spec/hub-api.md 3.10），
 * 不能崩溃、静默忽略或混入其他错误类别。
     * 完整构建（`--android-features desktop`）下这些调用成功，记为 `present`。返回每项的结果。
     */
    private suspend fun checkUnsupportedFeatures(hub: Hub): String {
        val base = HubConfig(listen = "127.0.0.1:0", enableIpc = false)
        val mcpHttp = expectUnsupported("mcp-server") { Hub.start(base.copy(mcpHttp = true)).close() }
        val upstream = expectUnsupported("upstream") {
            Hub.start(base.copy(upstreams = listOf(UpstreamSpec("up", "true", emptyList(), emptyMap())))).close()
        }
        val serveHttp = expectUnsupported("mcp-server") { hub.serveHttp("127.0.0.1:0") }
        return "mcpHttp=$mcpHttp upstreams=$upstream serveHttp=$serveHttp"
    }

    /**
     * 成功 → `present`；[HubException.Unsupported]（按类别判断）且说明中带 feature 名 → `unsupported`；
     * 其他异常（包括 `HubException.Io` 等其他类别）原样抛出（判为失败）。
     */
    private suspend fun expectUnsupported(feature: String, block: suspend () -> Unit): String {
        try {
            block()
            return "present"
        } catch (e: HubException.Unsupported) {
            check(e.detail.contains("`$feature`")) { "缺少 $feature 时的错误未说明 feature：$e" }
            return "unsupported"
        }
    }

    private const val DIAG_APP = "selftest-diag"
    private const val DIAG_CODE = "BLOCKED_LOCAL_NETWORK_ACCESS"
}
