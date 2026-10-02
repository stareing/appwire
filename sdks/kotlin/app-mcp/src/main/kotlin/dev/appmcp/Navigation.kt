package dev.appmcp

import kotlinx.serialization.json.JsonObject

/** 工具对界面的依赖（spec/protocol.md 3.4）：`APP`（缺省）后台可调、可唤醒；`VIEW` 只在所在界面可见且处于最上层时启用。 */
typealias ToolSurface = dev.appmcp.ffi.ToolSurface

/**
 * 导航回调的结果（Host 的 `app/navigate`，spec/protocol.md 3.4）。
 *
 * - [Ok]：已切换到目标页面（最好在新页面的工具注册之后再返回，Host 会等待目标工具出现）。
 * - [Denied]：不愿切换（用户正在输入、页面需要登录等）→ `NAVIGATION_DENIED`；[message] 面向模型 / 用户。
 * - [Failed]：页面不存在、参数不合法等 → `NAVIGATION_FAILED`（`reason: "error"`）。
 */
sealed class NavigationResult {
    data object Ok : NavigationResult()
    data class Denied(val message: String) : NavigationResult()
    data class Failed(val message: String) : NavigationResult()
}

/**
 * 导航回调：在配置的调度器上（Android 默认 `Dispatchers.Main`）以协程执行。
 *
 * @input page 目标页面名（清单 `pages[].name` 或工具声明的 `page`）
 * @input params 页面参数；Host 没有给出时为 null
 * @error 抛出的异常按 [NavigationResult.Failed] 回复；[ToolCallException]（`kind` 为 [ErrorKind.NAVIGATION_DENIED]）按拒绝回复
 */
typealias NavigateFunction = suspend (page: String, params: JsonObject?) -> NavigationResult

/**
 * 按页面名分派的导航表：各框架适配（Navigation Compose、Fragment 事务等）只需登记"页面 → 怎样切过去"。
 *
 * ```kotlin
 * client.setNavigationHandler(PageRouter()
 *     .page("cart") { navController.navigate("cart") }
 *     .page("orders.detail") { params -> navController.navigate("orders/${params?.get("id")}") })
 * ```
 *
 * 未登记的页面以 [NavigationResult.Failed] 回复；页面函数正常返回 = [NavigationResult.Ok]，也可直接返回其他结果。
 */
class PageRouter {
    private val pages = java.util.concurrent.ConcurrentHashMap<String, suspend (JsonObject?) -> NavigationResult>()

    /** 登记页面；同名覆盖。 */
    fun page(name: String, navigate: suspend (params: JsonObject?) -> Unit): PageRouter = apply {
        pages[name] = { params -> navigate(params); NavigationResult.Ok }
    }

    /** 登记页面，由函数决定结果（如未登录时返回 [NavigationResult.Denied]）。 */
    fun pageWithResult(name: String, navigate: suspend (params: JsonObject?) -> NavigationResult): PageRouter = apply {
        pages[name] = navigate
    }

    /** 已登记的页面名。 */
    val pageNames: Set<String> get() = pages.keys.toSet()

    /** 作为 [AppMcp.setNavigationHandler] 的回调。 */
    suspend operator fun invoke(page: String, params: JsonObject?): NavigationResult =
        pages[page]?.invoke(params) ?: NavigationResult.Failed("未知页面：$page")
}

private val ROUTE_PARAM = Regex("""\{([A-Za-z0-9_]+)}""")

/**
 * 用页面参数填充路由模板（Navigation Compose 等以字符串路由导航的框架共用）：`orders/{id}` + `{"id":"o1"}` → `orders/o1`。
 * 字符串取原文、其他值取 JSON 文本，均按 URL 编码（空格为 `%20`）。
 *
 * @error 模板引用了参数中没有的键时抛 [IllegalArgumentException]（导航回调据此以失败回复）
 */
fun fillRoute(template: String, params: JsonObject?): String = ROUTE_PARAM.replace(template) { m ->
    val key = m.groupValues[1]
    val value = params?.get(key) ?: throw IllegalArgumentException("缺少页面参数：$key")
    val text = (value as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content ?: value.toString()
    java.net.URLEncoder.encode(text, Charsets.UTF_8).replace("+", "%20")
}

/**
 * [ToolHandle.update] 的补丁：只改赋值过的字段；赋值 null 清除该声明（与网页 / Rust SDK 一致），未赋值的保持不变。
 *
 * ```kotlin
 * handle.update {
 *     description = "新描述"
 *     annotations = null   // 清除
 * }
 * ```
 */
class ToolUpdate internal constructor() {
    private val values = HashMap<String, Any?>()
    private val patches = LinkedHashMap<String, (FfiToolSpec) -> FfiToolSpec>()

    var description: String by field { s, v -> s.copy(description = v) }
    /** null = 清除（无参数）。 */
    var inputSchema: JsonObject? by field { s, v -> s.copy(inputSchemaJson = v?.toString()) }
    var risk: Risk by field { s, v -> s.copy(risk = v) }
    /** null = 清除。 */
    var title: String? by field { s, v -> s.copy(title = v) }
    /** null = 清除（Hub 按 [risk] 推导）。 */
    var annotations: ToolAnnotations? by field { s, v -> s.copy(annotations = v) }
    /** null = 清除。 */
    var outputSchema: JsonObject? by field { s, v -> s.copy(outputSchemaJson = v?.toString()) }
    /** null = 清除。 */
    var activation: Activation? by field { s, v -> s.copy(activation = v) }
    var surface: ToolSurface by field { s, v -> s.copy(surface = v) }
    /** null = 清除。 */
    var page: String? by field { s, v -> s.copy(page = v) }

    private fun <T> field(patch: (FfiToolSpec, T) -> FfiToolSpec) =
        object : kotlin.properties.ReadWriteProperty<ToolUpdate, T> {
            @Suppress("UNCHECKED_CAST")
            override fun getValue(thisRef: ToolUpdate, property: kotlin.reflect.KProperty<*>): T {
                check(property.name in values) { "字段 ${property.name} 未赋值" }
                return values[property.name] as T
            }

            override fun setValue(thisRef: ToolUpdate, property: kotlin.reflect.KProperty<*>, value: T) {
                values[property.name] = value
                patches[property.name] = { s -> patch(s, value) }
            }
        }

    internal fun applyTo(spec: FfiToolSpec): FfiToolSpec = patches.values.fold(spec) { s, p -> p(s) }
}

private typealias FfiToolSpec = dev.appmcp.ffi.ToolSpec
