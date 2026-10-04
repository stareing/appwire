package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.ToolResult
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import dev.appmcp.UndoAction as AppUndoAction

/**
 * 撤销（第 15 项 X2，spec/hub-api.md 3.23）：真实 App 声明 `undoable` 并在结果中给出 `undo` → [HubTool.undoable]、
 * [CallResult.undo]；`apps.undo` 的结果带 [CallResult.undoOf]；`status().undo` 有值；不合法的 `undo` 被去掉、调用照常成功。
 */
class UndoTest {
    private suspend fun Hub.awaitTool(name: String): HubTool = withTimeout(10.seconds) {
        var found: HubTool? = null
        while (found == null) {
            found = tools().firstOrNull { it.name == name }
            if (found == null) delay(20)
        }
        found
    }

    @Test
    fun undoOfferUndoOfAndStatus() = runBlocking {
        Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false)).use { hub ->
            val cfg = AppMcpConfig("tg", "Toggle", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default)
            AppMcp.create(cfg).use { app ->
                app.tool("toggle", "开关", undoable = true) { args, _ ->
                    val on = args["on"]?.jsonPrimitive?.boolean ?: false
                    val undo = AppUndoAction("toggle", buildJsonObject { put("on", !on) }, if (on) "关掉开关" else "打开开关")
                    ToolResult(buildJsonObject { put("on", on) }, undo = undo)
                }
                app.tool("broken", "撤销信息不合法") { _, _ ->
                    ToolResult(buildJsonObject { put("ok", true) }, undo = AppUndoAction("bad name!"))
                }
                app.start()

                assertTrue(hub.awaitTool("tg.toggle").undoable)
                assertFalse(hub.awaitTool("tg.broken").undoable)
                assertFalse(hub.awaitTool("apps.list").undoable, "内置工具为 false")

                val first = hub.callTool("tg.toggle", buildJsonObject { put("on", true) })
                assertNull(first.error, first.toString())
                val offer = assertNotNull(first.undo, "已登记撤销")
                assertEquals("关掉开关", offer.label)
                assertTrue(offer.expiresInMs in 1uL..1_800_000uL, offer.toString())
                assertNull(first.undoOf)
                assertEquals(UndoStatus(ttlMs = 1_800_000uL, maxPerTask = 32uL, records = 1uL), hub.status().undo)

                val undone = hub.callTool("apps.undo", buildJsonObject {})
                assertNull(undone.error, undone.toString())
                assertEquals(first.callId, undone.undoOf)
                assertEquals(JsonPrimitive(false), undone.data?.jsonObject?.get("on"), "逆调用以相反值执行")
                assertEquals("打开开关", undone.undo?.label, "逆调用结果再次登记（重做）")
                val again = hub.callTool("apps.undo", buildJsonObject { put("callId", first.callId) })
                assertEquals("TOOL_NOT_FOUND", again.error?.kind, "只能撤销一次")

                val bad = hub.callTool("tg.broken")
                assertNull(bad.error, bad.toString())
                assertNull(bad.undo, "不合法的 undo 被核心去掉")
                assertEquals(JsonPrimitive(true), bad.data?.jsonObject?.get("ok"))
            }
        }
    }
}
