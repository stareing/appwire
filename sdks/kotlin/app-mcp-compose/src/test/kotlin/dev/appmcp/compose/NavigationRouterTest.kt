package dev.appmcp.compose

import androidx.navigation.NavHostController
import androidx.navigation.compose.ComposeNavigator
import androidx.navigation.compose.composable
import androidx.navigation.createGraph
import androidx.test.core.app.ApplicationProvider
import dev.appmcp.NavigationResult
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** 页面名 → Navigation Compose 路由（不加载原生库：只测导航表本身）。 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class NavigationRouterTest {
    private fun controller() = NavHostController(ApplicationProvider.getApplicationContext()).apply {
        navigatorProvider.addNavigator(ComposeNavigator())
        graph = createGraph(startDestination = "home") {
            composable("home") {}
            composable("cart") {}
            composable("orders/{id}") {}
        }
    }

    @Test
    fun navigatesToMappedRoutes() = runBlocking {
        val nav = controller()
        val router = nav.navigationRouter(mapOf("cart" to "cart", "orders.detail" to "orders/{id}"))
        assertEquals(NavigationResult.Ok, router("cart", null))
        assertEquals("cart", nav.currentDestination?.route)
        // launchSingleTop：已在目标页面时不重复压栈
        val depth = nav.currentBackStack.value.size
        router("cart", null)
        assertEquals(depth, nav.currentBackStack.value.size)
        assertEquals(NavigationResult.Ok, router("orders.detail", JsonObject(mapOf("id" to JsonPrimitive("o1")))))
        assertEquals("orders/{id}", nav.currentDestination?.route)
        assertEquals("o1", nav.currentBackStackEntry?.arguments?.getString("id"))
    }

    @Test
    fun unknownPageAndMissingParamFail() = runBlocking {
        val router = controller().navigationRouter(mapOf("orders.detail" to "orders/{id}"))
        assertTrue(router("nowhere", null) is NavigationResult.Failed)
        val missing = runCatching { router("orders.detail", null) }
        assertTrue(missing.exceptionOrNull() is IllegalArgumentException)
    }
}
