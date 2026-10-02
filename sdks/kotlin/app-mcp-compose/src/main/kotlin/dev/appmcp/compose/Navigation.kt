package dev.appmcp.compose

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.navigation.NavController
import androidx.navigation.NavOptionsBuilder
import dev.appmcp.AppMcp
import dev.appmcp.PageRouter
import dev.appmcp.fillRoute
import kotlinx.serialization.json.JsonObject

/**
 * Navigation Compose 适配：页面名 → 路由。Host 的 `app/navigate`（spec/protocol.md 3.4）到达时在主线程
 * `navigate(route)`；路由模板中的 `{参数}` 用页面参数填充（[fillRoute]，缺少参数时导航失败）。
 *
 * ```kotlin
 * val router = navController.navigationRouter(mapOf("cart" to "cart", "orders.detail" to "orders/{id}"))
 * client.setNavigationHandler(router::invoke)   // 或 NavigationHandlerEffect(client, navController, routes)
 * ```
 *
 * @input routes 页面名（清单 `pages[].name` / 工具的 `page`）→ 路由模板
 * @input navOptions 导航选项；缺省 `launchSingleTop = true`（已在目标页面时不重复压栈）
 */
fun NavController.navigationRouter(
    routes: Map<String, String>,
    navOptions: NavOptionsBuilder.() -> Unit = { launchSingleTop = true },
): PageRouter = routes.entries.fold(PageRouter()) { router, (page, template) ->
    router.page(page) { params: JsonObject? -> navigate(fillRoute(template, params), navOptions) }
}

/**
 * 组合期间把 [navController] 设为 [client] 的导航回调，离开组合时清除。
 *
 * 能力在握手时声明（spec/protocol.md 3.4）：首次连接前进入组合最好；否则在下次连接（回连 / 唤醒）时生效。
 */
@Composable
fun NavigationHandlerEffect(
    client: AppMcp,
    navController: NavController,
    routes: Map<String, String>,
    navOptions: NavOptionsBuilder.() -> Unit = { launchSingleTop = true },
) {
    DisposableEffect(client, navController, routes) {
        val router = navController.navigationRouter(routes, navOptions)
        client.setNavigationHandler(router::invoke)
        onDispose { client.setNavigationHandler(null) }
    }
}
