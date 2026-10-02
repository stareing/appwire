package dev.appmcp.agent

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.io.PrintWriter
import kotlin.concurrent.thread

/** 每行一条 JSON-RPC 的最小 MCP 会话：握手、分页列工具、调用、通知 / 服务端请求穿插、错误与断开。 */
class McpLineClientTest {
    /** 脚本化的服务端：对每个收到的消息调用 [handle]，写回它返回的行。 */
    private class FakeServer(handle: (JsonObject) -> List<String>) {
        val toClient = PipedOutputStream()
        val clientIn = PipedInputStream(toClient, 1 shl 16)
        val fromClient = PipedInputStream(1 shl 16)
        val clientOut = PipedOutputStream(fromClient)
        val received = mutableListOf<JsonObject>()
        val worker = thread(isDaemon = true) {
            val reader = BufferedReader(InputStreamReader(fromClient))
            val out = PrintWriter(toClient, true)
            while (true) {
                val line = runCatching { reader.readLine() }.getOrNull() ?: break
                val msg = Json.parseToJsonElement(line).jsonObject
                synchronized(received) { received += msg }
                for (reply in handle(msg)) out.println(reply)
            }
            out.close()
        }
    }

    private fun id(msg: JsonObject) = msg["id"].toString()

    @Test
    fun handshakeListAndCall() {
        val server = FakeServer { msg ->
            when (msg["method"]?.jsonPrimitive?.content) {
                "initialize" -> listOf(
                    """{"jsonrpc":"2.0","id":${id(msg)},"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"app-mcp-hub","version":"0"},"capabilities":{"tools":{}}}}""",
                )
                "tools/list" -> if (msg["params"]?.jsonObject?.get("cursor") == null) listOf(
                    // 穿插一个通知与一个服务端请求：通知交给回调，请求回复"方法不存在"。
                    """{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}""",
                    """{"jsonrpc":"2.0","id":"srv-1","method":"roots/list"}""",
                    """{"jsonrpc":"2.0","id":${id(msg)},"result":{"tools":[{"name":"a.x","inputSchema":{"type":"object"}}],"nextCursor":"p2"}}""",
                ) else listOf(
                    """{"jsonrpc":"2.0","id":${id(msg)},"result":{"tools":[{"name":"b.y","inputSchema":{"type":"object"}}]}}""",
                )
                "tools/call" -> listOf(
                    """{"jsonrpc":"2.0","id":${id(msg)},"result":{"content":[{"type":"text","text":"ok"}],"structuredContent":${msg["params"]!!.jsonObject["arguments"]},"isError":false}}""",
                )
                else -> emptyList()
            }
        }
        val notifications = mutableListOf<String>()
        val client = McpLineClient(server.clientIn, server.clientOut, onNotification = { m, _ -> notifications += m })
        val init = client.initialize("test-agent")
        assertEquals("app-mcp-hub", init["serverInfo"]!!.jsonObject["name"]!!.jsonPrimitive.content)
        assertEquals(listOf("a.x", "b.y"), client.listTools().map { it["name"]!!.jsonPrimitive.content })
        assertEquals(listOf("notifications/tools/list_changed"), notifications)
        val result = client.callTool("a.x", buildJsonObject { put("text", "hi") })
        assertEquals(JsonPrimitive(false), result["isError"])
        assertEquals("hi", result["structuredContent"]!!.jsonObject["text"]!!.jsonPrimitive.content)
        client.close()
        server.worker.join(5_000)
        val methods = synchronized(server.received) { server.received.map { it["method"]?.jsonPrimitive?.content ?: "reply:${it["id"]}" } }
        assertEquals(
            listOf("initialize", "notifications/initialized", "tools/list", "reply:\"srv-1\"", "tools/list", "tools/call"),
            methods,
        )
        val reply = synchronized(server.received) { server.received.first { it["id"]?.toString() == "\"srv-1\"" } }
        assertEquals(McpLineClient.METHOD_NOT_FOUND, reply["error"]!!.jsonObject["code"]!!.jsonPrimitive.content.toInt())
    }

    @Test
    fun errorsAndDisconnect() {
        val server = FakeServer { msg ->
            when (msg["method"]?.jsonPrimitive?.content) {
                "tools/call" -> listOf("""{"jsonrpc":"2.0","id":${id(msg)},"error":{"code":-32602,"message":"未知工具"}}""")
                else -> emptyList()
            }
        }
        val client = McpLineClient(server.clientIn, server.clientOut)
        try {
            client.callTool("nope")
            fail("应抛出")
        } catch (e: McpException) {
            assertEquals(-32602, e.code)
            assertEquals("未知工具", e.message)
        }
        server.toClient.close()
        try {
            client.listTools()
            fail("断开后应抛出")
        } catch (e: McpException) {
            assertEquals(McpLineClient.CONNECTION_CLOSED, e.code)
        }
    }

    @Test
    fun oversizedLineIsRejected() {
        val server = FakeServer { msg -> listOf("""{"jsonrpc":"2.0","id":${id(msg)},"result":{"pad":"${"x".repeat(200)}"}}""") }
        val client = McpLineClient(server.clientIn, server.clientOut, maxLineChars = 64)
        val e = runCatching { client.request("ping", null) }.exceptionOrNull()
        assertTrue("$e", e is McpException && e.code == McpLineClient.PARSE_ERROR)
    }
}
