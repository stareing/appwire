package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import dev.appmcp.hub.ffi.HubEvent as Ev
import dev.appmcp.hub.ffi.HubException as FfiHubException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
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
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import dev.appmcp.Risk as AppRisk
import dev.appmcp.LifecycleMode as AppLifecycleMode
import dev.appmcp.LifecyclePolicy as AppLifecyclePolicy
import dev.appmcp.WakeDescriptor as AppWakeDescriptor
import dev.appmcp.WakeKind as AppWakeKind

/**
 * 嵌入式 Hub（随机端口）+ 同进程 App 端 SDK（dev.appmcp.AppMcp）经真实 WebSocket / 本地 IPC 连上。
 * 测试关闭默认 IPC 端点（`enableIpc = false`），不占用本机常驻 Host 的端点；IPC 用临时端点单独测试。
 */
class HubIntegrationTest {
    private val textSchema = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") { putJsonObject("text") { put("type", "string") } }
    }

    @Test
    fun endToEnd() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false, approvalMinRisk = Risk.DESTRUCTIVE))
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
            AppMcpConfig("notes", "笔记", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default),
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

            // 运行状态：实例的连接 ID 与 App 端 SDK 看到的一致
            val st = hub.status()
            assertEquals("app-mcp", st.service)
            assertEquals(hub.listenAddr, st.listen)
            assertTrue(st.reports.isEmpty())
            val notes = st.apps.first { it.appId == "notes" }
            assertEquals(AppState.CONNECTED, notes.state)
            assertEquals(InstanceState.CONNECTED, notes.instances[0].state)
            val cid = notes.instances[0].info.connectionId
            assertTrue(cid != null && cid == app.connectionId, "$cid vs ${app.connectionId}")
            assertEquals(cid, hub.apps().first { it.appId == "notes" }.instances[0].connectionId)

            // callTool
            val r = hub.callTool("notes.add", buildJsonObject { put("text", "买牛奶") }, timeout = 5.seconds)
            assertEquals(null, r.error)
            assertEquals("买牛奶", (r.getOrThrow() as JsonObject)["saved"]!!.jsonPrimitive.content)
            // Hub API 调用方的 Agent 任务（spec/hub-api.md 3.6）
            val task: AgentTaskStatus = assertNotNull(hub.status().tasks?.firstOrNull { it.caller == "api" })
            assertEquals(CallerKind.API, task.kind)
            assertTrue(task.id.startsWith("task-"), task.id)

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
            // principal / clientName 只在 MCP 出口发起的审批中出现
            assertEquals(null, approvals[0].principal)
            assertEquals(null, approvals[0].clientName)
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
            HubConfig(listen = "127.0.0.1:0", enableIpc = false, leaseTtlMs = 0uL, wakeTimeoutMs = 10_000uL, listChangedDebounceMs = 20uL),
        )
        val events = Channel<HubEvent>(Channel.UNLIMITED)
        val collector = launch(Dispatchers.Default, start = CoroutineStart.UNDISPATCHED) {
            hub.events.collect { events.send(it) }
        }
        val app = AppMcp.create(
            AppMcpConfig(
                "sleepy", "会睡觉的 App",
                hostUrl = "ws://${hub.listenAddr}/app",
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

    /** 关闭的能力报 [HubException.Unsupported]（与 `Io` 区分）。本机库为完整能力，调用成功；精简构建由 HubSelfTest 真机覆盖。 */
    @Test
    fun unsupportedFeatureIsDistinctCategory() = runBlocking {
        val base = HubConfig(listen = "127.0.0.1:0", enableIpc = false)
        runCatching { Hub.start(base.copy(mcpHttp = true)).close() }
            .exceptionOrNull()?.let { assertTrue(it is FfiHubException.Unsupported, "应为 Unsupported：$it") }
        Hub.start(base).use { hub ->
            runCatching { hub.serveHttp("127.0.0.1:0") }
                .exceptionOrNull()?.let { assertTrue(it is FfiHubException.Unsupported, "应为 Unsupported：$it") }
        }
        val e: HubException = FfiHubException.Unsupported("缺少 `mcp-server`")
        assertTrue(e !is FfiHubException.Io)
        assertTrue(e.message.orEmpty().contains("`mcp-server`"))
    }

    @Test
    fun parseFormatAndShutdown() {
        assertEquals(ToolFormat.ANTHROPIC, Hub.parseFormat("anthropic"))
        assertFailsWith<HubException> { Hub.parseFormat("nope") }
        val hub = Hub.start(HubConfig(enableListen = false, enableIpc = false))
        assertEquals(null, hub.listenAddr)
        assertEquals(null, hub.ipcEndpoint)
        assertEquals(
            setOf("apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release", "apps.lock", "apps.unlock",
                "apps.calls", "apps.cancel", "apps.events.subscribe", "apps.events.unsubscribe", "apps.events", "apps.search", "apps.intents",
                "apps.undo"),
            hub.tools().map { it.name }.toSet(),
        )
        val st = hub.status()
        assertEquals(null, st.listen)
        assertTrue(st.apps.isEmpty() && !st.mcpHttp && !st.auth.tokenConfigured)
        hub.close()
        hub.close() // 幂等
    }

    @Test
    fun stateDirReportsDormantStore() {
        val dir = java.nio.file.Files.createTempDirectory("app-mcp-kt-state-").toFile()
        try {
            val dormant = java.io.File(dir, "dormant").apply { mkdirs() }
            java.io.File(dormant, "broken.json").writeText("{")
            Hub.start(HubConfig(enableListen = false, enableIpc = false)).use { assertEquals(null, it.status().dormantStore) }
            Hub.start(HubConfig(enableListen = false, enableIpc = false, stateDir = dir.path)).use { hub ->
                val store: DormantStoreStatus = assertNotNull(hub.status().dormantStore)
                assertEquals(dormant.path, store.dir)
                assertEquals(listOf(0uL, 0uL, 0uL), listOf(store.loadedInstances, store.expiredInstances, store.writes))
                assertEquals(listOf("broken.json"), store.issues.map(StoreIssue::file))
                assertEquals(null, store.lastError)
            }
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun progressiveExposureConfig() {
        val hub = Hub.start(
            HubConfig(
                enableListen = false,
                enableIpc = false,
                toolExposure = ToolExposure.PROGRESSIVE,
                toolExposureThreshold = 5u,
                waker = dev.appmcp.hub.ffi.WakerConfig.Disabled,
            ),
        )
        // 渐进暴露：没有展开的 App 时只有内置工具（含 apps.tools）
        assertEquals(
            listOf(
                "apps.list", "apps.select", "apps.overview", "apps.tools", "apps.activate", "apps.release",
                "apps.lock", "apps.unlock", "apps.calls", "apps.cancel", "apps.events.subscribe", "apps.events.unsubscribe", "apps.events", "apps.search", "apps.intents",
                "apps.undo",
            ),
            hub.tools(ToolFilter(session = "c1")).map { it.name },
        )
        hub.close()
    }

    /** spec/hub-api.md 3.6 / 3.7：无会话 MCP 请求的配置可设置；启动时没有 Agent 任务。 */
    @Test
    fun statelessConfig() {
        val config = HubConfig(
            enableListen = false,
            enableIpc = false,
            taskIdleTtlMs = 0uL,
            statelessToolExposure = ToolExposure.PROGRESSIVE,
            principalSelectTtlMs = 1500uL,
            statelessListTtlMs = 750uL,
        )
        assertEquals(ToolExposure.PROGRESSIVE, config.statelessToolExposure)
        Hub.start(config).use { hub -> assertEquals(emptyList<AgentTaskStatus>(), hub.status().tasks) }
    }

    /** spec/hub-api.md 3.6：MCP 出口协议版本与 listen 上限可设置；status 报告 listen 流数。 */
    @Test
    fun mcpListenConfig() {
        val config = HubConfig(
            enableListen = false,
            enableIpc = false,
            mcpProtocolMode = McpProtocolMode.LEGACY_ONLY,
            maxListenStreams = 0u,
            maxListenResources = 8u,
        )
        assertEquals(McpProtocolMode.LEGACY_ONLY, config.mcpProtocolMode)
        Hub.start(config).use { hub -> assertEquals(0uL, hub.status().mcpListenStreams) }
    }

    /** spec/hub-api.md 3.6「任务句柄」：每主体任务句柄上限可设置（0 关闭），缺省为空即取 Hub 默认。 */
    @Test
    fun maxTaskHandlesConfig() {
        assertEquals(null, HubConfig().maxTaskHandles)
        for (n in listOf(0u, 5u)) {
            val config = HubConfig(enableListen = false, enableIpc = false, maxTaskHandles = n)
            assertEquals(n, config.maxTaskHandles)
            Hub.start(config).use { hub -> assertEquals(0uL, hub.status().mcpSessions) }
        }
    }

    /** spec/hub-api.md 3.14 / 3.15：HubTool.surface / page、callTool(idempotencyKey) 原样转交、routedTo、navigateTimeoutMs。 */
    @Test
    fun surfacePageAndIdempotencyKey() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false, navigateTimeoutMs = 800uL))
        val app = AppMcp.create(
            AppMcpConfig("cafe", "咖啡", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default),
        )
        val echoKey: dev.appmcp.ToolFunction = { _, ctx -> buildJsonObject { put("key", ctx.idempotencyKey) } }
        app.tool("cart.checkout", "结算", surface = dev.appmcp.ToolSurface.VIEW, page = "cart", handler = echoKey)
        app.tool("order.submit", "下单", handler = echoKey)
        try {
            app.start()
            val tools = withTimeout(10.seconds) {
                var t = hub.tools(ToolFilter(apps = listOf("cafe"), includeBuiltin = false))
                while (t.size < 2 || t.any { it.availability != Availability.AVAILABLE }) {
                    delay(20)
                    t = hub.tools(ToolFilter(apps = listOf("cafe"), includeBuiltin = false))
                }
                t
            }
            val checkout = tools.first { it.tool == "cart.checkout" }
            assertEquals(ToolSurface.VIEW to "cart", checkout.surface to checkout.page)
            val submit = tools.first { it.tool == "order.submit" }
            assertEquals(ToolSurface.APP to null, submit.surface to submit.page)
            val builtins = hub.tools().filter { it.appId == "apps" || it.name.startsWith("apps.") }
            assertTrue(builtins.map { it.name }.containsAll(listOf("apps.activate", "apps.release", "apps.page", "apps.navigate")))
            assertTrue(builtins.all { it.surface == null && it.page == null })

            val out = hub.callTool("cafe.order.submit", idempotencyKey = "order-7")
            assertEquals(null, out.error, out.toString())
            assertEquals("order-7", out.data?.jsonObject?.get("key")?.jsonPrimitive?.content)
            assertEquals(null, out.routedTo)
            assertEquals(false, out.woke, "已连接的 App 不唤醒")
            assertTrue(out.durationMs >= 0)
            assertEquals("INVALID_INPUT", hub.callTool("cafe.order.submit", idempotencyKey = "").error?.kind)
        } finally {
            app.close()
            hub.close()
        }
    }

    /** 第 16 项 P6（spec/hub-api.md 3.15）：callTool(priority) 经 tools/invoke 到达 App，调用队列先交互、后后台。 */
    @Test
    fun priorityReachesAppQueue() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false))
        val app = AppMcp.create(
            AppMcpConfig("jobs", "作业", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default, maxConcurrentCalls = 1),
        )
        val started = Collections.synchronizedList(mutableListOf<String>())
        app.tool("job.run", "执行") { args, _ ->
            args["tag"]?.jsonPrimitive?.content?.let { started.add(it) }
            args["delayMs"]?.jsonPrimitive?.content?.toLong()?.let { delay(it) }
            buildJsonObject { put("ok", true) }
        }
        try {
            app.start()
            withTimeout(10.seconds) {
                while (hub.tools(ToolFilter(apps = listOf("jobs"), includeBuiltin = false))
                        .none { it.availability == Availability.AVAILABLE }
                ) delay(20)
            }
            // 预热：首次调用的总览附带等不计入排队顺序
            assertEquals(null, hub.callTool("jobs.job.run").error)
            fun call(tag: String, priority: CallPriority, delayMs: Int) = async {
                hub.callTool(
                    "jobs.job.run",
                    buildJsonObject { put("tag", tag); put("delayMs", delayMs) },
                    priority = priority,
                )
            }
            val slow = call("slow", CallPriority.NORMAL, 600)
            delay(200)
            val background = call("background", CallPriority.BACKGROUND, 0)
            delay(50)
            val interactive = call("interactive", CallPriority.INTERACTIVE, 0)
            withTimeout(10.seconds) {
                for (out in listOf(slow, background, interactive).map { it.await() }) assertEquals(null, out.error, out.toString())
            }
            assertEquals(listOf("slow", "interactive", "background"), started.toList())
        } finally {
            app.close()
            hub.close()
        }
    }

    /** 第 14 / 19 项：限流 / 大小上限配置与统计、工具注解 / outputSchema、结构化调用结果。 */
    @Test
    fun limitsAnnotationsAndStructuredResult() = runBlocking {
        // 非法：限流时 burst 须 ≥ 1
        assertFailsWith<FfiHubException.InvalidConfig> {
            Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false, limits = LimitsConfig(toolRateBurst = 0u)))
        }
        val hub = Hub.start(
            HubConfig(
                listen = "127.0.0.1:0",
                enableIpc = false,
                limits = LimitsConfig(toolRatePerMinute = 1u, toolRateBurst = 1u, maxArgumentsBytes = 64uL),
                outputValidation = OutputValidation.REJECT,
            ),
        )
        val app = AppMcp.create(
            AppMcpConfig("orders", "订单", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default),
        )
        val outputSchema = buildJsonObject {
            put("type", "object")
            putJsonObject("properties") { putJsonObject("orderId") { put("type", "string") } }
        }
        app.tool(
            "submit", "下单",
            annotations = dev.appmcp.ToolAnnotations(idempotentHint = false),
            outputSchema = outputSchema,
        ) { _, _ ->
            dev.appmcp.ToolResult(
                buildJsonObject { put("orderId", "o1") },
                status = dev.appmcp.ResultStatus.PENDING,
                stateResource = "order.state",
                summary = "已提交，等待付款",
                annotations = dev.appmcp.ContentAnnotations(priority = 0.5),
            )
        }
        app.tool("echo", "回显") { args, _ -> args }
        try {
            app.start()
            val tools = withTimeout(10.seconds) {
                var t = hub.tools(ToolFilter(apps = listOf("orders"), includeBuiltin = false))
                while (t.size < 2 || t.any { it.availability != Availability.AVAILABLE }) {
                    delay(20)
                    t = hub.tools(ToolFilter(apps = listOf("orders"), includeBuiltin = false))
                }
                t
            }
            val submit = tools.first { it.tool == "submit" }
            assertEquals(false, submit.annotations.idempotentHint)
            assertEquals(false, submit.annotations.readOnlyHint, "缺少的字段按 risk（write）推导")
            assertEquals(outputSchema, submit.outputSchema)
            assertEquals(null, tools.first { it.tool == "echo" }.outputSchema)

            val out = hub.callTool("orders.submit")
            assertEquals(null, out.error, out.toString())
            assertEquals(ResultStatus.PENDING, out.status)
            assertEquals("app-mcp://orders/order.state", out.stateResource)
            assertEquals("已提交，等待付款", out.summary)
            assertEquals(0.5, out.annotations?.priority)
            assertEquals("RATE_LIMITED", hub.callTool("orders.submit").error?.kind)
            val big = buildJsonObject { put("text", "x".repeat(100)) }
            assertEquals("PAYLOAD_TOO_LARGE", hub.callTool("orders.echo", big).error?.kind)
            val plain = hub.callTool("orders.echo", buildJsonObject { put("a", 1) })
            assertEquals(null, plain.error, plain.toString())
            assertEquals(ResultStatus.DONE, plain.status)

            val st = hub.status()
            assertEquals(1u, st.limits?.toolRatePerMinute)
            assertEquals(64uL, st.limits?.maxArgumentsBytes)
            assertEquals(600u, st.limits?.appRatePerMinute)
            assertEquals(OutputValidation.REJECT, st.outputValidation)
            val orders = st.apps.first { it.appId == "orders" }
            assertEquals(1uL to 1uL, orders.rateLimited to orders.tooLarge)
            val decl = orders.tools.first { it.name == "submit" }
            assertEquals(Risk.WRITE, decl.risk)
            assertEquals(false, decl.annotations?.idempotentHint)
            assertEquals(false, decl.effective.readOnlyHint)
            assertTrue(decl.outputSchema)
        } finally {
            app.close()
            hub.close()
        }
    }

    /** 第 16 项 O2：`callTool(onProgress = …)` 在返回前收到合并后的进度；资源内容标注经 Hub 列出。 */
    @Test
    fun progressCallbackAndResourceAnnotations() = runBlocking {
        val hub = Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false))
        val app = AppMcp.create(
            AppMcpConfig("work", "长任务", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default),
        )
        app.tool("run", "长任务", risk = AppRisk.READ) { _, ctx ->
            ctx.progress(1.0, 2.0, "第一步")
            delay(350) // 超过 Hub 默认合并间隔 250 ms
            ctx.progress(2.0, 2.0)
            delay(350)
            buildJsonObject { put("done", true) }
        }
        app.resource(
            "cart", "购物车",
            annotations = dev.appmcp.ContentAnnotations(audience = listOf(dev.appmcp.Audience.USER), priority = 0.5),
        ) { emptyMap<String, Any>() }
        try {
            app.start()
            withTimeout(10.seconds) {
                while (hub.tools(ToolFilter(apps = listOf("work"), includeBuiltin = false)).isEmpty() ||
                    hub.resources().none { it.appId == "work" }
                ) {
                    delay(20)
                }
            }
            val res = hub.resources().first { it.appId == "work" }
            assertEquals(0.5, res.annotations?.priority)
            assertEquals(listOf(Audience.USER), res.annotations?.audience)

            val got = Collections.synchronizedList(mutableListOf<ProgressUpdate>())
            val out = hub.callTool("work.run", onProgress = { got += it })
            assertEquals(null, out.error, out.toString())
            assertEquals(
                listOf(ProgressUpdate(1.0, 2.0, "第一步"), ProgressUpdate(2.0, 2.0, null)),
                got.toList(),
                "结果返回前收到全部进度",
            )
            val failing = hub.callTool("work.run", onProgress = { error("UI 崩溃") })
            assertEquals(null, failing.error, "回调异常不影响结果")
        } finally {
            app.close()
            hub.close()
        }
    }

    /** 策略挂点（spec/hub-api.md 3.13）：hide / deny、setPolicy 不合法时保留旧规则、命中计数。 */
    @Test
    fun policyHideDenyAndReplace() = runBlocking {
        assertFailsWith<FfiHubException.InvalidConfig> {
            Hub.start(
                HubConfig(
                    enableListen = false,
                    enableIpc = false,
                    policy = PolicyConfig(listOf(PolicyRule("bad id", PolicyAction.HIDE, "notes"))),
                ),
            )
        }
        val hub = Hub.start(
            HubConfig(
                listen = "127.0.0.1:0",
                enableIpc = false,
                policy = PolicyConfig(
                    listOf(
                        PolicyRule("hide-clear", PolicyAction.HIDE, "notes", tool = "clear"),
                        PolicyRule("deny-add", PolicyAction.DENY, "notes", tool = "add"),
                    ),
                ),
            ),
        )
        val app = AppMcp.create(
            AppMcpConfig("notes", "笔记", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default),
        )
        app.tool("add", "添加") { args, _ -> args }
        app.tool("clear", "清空") { _, _ -> buildJsonObject { put("cleared", true) } }
        app.tool("echo", "回显") { args, _ -> args }
        fun names() = hub.tools(ToolFilter(apps = listOf("notes"), includeBuiltin = false)).map { it.tool }.sorted()
        try {
            app.start()
            withTimeout(10.seconds) { while (names().size < 2) delay(20) }
            assertEquals(listOf("add", "echo"), names(), "hide 的工具不在列表中")

            assertEquals("TOOL_NOT_FOUND", hub.callTool("notes.clear").error?.kind)
            val denied = hub.callTool("notes.add", buildJsonObject { put("a", 1) }).error
            assertEquals("POLICY_DENIED", denied?.kind)
            assertEquals("deny-add", denied?.details?.jsonObject?.get("ruleId")?.jsonPrimitive?.content)
            assertEquals(null, hub.callTool("notes.echo", buildJsonObject { put("a", 1) }).error)
            assertEquals(listOf(1uL, 1uL), hub.policy().rules.map { it.hits })
            assertEquals(2, hub.status().policy?.rules?.size)

            // 不合法（hide 不能写 hooks）→ INVALID_INPUT，旧规则继续生效
            val e = assertFailsWith<FfiHubException.Tool> {
                hub.setPolicy(
                    PolicyConfig(listOf(PolicyRule("h", PolicyAction.HIDE, "notes", hooks = listOf(PolicyHook.CALL)))),
                )
            }
            assertEquals("INVALID_INPUT", e.kind)
            assertEquals("POLICY_DENIED", hub.callTool("notes.add", buildJsonObject { put("a", 1) }).error?.kind)
            assertTrue(hub.policy().lastError != null, "失败记入 lastError")

            // 按注解匹配的规则经 FFI 往返；替换后 add 恢复、echo 隐藏
            val byAnnotation = PolicyRule(
                "deny-destructive", PolicyAction.DENY, "*",
                annotations = AnnotationMatch(destructiveHint = true),
                hooks = listOf(PolicyHook.CALL, PolicyHook.WAKE),
            )
            hub.setPolicy(PolicyConfig(listOf(PolicyRule("hide-echo", PolicyAction.HIDE, "notes", tool = "echo"), byAnnotation)))
            assertEquals(listOf("add", "clear"), names())
            assertEquals(null, hub.callTool("notes.add", buildJsonObject { put("a", 1) }).error)
            val st = hub.policy()
            assertEquals(null, st.lastError, "成功加载后清除")
            assertEquals(byAnnotation, st.rules[1].rule)
        } finally {
            app.close()
            hub.close()
        }
    }

    @Test
    fun nativeAppOverIpc() = runBlocking {
        val windows = System.getProperty("os.name").lowercase().contains("windows")
        val pid = ProcessHandle.current().pid()
        val dir = java.nio.file.Files.createTempDirectory("app-mcp-kt-ipc")
        val endpoint = if (windows) "pipe:\\\\.\\pipe\\app-mcp-kt-test-$pid" else "unix:${dir.resolve("run/hub.sock")}"
        val hub = Hub.start(HubConfig(enableListen = false, ipcEndpoint = endpoint))
        assertEquals(endpoint, hub.ipcEndpoint)
        val app = AppMcp.create(AppMcpConfig("notes", "笔记", hostUrl = hub.ipcEndpoint!!, dispatcher = Dispatchers.Default))
        app.tool("add", "添加笔记", textSchema, risk = AppRisk.WRITE) { args, _ ->
            mapOf("saved" to args["text"]!!.jsonPrimitive.content)
        }
        try {
            app.start()
            val instance = withTimeout(10.seconds) {
                var found: InstanceInfo? = null
                while (found == null) {
                    found = hub.apps().firstOrNull { it.appId == "notes" }?.instances?.firstOrNull()
                    if (found == null) delay(20)
                }
                found
            }
            assertEquals(pid.toUInt(), instance.pid)
        } finally {
            app.close()
            hub.close()
            dir.toFile().deleteRecursively()
        }
    }
}
