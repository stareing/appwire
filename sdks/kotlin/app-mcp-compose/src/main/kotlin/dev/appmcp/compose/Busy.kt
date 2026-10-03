package dev.appmcp.compose

import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import dev.appmcp.AppMcp

/**
 * 用户正在操作（spec/protocol.md 5.3）：[active] 为 true 且在组合中时持有一段忙碌作用域（[AppMcp.beginBusy]），
 * 变为 false 或离开组合时归还。多处同时使用按引用计数合并，最后一处结束才恢复空闲。
 *
 * ```kotlin
 * var editing by remember { mutableStateOf(false) }
 * BusyEffect(client, active = editing)
 * TextField(..., modifier = Modifier.onFocusChanged { editing = it.isFocused })
 * ```
 */
@Composable
fun BusyEffect(client: AppMcp, active: Boolean) {
    BusyScopeEffect(client, active, client::beginBusy)
}

/** [BusyEffect] 的实现（不依赖原生库，便于测试）：[active] 时调用 [begin]，失效或离开组合时关闭其返回值。 */
@Composable
internal fun BusyScopeEffect(key: Any, active: Boolean, begin: () -> AutoCloseable) {
    DisposableEffect(key, active) {
        val hold = if (active) begin() else null
        onDispose { hold?.close() }
    }
}
