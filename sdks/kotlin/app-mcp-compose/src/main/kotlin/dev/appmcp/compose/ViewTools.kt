package dev.appmcp.compose

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import dev.appmcp.AppMcpRegistrar
import dev.appmcp.Risk
import dev.appmcp.ToolAnnotations
import dev.appmcp.ToolFunction
import dev.appmcp.ToolHandle
import dev.appmcp.ToolSurface
import kotlinx.serialization.json.JsonObject

/**
 * `view` 工具（spec/protocol.md 3.4）只在所在界面 RESUMED（可见且在最上层可交互）时启用：`LifecycleResumeEffect`
 * 恢复时 `setEnabled(true)`，暂停或离开组合时 `setEnabled(false)`。
 *
 * Navigation Compose 中每个目的地有自己的 `LocalLifecycleOwner`（NavBackStackEntry），切到其他页面即暂停；
 * 弹窗（Dialog）不改变下层目的地的生命周期——需要压制时传 `enabled = !dialogOpen`（不要直接 `setEnabled`：
 * 界面恢复时会被本效果重新启用）。
 *
 * @input enabled App 侧条件（如没有弹窗压在上面）；实际启用 = RESUMED 且 [enabled]
 */
@Composable
fun ViewToolEffect(
    handle: ToolHandle,
    lifecycleOwner: LifecycleOwner = LocalLifecycleOwner.current,
    enabled: Boolean = true,
) {
    ViewEnabledEffect(handle, lifecycleOwner, enabled, handle::setEnabled)
}

/** [ViewToolEffect] 的实现（不依赖原生库，便于测试）：RESUMED 且 [enabled] 时 `setEnabled(true)`，否则 false。 */
@Composable
internal fun ViewEnabledEffect(
    key: Any,
    lifecycleOwner: LifecycleOwner,
    enabled: Boolean,
    setEnabled: (Boolean) -> Unit,
) {
    val apply by rememberUpdatedState(setEnabled)
    LifecycleResumeEffect(key, enabled, lifecycleOwner = lifecycleOwner) {
        apply(enabled)
        onPauseOrDispose { apply(false) }
    }
}

/**
 * 在组合中注册 `view` 工具：进入组合时注册（禁用状态），所在界面 RESUMED 时启用（[ViewToolEffect]），离开组合时注销。
 * [handler] 取最新一次组合传入的 lambda（可直接捕获界面状态）。
 *
 * ```kotlin
 * composable("cart") {
 *     rememberViewTool(client, "cart.checkout", "结算当前购物车", page = "cart") { _, _ -> viewModel.checkout() }
 *     CartScreen()
 * }
 * ```
 *
 * @input name / description / inputSchema 等同 [AppMcpRegistrar.tool]；名称或注册入口变化时重新注册
 * @input enabled 见 [ViewToolEffect]（如弹窗打开时传 false 压制下层工具）
 * @input backgroundTool 后台替身：同 App 内一个 `APP` 工具的名称，App 在后台时 Hub 改调它（spec/protocol.md 3.4「后台与前台」）
 * @input concurrency / exclusive SDK 内的调用调度（spec/protocol.md 5.3），等同 [AppMcpRegistrar.tool]
 */
@Composable
fun rememberViewTool(
    registrar: AppMcpRegistrar,
    name: String,
    description: String,
    inputSchema: JsonObject? = null,
    page: String? = null,
    risk: Risk = Risk.WRITE,
    title: String? = null,
    annotations: ToolAnnotations? = null,
    outputSchema: JsonObject? = null,
    backgroundTool: String? = null,
    concurrency: Int = 0,
    exclusive: String? = null,
    lifecycleOwner: LifecycleOwner = LocalLifecycleOwner.current,
    enabled: Boolean = true,
    handler: ToolFunction,
): ToolHandle {
    val current by rememberUpdatedState(handler)
    val handle = remember(registrar, name) {
        registrar.tool(
            name, description, inputSchema, risk,
            title = title,
            enabled = false,
            annotations = annotations,
            outputSchema = outputSchema,
            surface = ToolSurface.VIEW,
            page = page,
            backgroundTool = backgroundTool,
            concurrency = concurrency,
            exclusive = exclusive,
        ) { args, ctx -> current(args, ctx) }
    }
    DisposableEffect(handle) {
        onDispose { handle.dispose() }
    }
    ViewToolEffect(handle, lifecycleOwner, enabled)
    return handle
}
