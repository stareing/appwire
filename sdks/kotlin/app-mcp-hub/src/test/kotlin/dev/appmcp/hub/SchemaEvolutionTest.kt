package dev.appmcp.hub

import dev.appmcp.AppMcp
import dev.appmcp.AppMcpConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import dev.appmcp.Deprecation as AppDeprecation

/**
 * 工具演进（第 16 项 O4，spec/hub-api.md 3.21）：真实 App 声明弃用工具 → [HubTool.deprecated] 原样、`schemaHash` 有值；
 * schema 变化后 `schemaHash` 变化，破坏性变化记入 `status().schemaChanges`。
 */
class SchemaEvolutionTest {
    private fun schema(required: Boolean) = buildJsonObject {
        put("type", "object")
        putJsonObject("properties") { putJsonObject("q") { put("type", "string") } }
        if (required) putJsonArray("required") { add(kotlinx.serialization.json.JsonPrimitive("q")) }
    }

    private suspend fun Hub.awaitTool(name: String, pred: (HubTool) -> Boolean = { true }): HubTool = withTimeout(10.seconds) {
        var found: HubTool? = null
        while (found == null) {
            found = tools().firstOrNull { it.name == name && pred(it) }
            if (found == null) delay(20)
        }
        found
    }

    @Test
    fun deprecatedToolAndSchemaHash() = runBlocking {
        Hub.start(HubConfig(listen = "127.0.0.1:0", enableIpc = false)).use { hub ->
            val cfg = AppMcpConfig("lib", "Lib", hostUrl = "ws://${hub.listenAddr}/app", dispatcher = Dispatchers.Default)
            AppMcp.create(cfg).use { app ->
                val dep = AppDeprecation("改用 q.new", replacement = "q.new", until = "2027-06-30")
                val old = app.tool("q.old", "旧版查询", schema(required = false), deprecated = dep) { _, _ -> 1 }
                app.tool("q.new", "新版查询") { _, _ -> 2 }
                app.start()

                val t = hub.awaitTool("lib.q.old")
                assertEquals(Deprecation("改用 q.new", replacement = "q.new", until = "2027-06-30"), t.deprecated)
                val hash = assertNotNull(t.schemaHash)
                assertEquals(16, hash.length, hash)
                val fresh = hub.awaitTool("lib.q.new")
                assertNull(fresh.deprecated, "未弃用")
                assertNotNull(fresh.schemaHash)
                val builtin = hub.awaitTool("apps.list")
                assertNull(builtin.schemaHash, "内置工具不带")
                assertNull(builtin.deprecated)

                old.update(inputSchema = schema(required = true))
                val changed = hub.awaitTool("lib.q.old") { it.schemaHash != hash }
                assertNotEquals(hash, changed.schemaHash)
                assertEquals(t.deprecated, changed.deprecated, "更新 schema 不丢弃用声明")
                val rec = hub.status().schemaChanges.orEmpty().firstOrNull { it.appId == "lib" && it.tool == "q.old" }
                assertNotNull(rec, "记入 schemaChanges")
                assertEquals(ChangeLevel.BREAKING, rec.level)
                assertTrue(rec.changes.isNotEmpty() && rec.at > 0u, rec.toString())
            }
        }
    }
}
