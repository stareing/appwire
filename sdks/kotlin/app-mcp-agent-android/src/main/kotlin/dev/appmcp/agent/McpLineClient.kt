package dev.appmcp.agent

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import java.io.BufferedReader
import java.io.Closeable
import java.io.InputStream
import java.io.InputStreamReader
import java.io.OutputStream
import java.io.OutputStreamWriter
import java.io.Writer

/** MCP 请求失败：对端返回 JSON-RPC 错误，或连接已断开。 */
class McpException(val code: Int, message: String, val data: JsonElement? = null) : Exception(message)

/**
 * 最小 MCP 客户端会话（TASKS 4g f）：一条双向字节流上每行一条 JSON-RPC 消息（与 MCP stdio 传输相同的帧）。
 * 只做 Agent 需要的三件事：[initialize]、[listTools]、[callTool]。请求串行发送（同一时刻只有一个在途），
 * 等待回复期间收到的通知（如 `notifications/tools/list_changed`）交给 [onNotification]，服务端发来的请求以
 * "方法不存在"回复。阻塞 I/O：在后台线程调用（[HubClient] 在 `Dispatchers.IO` 上调用）。
 *
 * @invariant 单条消息不超过 [maxLineChars]（超出按协议错误断开），防止异常对端占满内存。
 */
class McpLineClient(
    input: InputStream,
    output: OutputStream,
    private val maxLineChars: Int = DEFAULT_MAX_LINE_CHARS,
    private val onNotification: (method: String, params: JsonElement?) -> Unit = { _, _ -> },
) : Closeable {
    private val reader = BufferedReader(InputStreamReader(input, Charsets.UTF_8))
    private val writer: Writer = OutputStreamWriter(output, Charsets.UTF_8)
    private var nextId = 1L
    private val lock = Any()

    /** 握手：`initialize` + `notifications/initialized`。返回服务端的 `initialize` 结果（含 `serverInfo`、`capabilities`）。 */
    fun initialize(clientName: String, clientVersion: String = "0.1.0"): JsonObject {
        val result = request("initialize", buildJsonObject {
            put("protocolVersion", PROTOCOL_VERSION)
            put("capabilities", JsonObject(emptyMap()))
            put("clientInfo", buildJsonObject { put("name", clientName); put("version", clientVersion) })
        })
        notify("notifications/initialized", null)
        return result.jsonObject
    }

    /** `tools/list`（跟随 `nextCursor` 取完全部页）。 */
    fun listTools(): List<JsonObject> {
        val all = mutableListOf<JsonObject>()
        var cursor: String? = null
        do {
            val params = cursor?.let { c -> buildJsonObject { put("cursor", c) } }
            val page = request("tools/list", params).jsonObject
            page["tools"]?.jsonArray?.forEach { all += it.jsonObject }
            cursor = (page["nextCursor"] as? JsonPrimitive)?.takeIf { it.isString }?.content
        } while (cursor != null)
        return all
    }

    /** `tools/call`。返回 MCP 结果（`content`、`structuredContent`、`isError`）；工具失败体现在 `isError`，不抛异常。 */
    fun callTool(name: String, arguments: JsonObject = JsonObject(emptyMap())): JsonObject =
        request("tools/call", buildJsonObject {
            put("name", name)
            put("arguments", arguments)
        }).jsonObject

    /** 发一个请求并等待同 id 的回复。 */
    fun request(method: String, params: JsonElement?): JsonElement = synchronized(lock) {
        val id = nextId++
        send(buildJsonObject {
            put("jsonrpc", "2.0")
            put("id", id)
            put("method", method)
            if (params != null) put("params", params)
        })
        awaitResult(id, method)
    }

    /** 读到同 id 的回复为止；途中的通知交给 [onNotification]，服务端请求回复"方法不存在"。 */
    private fun awaitResult(id: Long, method: String): JsonElement {
        while (true) {
            val msg = readMessage() ?: throw McpException(CONNECTION_CLOSED, "Hub 关闭了连接（$method 未得到回复）")
            val incomingMethod = (msg["method"] as? JsonPrimitive)?.takeIf { it.isString }?.content
            val msgId = msg["id"]
            if (incomingMethod != null) {
                if (msgId != null) replyMethodNotFound(msgId, incomingMethod)
                else runCatching { onNotification(incomingMethod, msg["params"]) }
                continue
            }
            // 其他 id 的回复（本客户端串行发送，不应出现）：忽略。
            if ((msgId as? JsonPrimitive)?.longOrNull != id) continue
            msg["error"]?.let { e ->
                val err = e.jsonObject
                throw McpException(
                    err["code"]?.jsonPrimitive?.longOrNull?.toInt() ?: INTERNAL_ERROR,
                    err["message"]?.jsonPrimitive?.content ?: "未知错误",
                    err["data"],
                )
            }
            return msg["result"] ?: JsonObject(emptyMap())
        }
    }

    /** 发一个通知。 */
    fun notify(method: String, params: JsonElement?) = synchronized(lock) {
        send(buildJsonObject {
            put("jsonrpc", "2.0")
            put("method", method)
            if (params != null) put("params", params)
        })
    }

    private fun send(msg: JsonObject) {
        writer.write(msg.toString())
        writer.write("\n")
        writer.flush()
    }

    private fun replyMethodNotFound(id: JsonElement, method: String) = send(buildJsonObject {
        put("jsonrpc", "2.0")
        put("id", id)
        put("error", buildJsonObject {
            put("code", METHOD_NOT_FOUND)
            put("message", "客户端不支持 $method")
        })
    })

    /** 读一行 JSON 对象；流结束时为 null。空行跳过；不是 JSON 对象按协议错误抛出。 */
    private fun readMessage(): JsonObject? {
        while (true) {
            val line = readBoundedLine() ?: return null
            if (line.isBlank()) continue
            return runCatching { json.parseToJsonElement(line).jsonObject }
                .getOrElse { throw McpException(PARSE_ERROR, "Hub 发来的不是 JSON 对象：${line.take(80)}") }
        }
    }

    private fun readBoundedLine(): String? {
        val sb = StringBuilder()
        while (true) {
            val c = reader.read()
            if (c < 0) return if (sb.isEmpty()) null else sb.toString()
            if (c == '\n'.code) return sb.toString()
            if (sb.length >= maxLineChars) throw McpException(PARSE_ERROR, "单条消息超过 $maxLineChars 字符")
            sb.append(c.toChar())
        }
    }

    override fun close() {
        runCatching { writer.close() }
        runCatching { reader.close() }
    }

    companion object {
        /** 客户端声明的 MCP 协议版本；服务端可协商为它支持的版本。 */
        const val PROTOCOL_VERSION = "2025-06-18"
        const val DEFAULT_MAX_LINE_CHARS = 16 * 1024 * 1024
        const val PARSE_ERROR = -32700
        const val METHOD_NOT_FOUND = -32601
        const val INTERNAL_ERROR = -32603

        /** 连接断开（非 JSON-RPC 码，本库自定义）。 */
        const val CONNECTION_CLOSED = -1

        private val json = Json { ignoreUnknownKeys = true }
    }
}
