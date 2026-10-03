package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
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
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/** 调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」）：HubStatus.calls、内置工具 apps.calls / apps.cancel。 */
class CallObjectsTest {
    /** 慢调用进行中：status().calls 含该调用（RUNNING、进度）；apps.calls 只见自己的；apps.cancel 后发起方得到 CANCELLED。 */
    @Test
    fun inflightCallVisibleAndCancellable() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false))
        val app = AppMcp.create(AppMcpConfig("jobs", "作业", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default))
        app.tool("job.run", "慢作业") { _, ctx ->
            ctx.progress(1.0, 4.0, "第一步")
            withTimeout(10.seconds) { while (!ctx.isCancelled) delay(20) }
            buildJsonObject { put("ok", true) }
        }
        try {
            val names = hub.tools().map { it.name }
            assertTrue(names.containsAll(listOf("apps.calls", "apps.cancel")), names.toString())
            app.start()
            withTimeout(10.seconds) {
                while (hub.tools(ToolFilter(apps = listOf("jobs"), includeBuiltin = false))
                        .none { it.availability == Availability.AVAILABLE }
                ) delay(20)
            }
            val slow = async { hub.callTool("jobs.job.run", callId = "slow-1", session = "s1") }
            val call: CallStatus = withTimeout(10.seconds) {
                var found: CallStatus?
                while (true) {
                    found = hub.status().calls?.firstOrNull { it.callId == "slow-1" }
                    if (found?.state == CallState.RUNNING && found.progress != null) break
                    delay(20)
                }
                found!!
            }
            assertEquals(listOf("jobs.job.run", "api:s1", "api"), listOf(call.name, call.caller, call.subject))
            assertFalse(call.instanceId.isNullOrEmpty(), call.toString())
            assertEquals(Triple(1.0, 4.0, "第一步"), Triple(call.progress, call.progressTotal, call.progressMessage))

            fun calls(out: CallResult) = assertNotNull(out.data).jsonObject["calls"]!!.jsonArray
            val own = calls(hub.callTool("apps.calls", session = "s1"))
            assertEquals(listOf("slow-1"), own.map { it.jsonObject["callId"]!!.jsonPrimitive.content })
            assertFalse("caller" in (own[0] as JsonObject))
            assertEquals(0, calls(hub.callTool("apps.calls", session = "s2")).size)
            val args = buildJsonObject { put("callId", "slow-1") }
            assertEquals("TOOL_NOT_FOUND", hub.callTool("apps.cancel", args, session = "s2").error?.kind)

            val cancelled = hub.callTool("apps.cancel", args, session = "s1")
            assertTrue(assertNotNull(cancelled.data).jsonObject["cancelled"]!!.jsonPrimitive.boolean, cancelled.toString())
            val out = withTimeout(10.seconds) { slow.await() }
            assertEquals("CANCELLED", out.error?.kind, out.toString())
            assertTrue(hub.status().calls.orEmpty().none { it.callId == "slow-1" })
        } finally {
            app.close()
            hub.close()
        }
    }
}
