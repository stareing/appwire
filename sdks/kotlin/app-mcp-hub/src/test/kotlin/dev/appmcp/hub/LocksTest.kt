package dev.appmcp.hub

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds

/** 对象锁（spec/hub-api.md 3.6「对象锁」）：HubConfig.maxLocks、apps.lock / apps.unlock、HubStatus.locks、LOCKED。 */
class LocksTest {
    private val shop =
        """{"manifestVersion":1,"appId":"shop","name":"商城",
           "tools":[{"name":"cart.add","description":"加购","inputSchema":{"type":"object"}}]}"""

    private fun start(maxLocks: UInt? = null): Hub =
        Hub.start(HubConfig(enableListen = false, enableIpc = false, manifestsJson = listOf(shop), maxLocks = maxLocks))

    private fun lockArgs() = buildJsonObject { put("appId", "shop") }

    /** 缺省列出 apps.lock / apps.unlock；会话 s1 加锁后 s2 加同一把锁 → LOCKED（holder = "api"）；status().locks 列出。 */
    @Test
    fun lockConflictAndStatus() = runBlocking {
        assertEquals(null, HubConfig().maxLocks)
        start().use { hub ->
            val names = hub.tools().map { it.name }
            assertTrue(names.containsAll(listOf("apps.lock", "apps.unlock")), names.toString())

            val args = buildJsonObject { put("appId", "shop"); put("ttlMs", 30000) }
            val ok = hub.callTool("apps.lock", args, session = "s1", timeout = 5.seconds)
            assertEquals(null, ok.error, ok.toString())
            val denied = hub.callTool("apps.lock", lockArgs(), session = "s2", timeout = 5.seconds)
            assertEquals("LOCKED", denied.error?.kind, denied.toString())
            val details = Json.parseToJsonElement(assertNotNull(denied.error?.detailsJson)).jsonObject
            assertEquals("api", details["holder"]?.jsonPrimitive?.content)

            val lock: LockStatus = assertNotNull(hub.status().locks).single()
            assertEquals(listOf("shop", null, "api:s1", "api"), listOf(lock.appId, lock.key, lock.caller, lock.holder))
            assertTrue(lock.expiresInMs in 1uL..30000uL, lock.toString())

            val released = hub.callTool("apps.unlock", lockArgs(), session = "s1", timeout = 5.seconds)
            assertTrue(assertNotNull(released.data).jsonObject["released"]!!.jsonPrimitive.boolean, released.toString())
            assertEquals(emptyList(), hub.status().locks)
        }
    }

    /** maxLocks = 0 关闭对象锁：不列出，调用为 TOOL_NOT_FOUND。 */
    @Test
    fun maxLocksZeroDisables() = runBlocking {
        start(0u).use { hub ->
            val names = hub.tools().map { it.name }
            assertFalse("apps.lock" in names || "apps.unlock" in names, names.toString())
            val kind = try {
                hub.callTool("apps.lock", lockArgs(), session = "s1", timeout = 5.seconds).error?.kind
            } catch (e: HubException) {
                (e as? dev.appmcp.hub.ffi.HubException.Tool)?.kind
            }
            assertEquals("TOOL_NOT_FOUND", kind)
        }
    }
}
