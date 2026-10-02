package dev.appmcp

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

/** 导航表、工具补丁更新与 surface / page（spec/protocol.md 3.4）；与 Host 的往返由 ConformanceTest 覆盖。 */
class NavigationTest {
    private val client = AppMcp.create(AppMcpConfig("kotlin-nav", "Kotlin 导航测试", hostUrl = "ws://127.0.0.1:9"))

    @AfterTest
    fun tearDown() = client.close()

    @Test
    fun pageRouterDispatchesByName() = runBlocking {
        val seen = mutableListOf<String>()
        val router = PageRouter()
            .page("cart") { seen += "cart:$it" }
            .pageWithResult("login") { NavigationResult.Denied("需要先登录") }
        val params = JsonObject(mapOf("id" to JsonPrimitive(7)))
        assertEquals(NavigationResult.Ok, router("cart", params))
        assertEquals(listOf("""cart:{"id":7}"""), seen)
        assertEquals(NavigationResult.Denied("需要先登录"), router("login", null))
        assertEquals(NavigationResult.Failed("未知页面：nowhere"), router("nowhere", null))
        assertEquals(setOf("cart", "login"), router.pageNames)
    }

    @Test
    fun fillRouteSubstitutesParams() {
        val params = JsonObject(mapOf("id" to JsonPrimitive("a b/c"), "n" to JsonPrimitive(3)))
        assertEquals("orders/a%20b%2Fc?n=3", fillRoute("orders/{id}?n={n}", params))
        assertEquals("cart", fillRoute("cart", null))
        assertFailsWith<IllegalArgumentException> { fillRoute("orders/{id}", null) }
    }

    @Test
    fun setNavigationHandlerAcceptsRouterAndNull() {
        val router = PageRouter().page("cart") {}
        client.setNavigationHandler(router::invoke)
        client.setNavigationHandler(null)
    }

    @Test
    fun toolUpdatePatchClearsOnlyAssignedFields() {
        val base = dev.appmcp.ffi.ToolSpec(
            name = "t", description = "旧", title = "标题", page = "cart", surface = ToolSurface.VIEW,
            annotations = ToolAnnotations(readOnlyHint = true, title = null, destructiveHint = null, idempotentHint = null, openWorldHint = null),
            outputSchemaJson = """{"type":"object"}""",
        )
        val next = ToolUpdate().apply {
            description = "新"
            annotations = null
            outputSchema = null
        }.applyTo(base)
        assertEquals("新", next.description)
        assertNull(next.annotations)
        assertNull(next.outputSchemaJson)
        // 未赋值的保持不变
        assertEquals("标题", next.title)
        assertEquals("cart", next.page)
        assertEquals(ToolSurface.VIEW, next.surface)
        assertFailsWith<IllegalStateException> { ToolUpdate().title }
    }

    @Test
    fun toolDeclaresSurfaceAndPage() {
        val handle = client.tool("v.tool", "依赖界面", surface = ToolSurface.VIEW, page = "cart") { _, _ -> null }
        handle.update { page = null; surface = ToolSurface.APP }
        handle.update(description = "旧式更新：只改描述")
        handle.dispose()
    }

    @Test
    fun updateKeepsEnabledState() {
        val handle = client.tool("v.toggle", "随可见性切换", surface = ToolSurface.VIEW) { _, _ -> null }
        handle.setEnabled(false)
        handle.update { title = "新标题" }
        assertEquals(false, handle.specForTest().enabled)
        handle.dispose()
    }
}
