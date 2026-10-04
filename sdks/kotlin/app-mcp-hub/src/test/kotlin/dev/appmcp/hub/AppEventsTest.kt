package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.CoroutineStart
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/**
 * App 事件、订阅与信箱（第 16 项 N3，spec/hub-api.md 3.17）：真实 App 端 emitEvent → Agent 会话 apps.events.subscribe /
 * apps.events 取件、status().events、HubEvent.AppEvent 与 setEventHandler。
 */
class AppEventsTest {
    @Test
    fun emittedEventReachesInboxStatusStreamAndHandler() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false))
        val received = CopyOnWriteArrayList<AppEvent>()
        hub.setEventHandler { error("回调崩溃") } // 异常被忽略
        hub.setEventHandler { received += it }
        val app = AppMcp.create(AppMcpConfig("shop", "商城", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default))
        app.declareEvent("order.shipped", "订单已发货")
        try {
            app.start()
            // 订阅未知 appId 为 TOOL_NOT_FOUND：先等 App 连上。
            withTimeout(10.seconds) { while (hub.apps().none { it.appId == "shop" && it.connected }) delay(20) }
            val sub = hub.callTool("apps.events.subscribe", buildJsonObject { put("appId", "shop"); put("event", "order.shipped") }, session = "s1")
            val subId = assertNotNull(sub.data, sub.toString()).jsonObject["subscriptionId"]!!.jsonPrimitive.content

            suspend fun emitAndAwait(payload: Any?): AppEvent {
                val next = async(start = CoroutineStart.UNDISPATCHED) {
                    hub.events.first { it is dev.appmcp.hub.ffi.HubEvent.AppEvent } as dev.appmcp.hub.ffi.HubEvent.AppEvent
                }
                withTimeout(10.seconds) { while (!app.emitEvent("order.shipped", payload)) delay(20) } // 握手完成前丢弃
                return withTimeout(10.seconds) { next.await() }.event
            }

            val event = emitAndAwait(mapOf("orderId" to "o1"))
            assertEquals(listOf("shop", "order.shipped"), listOf(event.appId, event.name))
            assertEquals(buildJsonObject { put("orderId", "o1") }, Json.parseToJsonElement(event.payloadJson!!))
            assertTrue(event.id.startsWith("ev-") && event.atMs > 0uL, event.toString())
            assertEquals(listOf(event), received.toList(), "回调收到同一事件")

            val status = assertNotNull(hub.status().events)
            assertEquals(0uL, status.droppedInvalid)
            val s = status.subscriptions.single()
            assertEquals(listOf(subId, "shop", "order.shipped"), listOf(s.subscriptionId, s.appId, s.event))
            assertEquals(Triple(1uL, 0uL, 1uL), Triple(s.delivered, s.dropped, s.pending))

            val inbox = assertNotNull(hub.callTool("apps.events", session = "s1").data).jsonObject
            val fetched = inbox["events"]!!.jsonArray.map { it as JsonObject }
            assertEquals(listOf("order.shipped"), fetched.map { it["name"]!!.jsonPrimitive.content })
            assertEquals(buildJsonObject { put("orderId", "o1") }, fetched[0]["payload"])
            assertEquals(0, inbox["pending"]!!.jsonPrimitive.int)

            // 清除回调后事件流照常，回调不再收到。
            hub.setEventHandler(null)
            assertNull(emitAndAwait(null).payloadJson)
            assertEquals(1, received.size)
        } finally {
            app.close()
            hub.close()
        }
    }

    /** HubConfig.eventLimits 经绑定生效：maxSubscriptions = 1 时第二个订阅 → RATE_LIMITED（scope events）。 */
    @Test
    fun eventLimitsMaxSubscriptionsApplies() = runBlocking {
        assertNull(HubConfig().eventLimits)
        val shop = """{"manifestVersion":1,"appId":"shop","name":"商城","tools":[]}"""
        val config = HubConfig(
            enableListen = false,
            enableIpc = false,
            manifestsJson = listOf(shop),
            eventLimits = EventLimitOverrides(maxSubscriptions = 1u),
        )
        Hub.start(config).use { hub ->
            fun args(event: String) = buildJsonObject { put("appId", "shop"); put("event", event) }
            val first = hub.callTool("apps.events.subscribe", args("a"), session = "s1", timeout = 5.seconds)
            assertNull(first.error, first.toString())
            val second = hub.callTool("apps.events.subscribe", args("b"), session = "s1", timeout = 5.seconds)
            assertEquals("RATE_LIMITED", second.error?.kind, second.toString())
            val details = Json.parseToJsonElement(assertNotNull(second.error?.detailsJson)).jsonObject
            assertEquals("events", details["scope"]?.jsonPrimitive?.content)
        }
    }
}
