package dev.appmcp

import dev.appmcp.ffi.AppMcpException
import kotlinx.coroutines.Dispatchers
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertSame
import kotlin.test.assertTrue

class UnitTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-unit", "Kotlin 单元测试", hostUrl = "ws://127.0.0.1:9"))

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun errorKindsMatchNative() {
        val declared = ErrorKind::class.java.declaredFields
            .filter { java.lang.reflect.Modifier.isStatic(it.modifiers) && it.type == String::class.java }
            .map { it.get(null) as String }
            .toSet()
        assertEquals(ErrorKind.all, declared)
    }

    @Test
    fun anyToJsonConversions() {
        assertEquals(JsonNull, anyToJson(null))
        assertEquals(JsonNull, anyToJson(Unit))
        assertEquals("""{"a":[1,"x",true]}""", anyToJson(mapOf("a" to listOf(1, "x", true))).toString())
        assertFailsWith<IllegalArgumentException> { anyToJson(Any()) }
    }

    @Test
    fun defaultDispatcherOnJvmIsDefault() {
        assertSame(Dispatchers.Default, AppMcp.defaultDispatcher())
    }

    @Test
    fun registrationErrors() {
        val schema = buildJsonObject { put("type", "object") }
        val h = client.tool("cart.add", "加入购物车", schema) { _, _ -> JsonPrimitive(1) }
        assertEquals("cart.add", h.name)
        assertFailsWith<AppMcpException.DuplicateName> { client.tool("cart.add", "重复") { _, _ -> null } }
        assertFailsWith<AppMcpException.InvalidName> { client.tool("bad name!", "非法") { _, _ -> null } }
        h.setEnabled(false)
        h.update(description = "新描述")
        h.dispose()
        client.tool("cart.add", "注销后可重新注册") { _, _ -> null }

        client.scope("page").use { s -> s.tool("page.one", "页内工具") { _, _ -> null } }
        client.tool("page.one", "scope 注销后可重新注册") { _, _ -> null }

        val r = client.resource("cart", "购物车") { mapOf("items" to emptyList<String>()) }
        r.notifyChanged()
        assertEquals(StateStatus.IDLE, client.state.value.status)
        assertTrue(client.instanceId.isNotEmpty())
    }

    @Test
    fun stoppedClientRejectsRegistration() {
        val c = AppMcp.create(AppMcpConfig("kotlin-unit", "停止", overview = AppOverview("测试")))
        c.stop()
        assertFailsWith<AppMcpException.Stopped> { c.tool("x", "x") { _, _ -> null } }
        c.close()
    }

    @Test
    fun lifecyclePolicyMapsToFfi() {
        val d = LifecyclePolicy().toFfi()
        assertEquals(LifecycleMode.PERSISTENT, d.mode)
        assertEquals(60_000uL, d.idleTimeoutMs)
        assertEquals(15_000uL, d.hiddenIdleTimeoutMs)
        val p = LifecyclePolicy(
            mode = LifecycleMode.IDLE, hiddenIdleTimeoutMillis = -5, residency = Residency.EXIT_WHEN_IDLE,
            wake = WakeDescriptor(WakeKind.ANDROID_INTENT, "pkg/dev.appmcp.android.WakeReceiver", true),
        ).toFfi()
        assertEquals(0uL, p.hiddenIdleTimeoutMs)
        assertEquals(Residency.EXIT_WHEN_IDLE, p.residency)
        assertEquals(WakeKind.ANDROID_INTENT, p.wake?.kind)
    }

    @Test
    fun powerSwitchesMapToFfi() {
        val d = LifecyclePolicy().toFfi()
        assertEquals(3u, d.hostAbsentRetries)
        assertEquals(false, d.legacyTimers)
        assertEquals(2_000uL, d.mergeWindowMs)
        assertEquals(false, d.sleepOnBackground)
        val p = LifecyclePolicy(
            hostAbsentRetries = 0, legacyTimers = true, mergeWindowMillis = 500, sleepOnBackground = true,
        ).toFfi()
        assertEquals(0u, p.hostAbsentRetries) // 0 = 一直重连，与 uniffi 编码相同
        assertEquals(true, p.legacyTimers)
        assertEquals(500uL, p.mergeWindowMs)
        assertEquals(true, p.sleepOnBackground)
        val clamped = LifecyclePolicy(hostAbsentRetries = -1, mergeWindowMillis = -1).toFfi()
        assertEquals(0u, clamped.hostAbsentRetries)
        assertEquals(0uL, clamped.mergeWindowMs)
    }

    @Test
    fun callDedupMapsToFfi() {
        assertEquals(null, AppMcpConfig("kotlin-unit", "去重").toFfi().callDedup)
        val d = CallDedupPolicy()
        assertEquals(300_000uL, d.ttlMs)
        assertEquals(64u, d.maxEntries)
        val off = CallDedupPolicy(0u, 0u)
        assertEquals(off, AppMcpConfig("kotlin-unit", "去重", callDedup = off).toFfi().callDedup)
        AppMcp.create(AppMcpConfig("kotlin-unit", "去重关", hostUrl = "ws://127.0.0.1:9", callDedup = off)).close()
    }

    @Test
    fun registerNameMapsToFfi() {
        val plain = AppMcpConfig("kotlin-unit", "按名").toFfi()
        assertEquals(false, plain.registerName)
        assertEquals(null, plain.nameInstance)
        val named = AppMcpConfig("kotlin-unit", "按名", registerName = true, nameInstance = "w2").toFfi()
        assertEquals(true, named.registerName)
        assertEquals("w2", named.nameInstance)
        AppMcp.create(AppMcpConfig("kotlin-unit", "按名", hostUrl = "ws://127.0.0.1:9", registerName = true, nameInstance = "w2"))
            .close()
        for (bad in listOf("default", "W2", "2w")) {
            assertFailsWith<AppMcpException.InvalidConfig>(bad) {
                AppMcp.create(AppMcpConfig("kotlin-unit", "按名", hostUrl = "ws://127.0.0.1:9", nameInstance = bad))
            }
        }
    }

    @Test
    fun heartbeatMapsToFfi() {
        assertEquals(HeartbeatMode.AUTO, AppMcpConfig("kotlin-unit", "心跳").toFfi().heartbeat)
        for (mode in HeartbeatMode.entries) {
            assertEquals(mode, AppMcpConfig("kotlin-unit", "心跳", heartbeat = mode).toFfi().heartbeat)
        }
        assertEquals(null, AppMcpConfig("kotlin-unit", "无策略").toFfi().lifecycle)
        val c = AppMcp.create(AppMcpConfig("kotlin-unit", "心跳关", hostUrl = "ws://127.0.0.1:9", heartbeat = HeartbeatMode.OFF))
        c.close()
    }

    @Test
    fun realtimeResourceChangesToolsHash() {
        fun hashWith(register: (AppMcp) -> Unit): String {
            val c = AppMcp.create(AppMcpConfig("kotlin-unit", "摘要", hostUrl = "ws://127.0.0.1:9"))
            try {
                register(c)
                return c.toolsHash
            } finally {
                c.close()
            }
        }
        val plain = hashWith { it.resource("order", "订单") { null } }
        val explicitFalse = hashWith { it.resource("order", "订单", realtime = false) { null } }
        val realtime = hashWith { it.resource("order", "订单", realtime = true) { null } }
        assertEquals(plain, explicitFalse)
        assertTrue(plain != realtime)
        // 资源内容标注随同步上报（计入摘要），未声明时不变
        val annotated = hashWith {
            it.resource("order", "订单", annotations = ContentAnnotations(audience = listOf(Audience.USER), priority = 0.5)) { null }
        }
        assertTrue(plain != annotated)
        assertEquals(plain, hashWith { it.resource("order", "订单", annotations = null) { null } })
        client.scope("s").use { it.resource("s.order", "订单", realtime = true) { null } }
    }

    @Test
    fun wakeTokenParsing() {
        assertEquals("abc", parseWakeToken("app-mcp-wake:abc"))
        assertEquals("t1", parseWakeToken("shop://app-mcp/wake?token=t1"))
        assertEquals(null, parseWakeToken("--help"))
    }

    @Test
    fun lifecycleApiWithoutHost() {
        val c = AppMcp.create(
            AppMcpConfig(
                "kotlin-unit", "按需", hostUrl = "ws://127.0.0.1:9",
                lifecycle = LifecyclePolicy(mode = LifecycleMode.ON_DEMAND), connectTimeoutMillis = 500,
            ),
        )
        assertEquals(16, c.toolsHash.length)
        val before = c.toolsHash
        c.tool("x.y", "改变摘要") { _, _ -> null }
        assertTrue(before != c.toolsHash)
        assertTrue(!c.handleWake("not a wake"))
        val h = c.hold()
        h.close()
        h.close()
        assertTrue(h.isReleased)
        c.close()
        c.close()
        assertTrue(c.isClosed)
        assertTrue(!c.wake() && !c.connectNow() && !c.sleep())
    }

    @Test
    fun toolCallExceptionDetails() {
        val e = ToolCallException(ErrorKind.INVALID_INPUT, "坏", JsonPrimitive(1))
        assertEquals(JsonPrimitive(1), e.details)
        assertEquals(null, ToolCallException(ErrorKind.INVALID_INPUT, "坏").details)
    }

    @Test
    fun userActionRequiredDetails() {
        val e = ToolCallException.userActionRequired("请先登录", UserActionReason.LOGIN, "shop://login")
        assertEquals(ErrorKind.USER_ACTION_REQUIRED, e.kind)
        assertEquals("请先登录", e.message)
        assertEquals(
            JsonObject(mapOf("reason" to JsonPrimitive("login"), "uri" to JsonPrimitive("shop://login"))),
            e.details,
        )
        assertEquals(JsonObject(mapOf("reason" to JsonPrimitive("custom"))), ToolCallException.userActionRequired("x", "custom").details)
        assertEquals(null, ToolCallException.userActionRequired("切到前台").details)
        assertTrue(ErrorKind.POLICY_DENIED in ErrorKind.all && ErrorKind.USER_ACTION_REQUIRED in ErrorKind.all)
    }

    @Test
    fun callSchedulingOptionsReachSpec() {
        val plain = client.tool("s.plain", "缺省") { _, _ -> null }
        assertEquals(0u, plain.specForTest().concurrency)
        assertEquals(null, plain.specForTest().exclusive)
        val handle = client.tool("s.edit", "改文档", concurrency = 2, exclusive = "doc") { _, _ -> null }
        assertEquals(2u, handle.specForTest().concurrency)
        assertEquals("doc", handle.specForTest().exclusive)
        handle.update(description = "新")
        assertEquals("doc", handle.specForTest().exclusive, "补丁型 update 不得重置调度声明")
        handle.update { concurrency = 0; exclusive = null }
        assertEquals(0u, handle.specForTest().concurrency)
        assertEquals(null, handle.specForTest().exclusive)
        handle.update(concurrency = 1, exclusive = "doc2")
        assertEquals(1u, handle.specForTest().concurrency)
        assertEquals("doc2", handle.specForTest().exclusive)
        val typed = client.typedTool<Unit, Unit>("s.typed", "带类型", concurrency = 3, exclusive = "doc") { _, _ -> }
        assertEquals(3u, typed.specForTest().concurrency)
        assertFailsWith<AppMcpException.InvalidName> { client.tool("s.bad", "非法组名", exclusive = "bad group") { _, _ -> null } }
    }

    @Test
    fun implementsReachSpec() {
        assertEquals(emptyList(), client.tool("i.plain", "缺省") { _, _ -> null }.specForTest().implements)
        val handle = client.tool("i.send", "发消息", implements = listOf("message.send@1")) { _, _ -> null }
        assertEquals(listOf("message.send@1"), handle.specForTest().implements)
        handle.update(description = "新")
        handle.setEnabled(false)
        assertEquals(listOf("message.send@1"), handle.specForTest().implements, "补丁型 update 不得重置意图声明")
        handle.update { implements = emptyList() }
        assertEquals(emptyList(), handle.specForTest().implements)
        handle.update(implements = listOf("link.open@1"))
        assertEquals(listOf("link.open@1"), handle.specForTest().implements)
        val typed = client.typedTool<Unit, Unit>("i.typed", "带类型", implements = listOf("link.open@1")) { _, _ -> }
        assertEquals(listOf("link.open@1"), typed.specForTest().implements)
        assertFailsWith<AppMcpException.InvalidName> {
            client.tool("i.bad", "缺版本", implements = listOf("message.send")) { _, _ -> null }
        }
        assertFailsWith<AppMcpException.InvalidName> { handle.update(implements = listOf("Bad Verb@1")) }
    }

    @Test
    fun maxQueuedCallsMapsToFfi() {
        assertEquals(null, AppMcpConfig("kotlin-unit", "排队").toFfi().maxQueuedCalls, "为空时交给核心缺省（64）")
        assertEquals(0u, AppMcpConfig("kotlin-unit", "排队", maxQueuedCalls = 0).toFfi().maxQueuedCalls)
        assertEquals(8u, AppMcpConfig("kotlin-unit", "排队", maxQueuedCalls = 8).toFfi().maxQueuedCalls)
        AppMcp.create(AppMcpConfig("kotlin-unit", "排队", hostUrl = "ws://127.0.0.1:9", maxQueuedCalls = 1)).close()
    }
}
