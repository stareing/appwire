package dev.appmcp

import kotlinx.coroutines.delay
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.double
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.io.File
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.test.fail
import org.junit.jupiter.api.Assumptions.assumeTrue

@Serializable
data class Greeting(val name: String)

@Serializable
data class GreetingReply(val text: String)

/** 启动 crates/native 的 fake_host，经真实 WebSocket 驱动工具调用。需要 cargo。 */
class IntegrationTest {
    private val repoRoot = File(System.getProperty("appmcp.repoRoot") ?: "../../..")
    private val targetDir = File(System.getenv("CARGO_TARGET_DIR") ?: File("../../../target").canonicalPath)

    private fun fakeHost(): File {
        System.getenv("APP_MCP_FAKE_HOST")?.let { return File(it) }
        val build = runCatching {
            ProcessBuilder("cargo", "build", "-q", "-p", "app-mcp-native", "--example", "fake_host")
                .directory(repoRoot)
                .redirectErrorStream(true)
                .start()
        }.getOrNull()
        assumeTrue(build != null, "没有 cargo，跳过集成测试")
        val log = build!!.inputStream.bufferedReader().readText()
        assumeTrue(build.waitFor() == 0, "fake_host 构建失败：$log")
        return File(targetDir, "debug/examples/fake_host")
    }

    @Test
    fun invokeToolsViaFakeHost() {
        val host = ProcessBuilder(
            fakeHost().path,
            "--invoke", "math.add", "--args", """{"a":2,"b":40}""",
            "--invoke", "greet", "--args", """{"name":"世界"}""",
            "--invoke", "cart.checkout",
            "--invoke", "boom",
            "--invoke", "account.login",
            "--invoke", "app.foreground",
            "--read", "cart",
            "--read", "session",
            "--read", "quota",
            "--timeout-ms", "15000",
        ).redirectError(ProcessBuilder.Redirect.INHERIT).start()
        val out = host.inputStream.bufferedReader()
        val first = out.readLine() ?: fail("fake_host 没有输出")
        assertTrue(first.startsWith("LISTENING "), first)
        val addr = first.removePrefix("LISTENING ").trim()

        val errorLogs = java.util.concurrent.CopyOnWriteArrayList<String>()
        val client = AppMcp.create(
            AppMcpConfig(
                "kotlin-it", "Kotlin 集成测试", hostUrl = "ws://$addr", callDedup = CallDedupPolicy(ttlMs = 1_000u),
                onLog = { level, msg -> if (level == LogLevel.ERROR) errorLogs += msg },
            ),
        )
        val threads = mutableListOf<String>()
        client.tool("math.add", "两数相加", risk = Risk.READ) { args, ctx ->
            threads += Thread.currentThread().name
            ctx.addStateHint("cart")
            ctx.progress(1.0, 2.0, "相加")
            delay(5)
            mapOf("sum" to args["a"]!!.jsonPrimitive.int + args["b"]!!.jsonPrimitive.int)
        }
        client.typedTool<Greeting, GreetingReply>("greet", "问候") { g, _ -> GreetingReply("你好，${g.name}") }
        client.tool("cart.checkout", "结账", risk = Risk.PAYMENT) { _, _ ->
            throw ToolCallException(ErrorKind.USER_REJECTED, "用户取消了结账")
        }
        client.tool("boom", "抛异常") { _, _ -> error("炸了") }
        client.tool("account.login", "需登录") { _, _ ->
            throw ToolCallException.userActionRequired("登录已过期", UserActionReason.LOGIN, "shop://login")
        }
        client.tool("app.foreground", "需前台") { _, _ -> throw ToolCallException.userActionRequired("请切到前台") }
        client.resource("cart", "购物车") { mapOf("items" to listOf("A")) }
        client.resource("session", "会话") {
            throw ToolCallException.userActionRequired("登录已过期", UserActionReason.LOGIN, "shop://login")
        }
        client.resource("quota", "额度") {
            throw ToolCallException(ErrorKind.USER_REJECTED, "额度不足", JsonObject(mapOf("quota" to JsonPrimitive(0))))
        }

        val lines = try {
            client.start()
            val text = out.readText()
            assertTrue(host.waitFor(30, TimeUnit.SECONDS), "fake_host 未退出")
            assertEquals(0, host.exitValue(), "fake_host 退出码非 0，输出：\n$text")
            text.lines().filter { it.isNotBlank() }.map { Json.parseToJsonElement(it).jsonObject }
        } finally {
            client.close()
            host.destroy()
        }

        assertEquals("tools", lines[0]["type"]!!.jsonPrimitive.content)
        val tools = lines[0]["tools"]!!.jsonArray.map { it.jsonPrimitive.content }.toSet()
        assertTrue(tools.containsAll(listOf("math.add", "greet", "cart.checkout", "boom")), tools.toString())

        val progress = lines.filter { it["type"]!!.jsonPrimitive.content == "progress" }
        assertEquals(1, progress.size, progress.toString())
        assertEquals(1.0, progress[0]["progress"]!!.jsonPrimitive.double)
        assertEquals(2.0, progress[0]["total"]!!.jsonPrimitive.double)
        assertEquals("相加", progress[0]["message"]!!.jsonPrimitive.content)
        val results = lines.drop(1).filter { "name" in it }.associateBy { it["name"]!!.jsonPrimitive.content }
        val add = results["math.add"]!!["result"]!!.jsonObject
        assertEquals(42, add["data"]!!.jsonObject["sum"]!!.jsonPrimitive.int)
        assertEquals(listOf("cart"), add["stateHints"]!!.jsonArray.map { it.jsonPrimitive.content })
        assertEquals(
            "你好，世界",
            results["greet"]!!["result"]!!.jsonObject["data"]!!.jsonObject["text"]!!.jsonPrimitive.content,
        )
        val rejected = results["cart.checkout"]!!["error"]!!.jsonObject
        assertEquals("USER_REJECTED", rejected["data"]!!.jsonObject["kind"]!!.jsonPrimitive.content)
        assertEquals("用户取消了结账", rejected["message"]!!.jsonPrimitive.content)
        val boom = results["boom"]!!["error"]!!.jsonObject
        assertEquals("HANDLER_ERROR", boom["data"]!!.jsonObject["kind"]!!.jsonPrimitive.content)
        // 未预期异常：设备端日志带工具名与堆栈，回复给 Host 的消息不带堆栈
        assertEquals("炸了", boom["message"]!!.jsonPrimitive.content)
        val boomLog = errorLogs.single { "工具 boom" in it }
        assertTrue("IllegalStateException: 炸了" in boomLog && "\tat " in boomLog, boomLog)
        // ToolCallException（业务错误）不记 ERROR 日志
        assertTrue(errorLogs.none { "cart.checkout" in it || "account.login" in it || "session" in it }, errorLogs.toString())
        val login = results["account.login"]!!["error"]!!.jsonObject
        assertEquals("登录已过期", login["message"]!!.jsonPrimitive.content)
        assertEquals(
            JsonObject(
                mapOf(
                    "kind" to JsonPrimitive("USER_ACTION_REQUIRED"),
                    "reason" to JsonPrimitive("login"),
                    "uri" to JsonPrimitive("shop://login"),
                ),
            ),
            login["data"],
        )
        assertEquals(
            JsonObject(mapOf("kind" to JsonPrimitive("USER_ACTION_REQUIRED"))),
            results["app.foreground"]!!["error"]!!.jsonObject["data"],
        )
        assertEquals(
            JsonObject(mapOf("items" to kotlinx.serialization.json.JsonArray(listOf(JsonPrimitive("A"))))),
            results["cart"]!!["result"]!!.jsonObject["contents"],
        )
        // 资源读取失败的类别与详情原样到达 Host
        assertEquals(login["data"], results["session"]!!["error"]!!.jsonObject["data"])
        assertEquals(
            JsonObject(mapOf("kind" to JsonPrimitive("USER_REJECTED"), "quota" to JsonPrimitive(0))),
            results["quota"]!!["error"]!!.jsonObject["data"],
        )
        assertTrue(threads.isNotEmpty() && threads[0].startsWith("DefaultDispatcher"), threads.toString())
    }

    /** 工具注解 + outputSchema 到达 Host；结构化结果（pending + stateResource + summary + 内容标注）原样回给 Host。 */
    @Test
    fun toolOptionsAndStructuredResultReachHost() {
        val host = ProcessBuilder(
            fakeHost().path,
            "--tool-info",
            "--invoke", "order.submit",
            "--invoke", "plain",
            "--timeout-ms", "15000",
        ).redirectError(ProcessBuilder.Redirect.INHERIT).start()
        val out = host.inputStream.bufferedReader()
        val first = out.readLine() ?: fail("fake_host 没有输出")
        val addr = first.removePrefix("LISTENING ").trim()

        val client = AppMcp.create(AppMcpConfig("kotlin-it", "Kotlin 集成测试", hostUrl = "ws://$addr"))
        val outputSchema = Json.parseToJsonElement(
            """{"type":"object","properties":{"orderId":{"type":"string"}}}""",
        ).jsonObject
        client.tool(
            "order.submit", "下单",
            annotations = ToolAnnotations(idempotentHint = false, openWorldHint = true),
            outputSchema = outputSchema,
        ) { _, _ ->
            ToolResult(
                JsonObject(mapOf("orderId" to JsonPrimitive("o1"))),
                status = ResultStatus.PENDING,
                stateResource = "order.state",
                summary = "已提交，等待用户在 App 内付款",
                annotations = ContentAnnotations(audience = listOf(Audience.USER), priority = 0.5),
            )
        }
        client.tool("plain", "普通返回值", risk = Risk.READ) { _, _ -> mapOf("ok" to true) }

        val lines = try {
            client.start()
            val text = out.readText()
            assertTrue(host.waitFor(30, TimeUnit.SECONDS), "fake_host 未退出")
            assertEquals(0, host.exitValue(), "fake_host 退出码非 0，输出：\n$text")
            text.lines().filter { it.isNotBlank() }.map { Json.parseToJsonElement(it).jsonObject }
        } finally {
            client.close()
            host.destroy()
        }

        val info = lines[0]["toolInfo"]!!.jsonObject
        assertEquals(
            Json.parseToJsonElement(
                """{"risk":"write","annotations":{"idempotentHint":false,"openWorldHint":true},"outputSchema":$outputSchema}""",
            ),
            info["order.submit"],
        )
        assertEquals(Json.parseToJsonElement("""{"risk":"read"}"""), info["plain"])

        val results = lines.drop(1).associateBy { it["name"]!!.jsonPrimitive.content }
        assertEquals(
            Json.parseToJsonElement(
                """{"data":{"orderId":"o1"},"status":"pending","stateResource":"order.state",
                   "summary":"已提交，等待用户在 App 内付款","annotations":{"audience":["user"],"priority":0.5}}""",
            ),
            results["order.submit"]!!["result"],
        )
        // 普通返回值：只有 data（status 缺省 done 不序列化）
        assertEquals(Json.parseToJsonElement("""{"data":{"ok":true}}"""), results["plain"]!!["result"])
    }
}
