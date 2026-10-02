package dev.appmcp.hub

import com.sun.jna.Library
import com.sun.jna.Native
import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.hub.ffi.DialOutcome
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import dev.appmcp.LifecycleMode as AppLifecycleMode
import dev.appmcp.LifecyclePolicy as AppLifecyclePolicy

/**
 * 宿主名字服务的 Kotlin 端到端（spec/naming.md 4.2，与 Android 的 ToolsService / AndroidNameService 同一形态）：
 * Kotlin 实现 [HubNameService]，拨号时用 socketpair 把一端交给同进程 App 端 SDK（[AppMcp.acceptChannelFd]），另一端交回 Hub；
 * 以及 fd 上的 MCP（[Hub.serveMcpFd]）。只在 Linux 上运行（socketpair 经 JNA 调 libc）。
 */
class NameServiceTest {
    private interface LibC : Library {
        fun socketpair(domain: Int, type: Int, protocol: Int, sv: IntArray): Int
        fun write(fd: Int, buf: ByteArray, count: Long): Long
        fun read(fd: Int, buf: ByteArray, count: Long): Long
        fun close(fd: Int): Int
    }

    private val linux = System.getProperty("os.name").lowercase().contains("linux")
    private val libc: LibC? = if (linux) Native.load("c", LibC::class.java) else null

    private fun socketPair(): Pair<Int, Int> {
        val sv = IntArray(2)
        check(libc!!.socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0) { "socketpair 失败" }
        return sv[0] to sv[1]
    }

    private fun manifest(appId: String) =
        """{"manifestVersion":1,"appId":"$appId","name":"fd 笔记","tools":[{"name":"echo","description":"回显","inputSchema":{"type":"object"}}]}"""

    private inner class FakeAndroid(private val app: AppMcp) : HubNameService {
        val released: MutableList<ULong> = Collections.synchronizedList(mutableListOf())
        var dials = 0

        override fun discover(): List<NamedApp> = listOf(NamedApp(APP, true, false, "dev.example/ToolsService", manifest(APP)))

        override fun dial(appId: String, timeoutMs: ULong): DialOutcome {
            if (appId != APP) return DialOutcome.Failed("HUB_NOT_TRUSTED", "App 拒绝了该 Hub")
            val (appEnd, hubEnd) = socketPair()
            val offer = app.acceptChannelFd(appEnd)
            if (offer !is dev.appmcp.ffi.ChannelOffer.Accepted) {
                libc!!.close(hubEnd)
                return DialOutcome.Failed("CHANNEL_LIMIT", "$offer")
            }
            dials++
            return DialOutcome.Channel(hubEnd, dials.toULong(), null)
        }

        override fun release(lease: ULong) {
            released += lease
        }
    }

    @Test
    fun dialOverSocketpairAndReleaseAfterGrace() = runBlocking {
        if (!linux) return@runBlocking
        val app = AppMcp.create(
            AppMcpConfig(
                APP, "fd 笔记",
                hostUrl = "ws://127.0.0.1:1/app",
                dispatcher = Dispatchers.Default,
                lifecycle = AppLifecyclePolicy(mode = AppLifecycleMode.ON_DEMAND),
            ),
        )
        app.tool("echo", "回显") { args, _ -> args }
        app.start()
        val android = FakeAndroid(app)
        val hub = Hub.startWithNameService(
            HubConfig(enableListen = false, enableIpc = false, channelGraceMs = 200u, leaseTtlMs = 0u, listChangedDebounceMs = 10u),
            "android", android,
        )
        try {
            withTimeout(10.seconds) { while (hub.tools().none { it.name == "$APP.echo" }) delay(10) }
            assertEquals(0, android.dials, "发现不拨号")
            val r = hub.callTool("$APP.echo", buildJsonObject { put("text", "经 socketpair") })
            assertEquals("经 socketpair", r.getOrThrow()!!.jsonObject["text"]!!.jsonPrimitive.content)
            withTimeout(10.seconds) { while (android.released.isEmpty()) delay(10) }
            assertEquals(listOf(1uL), android.released.toList())
            withTimeout(10.seconds) { while (app.currentState().status != dev.appmcp.StateStatus.DORMANT) delay(10) }

            hub.nameServiceInstalled(NamedApp("late", true, false, "", manifest("late")))
            withTimeout(10.seconds) { while (hub.tools().none { it.name == "late.echo" }) delay(10) }
            val refused = hub.callTool("late.echo")
            assertEquals("LAUNCH_FAILED", refused.error?.kind)
            assertTrue(refused.error?.detailsJson.orEmpty().contains("HUB_NOT_TRUSTED"), "${refused.error}")
            hub.nameServiceRemoved("late")
            withTimeout(10.seconds) { while (hub.tools().any { it.name == "late.echo" }) delay(10) }
        } finally {
            hub.close()
            app.close()
        }
    }

    @Test
    fun mcpOverFd() = runBlocking {
        if (!linux) return@runBlocking
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false))
        val (agent, hubEnd) = socketPair()
        try {
            val serving = async(Dispatchers.IO) { hub.serveMcpFd(hubEnd) }
            val c = libc!!
            val pending = StringBuilder()
            fun rpc(msg: String): JsonObject? {
                val bytes = (msg + "\n").toByteArray()
                check(c.write(agent, bytes, bytes.size.toLong()) == bytes.size.toLong())
                if (!msg.contains("\"id\"")) return null
                val buf = ByteArray(65536)
                while (!pending.contains('\n')) {
                    val n = c.read(agent, buf, buf.size.toLong())
                    check(n > 0) { "Hub 关闭了连接" }
                    pending.append(String(buf, 0, n.toInt()))
                }
                val line = pending.substring(0, pending.indexOf("\n"))
                pending.delete(0, line.length + 1)
                return Json.parseToJsonElement(line).jsonObject
            }
            val init = rpc("""{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"kt","version":"0"}}}""")!!
            assertTrue(init["result"]!!.jsonObject.containsKey("serverInfo"), "$init")
            rpc("""{"jsonrpc":"2.0","method":"notifications/initialized"}""")
            val list = rpc("""{"jsonrpc":"2.0","id":2,"method":"tools/list"}""")!!
            assertTrue(list["result"]!!.jsonObject["tools"]!!.jsonArray.isNotEmpty(), "$list")
            c.close(agent)
            withTimeout(10.seconds) { serving.await() }
        } finally {
            hub.close()
        }
    }

    private companion object {
        const val APP = "fd-notes"
        const val AF_UNIX = 1
        const val SOCK_STREAM = 1
    }
}
