package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.hub.ffi.HubEvent as Ev
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import dev.appmcp.Risk as AppRisk
import dev.appmcp.LifecycleMode as AppLifecycleMode
import dev.appmcp.LifecyclePolicy as AppLifecyclePolicy
import dev.appmcp.WakeDescriptor as AppWakeDescriptor
import dev.appmcp.WakeKind as AppWakeKind

/** 嵌入式 Hub（随机端口）+ 同进程 App 端 SDK（dev.appmcp.AppMcp）经真实 WebSocket 连上。 */
class HubIntegrationTest {
    private val textSchema = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") { putJsonObject("text") { put("type", "string") } }
    }

    @Test
    fun endToEnd() = runBlocking {
        val hub = Hub.start(HubConfig(wsAddr = "127.0.0.1:0", approvalMinRisk = Risk.DESTRUCTIVE))
        val events = Channel<HubEvent>(Channel.UNLIMITED)
        val collector = launch(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) {
            hub.events.collect { events.send(it) }
        }
        val approvals = Collections.synchronizedList(mutableListOf<ApprovalRequest>())
        hub.setApprovalHandler { req ->
            approvals += req
            false
        }

        val app = AppMcp.create(
            AppMcpConfig("notes", "笔记", hostUrl = "ws://${hub.wsAddr}", dispatcher = Dispatchers.Default),
        )
        app.tool("add", "添加笔记", textSchema, risk = AppRisk.WRITE) { args, _ ->
            mapOf("saved" to args["text"]!!.jsonPrimitive.content)
        }
        app.tool("clear", "清空笔记", risk = AppRisk.DESTRUCTIVE) { _, _ -> mapOf("cleared" to true) }

        try {
            app.start()
            suspend fun expect(what: String, pred: (HubEvent) -> Boolean): HubEvent = withTimeout(10.seconds) {
                var e: HubEvent
                do { e = events.receive() } while (!pred(e))
                e
            }.also { println("收到事件（$what）：$it") }

            // 事件：App 连上
            expect("AppConnected") { it is Ev.AppConnected && it.appId == "notes" }

            // 列工具（注册可能稍后到达）
            withTimeout(10.seconds) {
                while (hub.tools(ToolFilter(apps = listOf("notes"))).size < 2) delay(20)
            }
            val tools = hub.tools(ToolFilter(apps = listOf("notes"), includeBuiltin = false))
            assertEquals(setOf("notes.add", "notes.clear"), tools.map { it.name }.toSet())
            val add = tools.first { it.name == "notes.add" }
            assertEquals(Availability.AVAILABLE, add.availability)
            assertEquals("object", add.inputSchema.jsonObject["type"]!!.jsonPrimitive.content)
            assertTrue(hub.apps().any { it.appId == "notes" && it.connected })

            // callTool
            val r = hub.callTool("notes.add", buildJsonObject { put("text", "买牛奶") }, timeout = 5.seconds)
            assertEquals(null, r.error)
            assertEquals("买牛奶", (r.getOrThrow() as JsonObject)["saved"]!!.jsonPrimitive.content)

            // exportTools + dispatch（OpenAI Chat）
            val exported = hub.exportTools(ToolFormat.OPEN_AI_CHAT, ToolFilter(apps = listOf("notes")))
            val names = exported.jsonArray.map { it.jsonObject["function"]!!.jsonObject["name"]!!.jsonPrimitive.content }
            assertTrue("notes__add" in names, names.toString())
            val reply = hub.dispatch(
                ToolFormat.OPEN_AI_CHAT,
                buildJsonObject {
                    put("id", "call_1")
                    put("type", "function")
                    putJsonObject("function") {
                        put("name", "notes__add")
                        put("arguments", JsonPrimitive("""{"text":"来自 LLM"}"""))
                    }
                },
            ).jsonObject
            assertEquals("tool", reply["role"]!!.jsonPrimitive.content)
            assertEquals("call_1", reply["tool_call_id"]!!.jsonPrimitive.content)
            assertTrue("来自 LLM" in reply["content"]!!.jsonPrimitive.content, reply.toString())

            // 审批拒绝 → USER_REJECTED
            val rejected = hub.callTool("notes.clear", timeout = 5.seconds)
            assertEquals("USER_REJECTED", rejected.error?.kind)
            val ex = assertFailsWith<ToolException> { rejected.getOrThrow() }
            assertEquals("USER_REJECTED", ex.kind)
            assertEquals(1, approvals.size)
            assertEquals("notes", approvals[0].appId)
            assertEquals(Risk.DESTRUCTIVE, approvals[0].risk)
            // 低于阈值的 write 工具不询问
            assertTrue(approvals.none { it.tool.endsWith("add") })

            // handler 在指定的协程上下文中执行（Android 上即 Dispatchers.Main）；挂起后同意 → 成功
            val uiThread = java.util.concurrent.Executors.newSingleThreadExecutor { r -> Thread(r, "fake-ui") }
            val threads = Collections.synchronizedList(mutableListOf<String>())
            try {
                hub.setApprovalHandler(uiThread.asCoroutineDispatcher()) { _ ->
                    delay(10)
                    threads += Thread.currentThread().name
                    true
                }
                val approved = hub.callTool("notes.clear", timeout = 5.seconds)
                assertEquals(null, approved.error, approved.toString())
                assertTrue(threads.size == 1 && threads[0].startsWith("fake-ui"), threads.toString())
                // handler 抛出异常 → 拒绝
                hub.setApprovalHandler { _ -> error("UI 崩溃") }
                assertEquals("USER_REJECTED", hub.callTool("notes.clear", timeout = 5.seconds).error?.kind)
            } finally {
                uiThread.shutdown()
            }

            // 名称无法解析 → HubException
            assertFailsWith<HubException> { hub.callTool("nosuchapp.x") }

            // 事件：App 断开
            app.stop()
            expect("AppDisconnected") { it is Ev.AppDisconnected && it.appId == "notes" }
        } finally {
            app.close()
            collector.cancel()
            hub.close()
        }
        Unit
    }

    @Test
    fun dormantAppWokenByCustomWaker() = runBlocking {
        val hub = Hub.start(
            HubConfig(wsAddr = "127.0.0.1:0", leaseTtlMs = 0uL, wakeTimeoutMs = 10_000uL, listChangedDebounceMs = 20uL),
        )
        val events = Channel<HubEvent>(Channel.UNLIMITED)
        val collector = launch(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) {
            hub.events.collect { events.send(it) }
        }
        val app = AppMcp.create(
            AppMcpConfig(
                "sleepy", "会睡觉的 App",
                hostUrl = "ws://${hub.wsAddr}",
                instanceId = "s1",
                dispatcher = Dispatchers.Default,
                lifecycle = AppLifecyclePolicy(
                    mode = AppLifecycleMode.IDLE,
                    idleTimeoutMillis = 300,
                    wake = AppWakeDescriptor(AppWakeKind.ANDROID_INTENT, "dev.example/.WakeReceiver", true),
                ),
            ),
        )
        app.tool("ping", "回显") { args, _ -> mapOf("echo" to args.toString()) }
        val wakes = Collections.synchronizedList(mutableListOf<WakeRequest>())
        // 厂商在这里发送显式广播；测试中直接让同进程 App 处理激活参数。
        hub.setWaker { req ->
            wakes += req
            if (!app.handleWake(req.activationArg)) throw WakeFailedException("LAUNCH_FAILED", "不认识的激活参数")
        }
        suspend fun expect(pred: (HubEvent) -> Boolean): HubEvent = withTimeout(10.seconds) {
            var e: HubEvent
            do { e = events.receive() } while (!pred(e))
            e
        }
        try {
            app.start()
            val d = expect { it is Ev.AppDormant && it.appId == "sleepy" } as Ev.AppDormant
            assertEquals("s1", d.instanceId)
            val info = hub.apps().first { it.appId == "sleepy" }
            assertTrue(info.isDormant)
            assertEquals(listOf("s1"), info.dormantInstances.map { it.instanceId })
            val tools = hub.tools(ToolFilter(apps = listOf("sleepy"), includeBuiltin = false))
            assertEquals(listOf(Availability.DORMANT), tools.map { it.availability })

            val r = hub.callTool("sleepy.ping", buildJsonObject { put("x", 1) }, timeout = 10.seconds)
            assertEquals(null, r.error, r.toString())
            assertEquals("s1", r.instanceId)
            assertEquals(1, wakes.size)
            val w = wakes[0]
            assertEquals("sleepy", w.appId)
            assertEquals("s1", w.instanceId)
            assertEquals(WakeKind.ANDROID_INTENT, w.descriptor.kind)
            assertEquals("app-mcp-wake:${w.token}", w.activationArg)
            expect { it is Ev.AppWaking && it.instanceId == "s1" }

            // 失败类别透传；清除后恢复默认实现
            expect { it is Ev.AppDormant && it.appId == "sleepy" }
            hub.setWaker { throw WakeFailedException("APP_NOT_INSTALLED", "没装") }
            val bad = hub.callTool("sleepy.ping", timeout = 10.seconds)
            assertEquals("APP_NOT_INSTALLED", bad.error?.kind)
            hub.setWaker(null)
        } finally {
            app.close()
            collector.cancel()
            hub.close()
        }
        Unit
    }

    @Test
    fun parseFormatAndShutdown() {
        assertEquals(ToolFormat.ANTHROPIC, Hub.parseFormat("anthropic"))
        assertFailsWith<HubException> { Hub.parseFormat("nope") }
        val hub = Hub.start(HubConfig(enableWs = false))
        assertEquals(null, hub.wsAddr)
        assertEquals(setOf("apps.list", "apps.select", "apps.overview"), hub.tools().map { it.name }.toSet())
        hub.close()
        hub.close() // 幂等
    }
}
