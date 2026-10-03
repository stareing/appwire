package dev.appmcp

import dev.appmcp.ffi.AppMcpException
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** 事件（spec/protocol.md 3.5）的本地行为：未连接时丢弃、本地错误；不需要 Host。连接后的发送见一致性用例 event-emit。 */
class EventsTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-unit", "事件", hostUrl = "ws://127.0.0.1:9"))

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun emitWhileDisconnectedIsDropped() {
        client.declareEvent("order.shipped", "订单已发货", JsonObject(mapOf("type" to JsonPrimitive("object"))))
        assertFalse(client.emitEvent("order.shipped", mapOf("orderId" to "o1")))
        assertFalse(client.emitEvent("order.shipped", JsonObject(mapOf("orderId" to JsonPrimitive("o1")))))
        assertFalse(client.emitEvent("order.shipped"))
    }

    @Test
    fun localErrors() {
        client.declareEvent("order.shipped", "订单已发货")
        assertFailsWith<AppMcpException.InvalidName> { client.emitEvent("nope") }
        assertFailsWith<AppMcpException.InvalidJson> { client.emitEvent("order.shipped", 1) }
        assertFailsWith<AppMcpException.InvalidJson> { client.emitEvent("order.shipped", JsonArray(listOf(JsonPrimitive(1)))) }
        assertFailsWith<AppMcpException.InvalidJson> { client.emitEvent("order.shipped", mapOf("x" to Any())) }
        assertFailsWith<AppMcpException.InvalidJson> { client.emitEvent("order.shipped", mapOf("blob" to "x".repeat(9000))) }
        assertFailsWith<AppMcpException.InvalidName> { client.declareEvent("坏 名", "名称不合法") }
    }

    @Test
    fun removeEvent() {
        client.declareEvent("download.done", "下载完成")
        assertTrue(client.removeEvent("download.done"))
        assertFalse(client.removeEvent("download.done"))
        assertFailsWith<AppMcpException.InvalidName> { client.emitEvent("download.done") }
    }
}
