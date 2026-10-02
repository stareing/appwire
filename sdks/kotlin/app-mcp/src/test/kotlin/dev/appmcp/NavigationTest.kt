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
    fun thrownErrorsMapToNavigationResults() {
        val ua = ToolCallException.userActionRequired("请点开通知", UserActionReason.FOREGROUND, "shop://cart")
        assertEquals(
            NavigationResult.UserActionRequired("请点开通知", "foreground", "shop://cart"),
            navigationFailure(ua.kind, ua.message, ua.details),
        )
        assertEquals(
            NavigationResult.UserActionRequired("只有说明"),
            navigationFailure(ErrorKind.USER_ACTION_REQUIRED, "只有说明", null),
        )
        assertEquals(NavigationResult.Denied("不行"), navigationFailure(ErrorKind.NAVIGATION_DENIED, "不行", null))
        assertEquals(NavigationResult.Failed("坏了"), navigationFailure(ErrorKind.HANDLER_ERROR, "坏了", null))
    }

    @Test
    fun navigateInBackgroundConfigAndSetter() {
        AppMcp.create(AppMcpConfig("kotlin-nav-bg", "后台导航", hostUrl = "ws://127.0.0.1:9", navigateInBackground = true))
            .use { it.setNavigateInBackground(false) }
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
            name = "t", description = "旧", title = "标题", page = "cart", surface = ToolSurface.VIEW, backgroundTool = "t.bg",
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
        assertEquals("t.bg", next.backgroundTool)
        assertNull(ToolUpdate().apply { backgroundTool = null }.applyTo(base).backgroundTool)
        assertFailsWith<IllegalStateException> { ToolUpdate().title }
    }

    @Test
    fun toolDeclaresSurfaceAndPage() {
        val handle = client.tool("v.tool", "依赖界面", surface = ToolSurface.VIEW, page = "cart", backgroundTool = "v.bg") { _, _ -> null }
        assertEquals("v.bg", handle.specForTest().backgroundTool)
        handle.update(backgroundTool = "v.bg2")
        assertEquals("v.bg2", handle.specForTest().backgroundTool)
        handle.update { page = null; surface = ToolSurface.APP; backgroundTool = null }
        assertNull(handle.specForTest().backgroundTool)
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
