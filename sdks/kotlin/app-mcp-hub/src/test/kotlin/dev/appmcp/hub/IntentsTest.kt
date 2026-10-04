package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.hub.ffi.HubException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/**
 * 标准意图（第 16 项 N4，spec/intents.md 第 4 节）：真实 App 以 `implements` 声明 → `HubTool.implements`、Agent 会话
 * `apps.intents`、[Hub.setIntentDefaults] / [Hub.intents] 与 `status().intents`。
 */
class IntentsTest {
    private val schema = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") {
            putJsonObject("to") { put("type", "array") }
            putJsonObject("text") { put("type", "string") }
        }
    }

    /** Agent 会话中 `apps.intents {intent: "message.send"}` 的实现者（工具全名 to 是否默认）。 */
    private suspend fun implementations(hub: Hub): List<Pair<String, Boolean>> {
        val out = hub.callTool("apps.intents", buildJsonObject { put("intent", "message.send") }, session = "agent")
        assertNull(out.error, out.toString())
        val entry = assertNotNull(out.data).jsonObject["intents"]!!.jsonArray
            .map { it as JsonObject }
            .single { it["intent"]!!.jsonPrimitive.content == "message.send@1" }
        assertTrue(entry["known"]!!.jsonPrimitive.boolean, entry.toString())
        return entry["implementations"]!!.jsonArray.map {
            val o = it.jsonObject
            o["tool"]!!.jsonPrimitive.content to (o["default"]?.jsonPrimitive?.boolean ?: false)
        }
    }

    @Test
    fun implementsListedAndDefaultsReorder() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false))
        val app = AppMcp.create(AppMcpConfig("mail", "邮件", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default))
        for (name in listOf("a_send", "b_send")) {
            app.tool(name, "发邮件", schema, implements = listOf("message.send@1")) { _, _ -> null }
        }
        try {
            app.start()
            val filter = ToolFilter(apps = listOf("mail"))
            // 连接之后工具列表才送达：等到出现。
            withTimeout(10.seconds) { while (hub.tools(filter).none { it.name == "mail.b_send" }) delay(20) }
            assertEquals(listOf("message.send@1"), hub.tools(filter).single { it.name == "mail.b_send" }.implements, "HubTool.implements 透传")
            assertEquals(IntentsStatus(emptyMap(), null), hub.intents())
            assertEquals(listOf("mail.a_send" to false, "mail.b_send" to false), implementations(hub), "无默认时按全名排序")

            hub.setIntentDefaults(mapOf("message.send" to "mail.b_send"))
            assertEquals(listOf("mail.b_send" to true, "mail.a_send" to false), implementations(hub), "默认排首位")
            val st = hub.intents()
            assertEquals(mapOf("message.send" to "mail.b_send"), st.defaults)
            assertNull(st.lastError)
            assertEquals(st, hub.status().intents)

            val err = assertFailsWith<HubException.Tool> { hub.setIntentDefaults(mapOf("Bad Verb" to "mail.a_send")) }
            assertEquals("INVALID_INPUT", err.kind)
            val after = hub.intents()
            assertEquals(mapOf("message.send" to "mail.b_send"), after.defaults, "旧值保留")
            assertTrue(after.lastError?.contains("Bad Verb") == true, after.toString())

            hub.setIntentDefaults(emptyMap())
            assertEquals(IntentsStatus(emptyMap(), null), hub.intents())
        } finally {
            app.close()
            hub.close()
        }
    }

    /** `HubConfig.intentDefaults` 经绑定生效；不合法时 Hub 照常启动、空表并记下原因。 */
    @Test
    fun configIntentDefaults() {
        assertNull(HubConfig().intentDefaults)
        Hub.start(HubConfig(enableListen = false, enableIpc = false, intentDefaults = mapOf("link.open@1" to "web.open"))).use { hub ->
            assertEquals(mapOf("link.open@1" to "web.open"), hub.intents().defaults)
        }
        Hub.start(HubConfig(enableListen = false, enableIpc = false, intentDefaults = mapOf("link.open" to "nodot"))).use { hub ->
            val st = hub.intents()
            assertTrue(st.defaults.isEmpty() && st.lastError?.contains("nodot") == true, st.toString())
        }
    }
}
