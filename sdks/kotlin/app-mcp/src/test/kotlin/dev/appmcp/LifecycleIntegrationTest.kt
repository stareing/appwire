package dev.appmcp

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import java.io.BufferedReader
import java.io.File
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.test.fail
import org.junit.jupiter.api.Assumptions.assumeTrue

/**
 * 生命周期往返（spec/lifecycle.md）：idle 休眠 → 唤醒令牌 → handleWake 回连（toolsCurrent，跳过 sync）→
 * 调用（含 failWithDetails 的 details）→ 再次休眠。需要 cargo 与 fake_host。
 */
class LifecycleIntegrationTest {
    private val repoRoot = File(System.getProperty("appmcp.repoRoot") ?: "../../..")
    private val targetDir = File(System.getenv("CARGO_TARGET_DIR") ?: File("../../../target").canonicalPath)

    private fun fakeHost(): File {
        System.getenv("APP_MCP_FAKE_HOST")?.let { return File(it) }
        val build = runCatching {
            ProcessBuilder("cargo", "build", "-q", "-p", "app-mcp-native", "--example", "fake_host")
                .directory(repoRoot).redirectErrorStream(true).start()
        }.getOrNull()
        assumeTrue(build != null, "没有 cargo，跳过集成测试")
        val log = build!!.inputStream.bufferedReader().readText()
        assumeTrue(build.waitFor() == 0, "fake_host 构建失败：$log")
        return File(targetDir, "debug/examples/fake_host")
    }

    private fun BufferedReader.nextJson(): JsonObject {
        val line = readLine() ?: fail("fake_host 提前结束输出")
        return Json.parseToJsonElement(line).jsonObject
    }

    private val JsonObject.type get() = this["type"]!!.jsonPrimitive.content

    /**
     * 回归（魅族 18 Pro 复测，TASKS.md 4e）：唤醒后等待再次休眠由状态回调驱动，不按 100 ms 轮询原生状态。
     * 唤醒后调用持有 3 s 才结束：旧实现在线期间检查约 30 次以上，现在只在状态变化时检查。
     */
    @Test
    fun awaitSleepAfterWakeDoesNotPoll() = runBlocking {
        val host = ProcessBuilder(
            fakeHost().path,
            "--await-sleep", "--wake", "--invoke", "job.run", "--await-sleep", "--timeout-ms", "20000",
        ).redirectError(ProcessBuilder.Redirect.INHERIT).start()
        val out = host.inputStream.bufferedReader()
        val first = out.readLine() ?: fail("fake_host 没有输出")
        assertTrue(first.startsWith("LISTENING "), first)
        val client = AppMcp.create(
            AppMcpConfig(
                "kotlin-poll", "Kotlin 唤醒等待测试",
                hostUrl = "ws://${first.removePrefix("LISTENING ").trim()}",
                lifecycle = LifecyclePolicy(
                    mode = LifecycleMode.IDLE,
                    idleTimeoutMillis = 300,
                    wake = WakeDescriptor(WakeKind.URI, "kotlin-poll", true),
                ),
                connectTimeoutMillis = 2_000,
            ),
        )
        client.tool("job.run", "后台任务") { _, ctx ->
            val hold = ctx.hold()
            CoroutineScope(Dispatchers.Default).launch {
                delay(3_000)
                hold.close()
            }
            "started"
        }
        try {
            client.start()
            var wake: JsonObject
            do {
                wake = out.nextJson()
            } while (wake.type != "wake")
            assertTrue(client.awaitState(StateStatus.DORMANT, 5_000), "未进入 DORMANT")
            val before = client.awaitSleepChecks.get()
            val started = System.nanoTime()
            val outcome = client.handleWakeAndAwaitSleep(wake["arg"]!!.jsonPrimitive.content, timeoutMillis = 15_000)
            val elapsedMs = (System.nanoTime() - started) / 1_000_000
            assertEquals(WakeOutcome.SLEPT, outcome)
            assertTrue(elapsedMs >= 3_000, "应等到持有结束后的休眠：$elapsedMs ms")
            val checks = client.awaitSleepChecks.get() - before
            assertTrue(checks <= 15, "等待期间检查了 $checks 次原生状态（应只在状态变化时检查）")
            assertTrue(host.waitFor(10, TimeUnit.SECONDS), "fake_host 未退出")
        } finally {
            client.close()
            host.destroy()
        }
    }

    @Test
    fun idleSleepWakeInvokeSleep() = runBlocking {
        val host = ProcessBuilder(
            fakeHost().path,
            "--await-sleep", "--wake",
            "--invoke", "cart.add", "--args", """{"qty":3}""",
            "--invoke", "cart.add", "--args", """{"qty":-1}""",
            "--await-sleep",
            "--timeout-ms", "20000",
        ).redirectError(ProcessBuilder.Redirect.INHERIT).start()
        val out = host.inputStream.bufferedReader()
        val first = out.readLine() ?: fail("fake_host 没有输出")
        assertTrue(first.startsWith("LISTENING "), first)
        val addr = first.removePrefix("LISTENING ").trim()

        var idleExit = 0
        val client = AppMcp.create(
            AppMcpConfig(
                "kotlin-life", "Kotlin 生命周期测试",
                hostUrl = "ws://$addr",
                lifecycle = LifecyclePolicy(
                    mode = LifecycleMode.IDLE,
                    idleTimeoutMillis = 300,
                    wake = WakeDescriptor(WakeKind.URI, "kotlin-life", true),
                ),
                connectTimeoutMillis = 2_000,
                onIdleExit = { idleExit++ },
            ),
        )
        var holdReleased = false
        client.tool("cart.add", "加入购物车") { args, ctx ->
            val qty = args["qty"]!!.jsonPrimitive.int
            if (qty <= 0) {
                throw ToolCallException(
                    ErrorKind.INVALID_INPUT, "数量必须为正",
                    buildJsonObject { put("field", "qty"); put("min", 1) },
                )
            }
            // handler 发起的后台任务：持有到任务结束，期间不休眠。
            val hold = ctx.hold()
            CoroutineScope(Dispatchers.Default).launch {
                delay(200)
                hold.close()
                holdReleased = true
            }
            mapOf("qty" to qty)
        }
        try {
            client.start()
            assertEquals("tools", out.nextJson().type)
            val sleep1 = out.nextJson()
            assertEquals("sleep", sleep1.type)
            assertTrue(sleep1["accepted"]!!.jsonPrimitive.boolean)
            assertEquals("idle", sleep1["reason"]!!.jsonPrimitive.content)
            assertEquals(client.toolsHash, sleep1["toolsHash"]!!.jsonPrimitive.content)
            assertTrue(client.awaitState(StateStatus.DORMANT, 5_000), "未进入 DORMANT")

            val wake = out.nextJson()
            assertEquals("wake", wake.type)
            val arg = wake["arg"]!!.jsonPrimitive.content
            assertFalse(client.handleWake("--not-a-wake"))
            val outcome = async(Dispatchers.Default) { client.handleWakeAndAwaitSleep(arg, timeoutMillis = 15_000) }

            val hello = out.nextJson()
            assertEquals("hello", hello.type)
            assertEquals(wake["token"], hello["launchToken"])
            assertEquals("os-activation", hello["wakeReason"]!!.jsonPrimitive.content)
            assertTrue(hello["toolsCurrent"]!!.jsonPrimitive.boolean, hello.toString())
            val tools = out.nextJson()
            assertEquals("tools", tools.type)
            assertFalse(tools["synced"]!!.jsonPrimitive.boolean, "toolsCurrent 时不应重新 tools/sync")
            assertEquals(listOf("cart.add"), tools["tools"]!!.jsonArray.map { it.jsonPrimitive.content })

            val ok = out.nextJson()
            assertEquals(3, ok["result"]!!.jsonObject["data"]!!.jsonObject["qty"]!!.jsonPrimitive.int)
            val err = out.nextJson()["error"]!!.jsonObject
            val data = err["data"]!!.jsonObject
            assertEquals("INVALID_INPUT", data["kind"]!!.jsonPrimitive.content)
            assertEquals("qty", data["field"]!!.jsonPrimitive.content, err.toString())
            assertEquals(1, data["min"]!!.jsonPrimitive.int)

            val sleep2 = out.nextJson()
            assertEquals("sleep", sleep2.type)
            assertTrue(sleep2["accepted"]!!.jsonPrimitive.boolean)
            assertTrue(holdReleased, "持有释放前不应休眠")

            assertEquals(WakeOutcome.SLEPT, outcome.await())
            assertEquals(StateStatus.DORMANT, client.currentState().status)
            assertTrue(host.waitFor(10, TimeUnit.SECONDS), "fake_host 未退出")
            assertEquals(0, host.exitValue())
            assertEquals(0, idleExit, "residency=keep 不应回调 onIdleExit")

            // App 主动 hold 期间 sleep() 仍然有效（sleep 不受持有影响），hold 可重复释放。
            client.hold().use { it.release() }
        } finally {
            client.close()
            client.close() // 幂等
            host.destroy()
        }
        assertFalse(client.handleWake("app-mcp-wake:abc"), "关闭后 handleWake 返回 false")
        assertEquals(StateStatus.STOPPED, client.currentState().status)
    }
}
