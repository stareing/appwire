package dev.appmcp.sample.android

import android.util.Base64
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.EOFException
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.security.SecureRandom

/**
 * 自检专用的最小 WebSocket 客户端（RFC 6455，仅文本帧），以「网页 SDK」身份直接对 Hub 的 `/app` 收发协议消息。
 *
 * @why 原生 SDK（crates/native）不发 `app/diagnostic`（只有网页 SDK 会上报拦截等问题），自检要在 R8 后的
 *   绑定上触发 `HubEvent.AppDiagnostic`，只能手动发协议消息；Android 没有 java.net.http.WebSocket，也不为自检引入依赖。
 * @invariant 仅用于回环地址上的自检，不处理分片 / 扩展；收到 ping 以外的控制帧即视为连接结束。
 */
internal class RawAppConnection private constructor(private val socket: Socket) : AutoCloseable {
    private val input = DataInputStream(socket.getInputStream())
    private val output: OutputStream = socket.getOutputStream()
    private val random = SecureRandom()

    companion object {
        private const val OP_TEXT = 0x1
        private const val OP_CLOSE = 0x8
        private const val OP_PING = 0x9
        private const val OP_PONG = 0xA

        /** 连接 `ws://<listenAddr>/app` 并完成握手。@error IllegalStateException：握手未返回 101。 */
        fun open(listenAddr: String, timeoutMs: Int): RawAppConnection {
            val host = listenAddr.substringBeforeLast(':')
            val port = listenAddr.substringAfterLast(':').toInt()
            val socket = Socket()
            try {
                socket.connect(InetSocketAddress(host, port), timeoutMs)
                socket.soTimeout = timeoutMs
                val key = Base64.encodeToString(ByteArray(16).also { SecureRandom().nextBytes(it) }, Base64.NO_WRAP)
                val request = "GET /app HTTP/1.1\r\nHost: $listenAddr\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
                    "Sec-WebSocket-Key: $key\r\nSec-WebSocket-Version: 13\r\n\r\n"
                socket.getOutputStream().apply { write(request.toByteArray()); flush() }
                val status = readHeaders(DataInputStream(socket.getInputStream()))
                check(status.contains(" 101 ")) { "WebSocket 握手失败：$status" }
                return RawAppConnection(socket)
            } catch (e: Throwable) {
                socket.close()
                throw e
            }
        }

        /** 读到空行为止，返回状态行。逐字节读，避免缓冲吞掉其后的帧。 */
        private fun readHeaders(input: DataInputStream): String {
            val buf = ByteArrayOutputStream()
            var tail = 0
            while (tail != 0x0D0A0D0A) {
                val b = input.read()
                if (b < 0) throw EOFException("握手响应不完整")
                buf.write(b)
                tail = (tail shl 8) or b
            }
            return buf.toString("UTF-8").lineSequence().first()
        }
    }

    /** 发送一条 JSON-RPC 消息（客户端帧须加掩码）。 */
    fun send(message: JsonObject) = sendFrame(OP_TEXT, message.toString().toByteArray())

    /** 读取下一条文本消息；自动回应 ping。@error EOFException：对端关闭。 */
    fun receive(): JsonObject {
        while (true) {
            val b0 = input.readUnsignedByte()
            val b1 = input.readUnsignedByte()
            val len = when (val l = b1 and 0x7F) {
                126 -> input.readUnsignedShort().toLong()
                127 -> input.readLong()
                else -> l.toLong()
            }
            check(len in 0..(1 shl 20)) { "帧长度异常：$len" }
            val payload = ByteArray(len.toInt()).also { input.readFully(it) }
            when (b0 and 0x0F) {
                OP_TEXT -> return Json.parseToJsonElement(payload.toString(Charsets.UTF_8)).jsonObject
                OP_PING -> sendFrame(OP_PONG, payload)
                OP_CLOSE -> throw EOFException("对端关闭连接")
            }
        }
    }

    private fun sendFrame(opcode: Int, payload: ByteArray) {
        val frame = ByteArrayOutputStream()
        frame.write(0x80 or opcode)
        when {
            payload.size < 126 -> frame.write(0x80 or payload.size)
            payload.size < 65536 -> {
                frame.write(0x80 or 126)
                frame.write(payload.size ushr 8)
                frame.write(payload.size and 0xFF)
            }
            else -> error("自检消息过长：${payload.size}")
        }
        val mask = ByteArray(4).also { random.nextBytes(it) }
        frame.write(mask)
        frame.write(ByteArray(payload.size) { i -> (payload[i].toInt() xor mask[i % 4].toInt()).toByte() })
        synchronized(output) {
            output.write(frame.toByteArray())
            output.flush()
        }
    }

    override fun close() {
        runCatching { sendFrame(OP_CLOSE, ByteArray(0)) }
        socket.close()
    }
}
