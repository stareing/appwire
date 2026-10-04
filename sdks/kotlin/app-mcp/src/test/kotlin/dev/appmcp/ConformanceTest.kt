package dev.appmcp

import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.delay
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.jupiter.api.DynamicTest
import org.junit.jupiter.api.TestFactory
import kotlin.test.assertTrue

/**
 * 一致性用例 runner（Kotlin JVM）：按 `conformance/cases/` 下各用例 JSON 的 `app` 部分注册工具与资源，连接 fake_host
 * （`--case` 模式，核对在 fake_host 内完成）。格式与约定见 conformance/README.md；参照 Rust runner
 * crates/native/tests/it/conformance.rs。
 *
 * 只跑部分用例：`APP_MCP_CONFORMANCE_CASES=handshake,errors ./gradlew :app-mcp:test --tests dev.appmcp.ConformanceTest`。
 */
class ConformanceTest {
    private companion object {
        const val SDK = "kotlin"

        /** 本 runner 支持的用例能力（`requires`），见 conformance/README.md 第 4 节。 */
        val FEATURES = setOf(
            "toolOptions", "mutate", "lifecycle", "wake", "richResult", "userAction", "progress", "resourceOptions",
            "readFailure", "surface", "navigation", "backgroundTool", "backgroundNavigation", "idempotencyKey",
            "callScheduling", "busy", "events", "implements",
        )
        val VERDICT_OK = setOf("pass", "xfail", "xpass", "skip")
        val repoRoot: File = FakeHostSupport.repoRoot.canonicalFile
        val reportDir = File(repoRoot, "target/conformance")
    }

    @TestFactory
    fun conformanceCases(): List<DynamicTest> {
        val only = System.getenv("APP_MCP_CONFORMANCE_CASES")?.split(',')?.map { it.trim() }?.toSet()
        val cases = File(repoRoot, "conformance/cases").listFiles { f -> f.extension == "json" }.orEmpty()
            .filter { only == null || it.nameWithoutExtension in only }
            .sortedBy { it.name }
        assertTrue(cases.isNotEmpty(), "没有找到用例")
        return cases.map { path ->
            DynamicTest.dynamicTest(path.nameWithoutExtension) {
                val (verdict, exit) = runCase(FakeHostSupport.binary, path)
                val status = verdict["status"]?.jsonPrimitive?.contentOrNull ?: "error"
                println("[$SDK] ${path.nameWithoutExtension.padEnd(24)} $status")
                assertTrue(
                    status in VERDICT_OK && exit == 0,
                    "${path.nameWithoutExtension}: $status（退出码 $exit）\n${verdict["failures"]}\n" +
                        "详情见 ${File(reportDir, "$SDK/${path.nameWithoutExtension}.json")}",
                )
            }
        }
    }

    /** 跑一个用例，返回 fake_host 的结论行与退出码。 */
    private fun runCase(bin: File, path: File): Pair<JsonObject, Int> {
        val case = Json.parseToJsonElement(path.readText()).jsonObject
        val missing = case["requires"]?.jsonArray.orEmpty().map { it.jsonPrimitive.content }.filter { it !in FEATURES }
        val cmd = mutableListOf(bin.path, "--case", path.path, "--sdk", SDK, "--report-dir", reportDir.path)
        if (missing.isNotEmpty()) cmd += listOf("--skip", "runner 不支持：${missing.joinToString(", ")}")
        val host = ProcessBuilder(cmd).redirectError(ProcessBuilder.Redirect.INHERIT).start()
        var client: AppMcp? = null
        var verdict = JsonObject(emptyMap())
        try {
            host.inputStream.bufferedReader().lineSequence().forEach { line ->
                if (line.startsWith("LISTENING ")) {
                    client = startApp(line.removePrefix("LISTENING ").trim(), case)
                    return@forEach
                }
                val v = runCatching { Json.parseToJsonElement(line).jsonObject }.getOrNull() ?: return@forEach
                when (v.str("type")) {
                    "wake" -> client?.handleWake(v.str("arg").orEmpty())
                    "verdict" -> verdict = v
                }
            }
            host.waitFor(60, TimeUnit.SECONDS)
        } finally {
            client?.close()
            if (host.isAlive) host.destroy()
        }
        return verdict to host.waitFor()
    }

    private fun startApp(addr: String, case: JsonObject): AppMcp {
        val app = case.obj("app")
        val client = AppMcp.create(config(addr, app?.obj("config")))
        val tools = ConcurrentHashMap<String, ToolHandle>()
        app?.get("tools")?.jsonArray.orEmpty().forEach { registerTool(client, tools, it.jsonObject) }
        app?.get("resources")?.jsonArray.orEmpty().forEach { registerResource(client, it.jsonObject) }
        app?.get("events")?.jsonArray.orEmpty().forEach { declareEvent(client, it.jsonObject) }
        app?.obj("navigation")?.let { pages -> client.setNavigationHandler { page, params -> navigate(client, tools, pages, page, params) } }
        app?.str("visibility")?.let { client.setVisibility(Visibility.valueOf(enumName(it)), focused = false) }
        if (app?.bool("busy") == true) client.setBusy(true)
        return client.start()
    }

    /** `app.navigation`（conformance/README.md 2.4）。 */
    private fun navigate(
        client: AppMcp,
        tools: MutableMap<String, ToolHandle>,
        pages: JsonObject,
        page: String,
        params: JsonObject?,
    ): NavigationResult {
        val spec = pages.obj(page) ?: return NavigationResult.Failed("未知页面：$page")
        spec["mutate"]?.jsonArray.orEmpty().forEach { mutate(client, tools, it.jsonObject) }
        spec.str("throw")?.let { error(it) }
        spec.str("deny")?.let { return NavigationResult.Denied(it) }
        spec.str("fail")?.let { return NavigationResult.Failed(it) }
        // 与工具 handler 同一惯用法：抛 userActionRequired（封装层映射为 USER_ACTION_REQUIRED，不是 NAVIGATION_FAILED）
        spec.obj("userAction")?.let { throw ToolCallException.userActionRequired(it.str("message")!!, it.str("reason"), it.str("uri")) }
        if (spec.bool("failParams") == true) return NavigationResult.Failed(params?.toString().orEmpty())
        return NavigationResult.Ok
    }

    /** handler 的 `mutate`（conformance/README.md 2.3）：update 用补丁 API，null 清除。 */
    private fun mutate(client: AppMcp, tools: MutableMap<String, ToolHandle>, op: JsonObject) {
        val name = op.str("name").orEmpty()
        when (op.str("op")) {
            "register" -> registerTool(client, tools, op.obj("tool")!!)
            "busy" -> client.setBusy(op.bool("value")!!)
            "declareEvent" -> declareEvent(client, op.obj("event")!!)
            "removeEvent" -> client.removeEvent(name)
            "update" -> tools.getValue(name).update {
                op.obj("set").orEmpty().forEach { (key, v) -> setField(this, key, v) }
            }
            "remove" -> tools.remove(name)?.dispose()
            "enable" -> tools.getValue(name).setEnabled(true)
            "disable" -> tools.getValue(name).setEnabled(false)
            else -> error("未知的 mutate 操作 ${op.str("op")}")
        }
    }

    /** 事件声明（conformance/README.md 2.2）。 */
    private fun declareEvent(client: AppMcp, decl: JsonObject) =
        client.declareEvent(decl.str("name")!!, decl.str("description")!!, decl.obj("payloadSchema"))

    /** handler 的 `emit`：每项为 true / false（已发送 / 未连接丢弃），本地错误为 `"error"`。 */
    private fun emitEvents(client: AppMcp, items: List<JsonObject>): List<Any> = items.map {
        try {
            client.emitEvent(it.str("name")!!, it["payload"])
        } catch (_: dev.appmcp.ffi.AppMcpException) {
            "error"
        }
    }

    private fun setField(u: ToolUpdate, key: String, v: kotlinx.serialization.json.JsonElement) {
        val str = (v as? JsonPrimitive)?.contentOrNull
        when (key) {
            "description" -> u.description = str!!
            "inputSchema" -> u.inputSchema = v as? JsonObject
            "risk" -> u.risk = str?.let(::risk) ?: Risk.WRITE
            "title" -> u.title = str
            "annotations" -> u.annotations = (v as? JsonObject)?.let(::toolAnnotations)
            "outputSchema" -> u.outputSchema = v as? JsonObject
            "activation" -> u.activation = str?.let { Activation.valueOf(it.uppercase()) }
            "surface" -> u.surface = str?.let(::surface) ?: ToolSurface.APP
            "page" -> u.page = str
            "backgroundTool" -> u.backgroundTool = str
            "concurrency" -> u.concurrency = str?.toIntOrNull() ?: 0
            "exclusive" -> u.exclusive = str
            "implements" -> u.implements = strings(v) // 封装层空列表 = 清除
            else -> error("未知的工具字段 $key")
        }
    }

    private fun config(addr: String, c: JsonObject?): AppMcpConfig {
        val hostUrl = if (':' in addr && !addr.startsWith("unix:") && !addr.startsWith("pipe:")) "ws://$addr/app" else addr
        val lc = c?.obj("lifecycle")
        val lifecycle = lc?.let {
            val d = LifecyclePolicy()
            LifecyclePolicy(
                mode = it.str("mode")?.let(::lifecycleMode) ?: d.mode,
                idleTimeoutMillis = it.long("idleTimeoutMs") ?: d.idleTimeoutMillis,
                graceMillis = it.long("graceMs") ?: d.graceMillis,
                mergeWindowMillis = it.long("mergeWindowMs") ?: d.mergeWindowMillis,
            )
        }
        val dedup = c?.obj("callDedup")?.let {
            val d = CallDedupPolicy()
            CallDedupPolicy(
                ttlMs = it.long("ttlMs")?.toULong() ?: d.ttlMs,
                maxEntries = it.long("maxEntries")?.toUInt() ?: d.maxEntries,
            )
        }
        return AppMcpConfig(
            "conf", "Conformance",
            hostUrl = hostUrl,
            lifecycle = lifecycle,
            callDedup = dedup,
            maxConcurrentCalls = c?.long("maxConcurrentCalls")?.toInt() ?: 1,
            navigateInBackground = c?.bool("navigateInBackground"),
            maxQueuedCalls = c?.long("maxQueuedCalls")?.toInt(),
            busyPolicy = c?.str("busyPolicy")?.let { BusyPolicy.valueOf(enumName(it)) },
        )
    }

    private fun registerTool(client: AppMcp, tools: MutableMap<String, ToolHandle>, decl: JsonObject) {
        val handler = decl.obj("handler") ?: JsonObject(emptyMap())
        val runs = AtomicLong(0)
        tools[decl.str("name")!!] = client.tool(
            name = decl.str("name")!!,
            description = decl.str("description")!!,
            inputSchema = decl.obj("inputSchema"),
            risk = decl.str("risk")?.let(::risk) ?: Risk.WRITE, // 封装层 risk 不可为空，缺省即 WRITE
            activation = decl.str("activation")?.let { Activation.valueOf(it.uppercase()) },
            title = decl.str("title"),
            enabled = decl["enabled"]?.jsonPrimitive?.booleanOrNull ?: true,
            annotations = decl.obj("annotations")?.let(::toolAnnotations),
            outputSchema = decl.obj("outputSchema"),
            surface = decl.str("surface")?.let(::surface) ?: ToolSurface.APP,
            page = decl.str("page"),
            backgroundTool = decl.str("backgroundTool"),
            concurrency = decl.long("concurrency")?.toInt() ?: 0,
            exclusive = decl.str("exclusive"),
            implements = strings(decl["implements"]),
        ) { args, ctx -> runHandler(client, tools, handler, runs.incrementAndGet(), args, ctx) }
    }

    /** 顺序：progress → delayMs → mutate → emit → 结果（conformance/README.md 2.1）。 */
    private suspend fun runHandler(
        client: AppMcp,
        tools: MutableMap<String, ToolHandle>,
        spec: JsonObject,
        count: Long,
        args: JsonObject,
        ctx: ToolContext,
    ): Any? {
        spec["progress"]?.jsonArray.orEmpty().map { it.jsonObject }.forEach {
            ctx.progress(it.double("progress") ?: 0.0, it.double("total"), it.str("message"))
        }
        spec.long("delayMs")?.let { delay(it) } // 取消 / 超时时协程被取消
        spec["mutate"]?.jsonArray.orEmpty().forEach { mutate(client, tools, it.jsonObject) }
        val emitted = spec["emit"]?.jsonArray?.let { list -> emitEvents(client, list.map { it.jsonObject }) }
        spec.str("throw")?.let { error(it) }
        spec.obj("userAction")?.let { throw ToolCallException.userActionRequired(it.str("message")!!, it.str("reason"), it.str("uri")) }
        spec.obj("result")?.let { r ->
            return ToolResult(
                data = r["data"],
                stateHints = r["stateHints"]?.jsonArray.orEmpty().map { it.jsonPrimitive.content },
                status = r.str("status")?.let { ResultStatus.valueOf(it.uppercase()) } ?: ResultStatus.DONE,
                stateResource = r.str("stateResource"),
                summary = r.str("summary"),
                annotations = r.obj("annotations")?.let(::contentAnnotations),
            )
        }
        if ("return" in spec) return spec["return"]
        if (spec.bool("echo") == true) return args
        if (spec.bool("returnIdempotencyKey") == true) {
            return JsonObject(mapOf("idempotencyKey" to JsonPrimitive(ctx.idempotencyKey)))
        }
        if (spec.bool("counter") == true) return mapOf("count" to count)
        if (emitted != null) return mapOf("emitted" to emitted)
        // returnNothing（以及未声明结果）：Kotlin 的"无返回值"即返回 Unit。
        return Unit
    }

    private fun registerResource(client: AppMcp, decl: JsonObject) {
        val read = decl.obj("read") ?: JsonObject(emptyMap())
        client.resource(
            name = decl.str("name")!!,
            description = decl.str("description")!!,
            mimeType = decl.str("mimeType"),
            realtime = decl.bool("realtime") ?: false,
            annotations = decl.obj("annotations")?.let(::contentAnnotations),
        ) {
            if ("return" in read) return@resource read["return"]
            read.obj("fail")?.let {
                throw ToolCallException(it.str("kind") ?: ErrorKind.HANDLER_ERROR, it.str("message")!!, it["details"])
            }
            read.obj("userAction")?.let {
                throw ToolCallException.userActionRequired(it.str("message")!!, it.str("reason"), it.str("uri"))
            }
            error(read.str("throw") ?: "读取失败")
        }
    }

    private fun toolAnnotations(a: JsonObject) = ToolAnnotations(
        title = a.str("title"),
        readOnlyHint = a.bool("readOnlyHint"),
        destructiveHint = a.bool("destructiveHint"),
        idempotentHint = a.bool("idempotentHint"),
        openWorldHint = a.bool("openWorldHint"),
    )

    private fun contentAnnotations(a: JsonObject) = ContentAnnotations(
        audience = a["audience"]?.jsonArray?.map { Audience.valueOf(it.jsonPrimitive.content.uppercase()) },
        priority = a.double("priority"),
        lastModified = a.str("lastModified"),
    )

    /** 协议取值（kebab-case）→ 生成的枚举名。 */
    private fun enumName(value: String) = value.uppercase().replace('-', '_')
    private fun risk(value: String) = Risk.valueOf(enumName(value))
    private fun lifecycleMode(value: String) = LifecycleMode.valueOf(enumName(value))
    private fun surface(value: String) = ToolSurface.valueOf(enumName(value))

    /** 字符串数组；null / 缺省为空列表。 */
    private fun strings(v: kotlinx.serialization.json.JsonElement?): List<String> =
        (v as? kotlinx.serialization.json.JsonArray).orEmpty().map { it.jsonPrimitive.content }

    private fun JsonObject.obj(key: String): JsonObject? = (this[key] as? JsonObject)
    private fun JsonObject.prim(key: String): JsonPrimitive? = (this[key] as? JsonPrimitive)?.takeIf { it !is JsonNull }
    private fun JsonObject.str(key: String): String? = prim(key)?.contentOrNull
    private fun JsonObject.long(key: String): Long? = prim(key)?.longOrNull
    private fun JsonObject.double(key: String): Double? = prim(key)?.doubleOrNull
    private fun JsonObject.bool(key: String): Boolean? = prim(key)?.booleanOrNull
}
