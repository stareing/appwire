package dev.appmcp.android

import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import dev.appmcp.ToolHandle

/**
 * 把 `view` 工具（spec/protocol.md 3.4）的启用状态绑定到生命周期：所在界面至少处于 [minState]（默认 `RESUMED`，
 * 即可见且在最上层可交互）时启用，低于它时禁用（禁用的工具不同步给 Host）；`DESTROYED` 时注销（[disposeOnDestroy]）。
 *
 * ```kotlin
 * // Fragment：用 viewLifecycleOwner，视图销毁即注销
 * override fun onViewCreated(view: View, savedInstanceState: Bundle?) {
 *     client.tool("cart.checkout", "结算当前购物车", surface = ToolSurface.VIEW, page = "cart", enabled = false) { _, _ -> … }
 *         .enableWhile(viewLifecycleOwner)
 * }
 * ```
 *
 * Compose 用 `dev.appmcp.compose.ViewToolEffect`（`LifecycleResumeEffect`）。
 *
 * @side-effect 在 [owner] 的生命周期上登记观察者；返回值 `close()` 移除观察者（不改变工具当前状态）。
 * @invariant 必须在主线程调用（`Lifecycle.addObserver` 的要求）。
 */
@JvmOverloads
fun ToolHandle.enableWhile(
    owner: LifecycleOwner,
    minState: Lifecycle.State = Lifecycle.State.RESUMED,
    disposeOnDestroy: Boolean = true,
): AutoCloseable = bindEnabled(owner.lifecycle, minState, ::setEnabled) {
    if (disposeOnDestroy) dispose() else setEnabled(false)
}

/**
 * [enableWhile] 的实现：与具体工具无关（单元测试用假的启用 / 注销函数）。
 *
 * @input onDestroy `DESTROYED` 时调用一次（观察者已移除）
 */
internal fun bindEnabled(
    lifecycle: Lifecycle,
    minState: Lifecycle.State,
    setEnabled: (Boolean) -> Unit,
    onDestroy: () -> Unit,
): AutoCloseable {
    require(minState != Lifecycle.State.DESTROYED) { "minState 不能为 DESTROYED" }
    val observer = object : LifecycleEventObserver {
        private var enabled: Boolean? = null

        override fun onStateChanged(source: LifecycleOwner, event: Lifecycle.Event) {
            if (event == Lifecycle.Event.ON_DESTROY) {
                lifecycle.removeObserver(this)
                onDestroy()
                return
            }
            val want = lifecycle.currentState.isAtLeast(minState)
            if (enabled == want) return
            enabled = want
            setEnabled(want)
        }
    }
    lifecycle.addObserver(observer)
    return AutoCloseable { lifecycle.removeObserver(observer) }
}
