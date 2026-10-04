// 由 app-mcp-codegen 生成（target: kotlin），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.legacy

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** 按状态列出订单（旧版） */
@Serializable
data class OrdersListParams(
    /** 订单状态 */
    @Deprecated("参数已弃用（inputSchema 中 deprecated: true）")
    @SerialName("status") val status: String,
    /**
     * 页码
     * 取值范围：≥ 1
     */
    @Deprecated("参数已弃用（inputSchema 中 deprecated: true）")
    @SerialName("page") val page: Long? = null,
    /** 最多返回条数 */
    @SerialName("limit") val limit: Long? = null,
)

/** 过滤条件 */
@Serializable
data class OrdersFindFilter(
    /** 关键词 */
    @SerialName("keyword") val keyword: String? = null,
    /** 旧标签 */
    @Deprecated("参数已弃用（inputSchema 中 deprecated: true）")
    @SerialName("legacyTag") val legacyTag: String? = null,
)

/** 按状态查找订单 */
@Serializable
data class OrdersFindParams(
    /** 订单状态 */
    @SerialName("state") val state: String,
    /** 分页游标 */
    @SerialName("cursor") val cursor: String? = null,
    /** 过滤条件 */
    @SerialName("filter") val filter: OrdersFindFilter? = null,
)

/** 清空购物车（旧版） */
@Serializable
class CartLegacyClearParams {
    override fun equals(other: Any?): Boolean = other is CartLegacyClearParams
    override fun hashCode(): Int = 0
    override fun toString(): String = "CartLegacyClearParams()"
}

/**
 * 弃用示例 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
 * 返回值作为工具结果（JSON）。
 */
interface LegacyToolHandlers {
    /**
     * 按状态列出订单（旧版）
     *
     * 工具 `orders.list`「列出订单」，风险：read
     */
    @Deprecated("旧版 \"列表\" 接口 */ 不再维护：\$x \${y} \\(z) 'q' <b>&amp; #{w}\n请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）")
    suspend fun ordersList(params: OrdersListParams): JsonElement
    /**
     * 按状态查找订单
     *
     * 工具 `orders.find`「查找订单」，风险：read
     */
    suspend fun ordersFind(params: OrdersFindParams): JsonElement
    /**
     * 清空购物车（旧版）
     *
     * 工具 `cart.legacyClear`「清空购物车」，风险：write
     */
    @Deprecated("改用 cart.clear")
    suspend fun cartLegacyClear(params: CartLegacyClearParams): JsonElement
}

/** 工具名与分派辅助。 */
object LegacyTools {
    /** 清单中的全部工具名。 */
    val names: List<String> = listOf(
        "orders.list",
        "orders.find",
        "cart.legacyClear",
    )

    /** 解析参数使用的 Json 实例（忽略未知字段）。 */
    val json: Json = Json { ignoreUnknownKeys = true }

    /** 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。 */
    @Suppress("DEPRECATION")
    suspend fun dispatch(handlers: LegacyToolHandlers, name: String, arguments: JsonElement?): JsonElement {
        val args = arguments ?: JsonObject(emptyMap())
        return when (name) {
            "orders.list" -> handlers.ordersList(json.decodeFromJsonElement(OrdersListParams.serializer(), args))
            "orders.find" -> handlers.ordersFind(json.decodeFromJsonElement(OrdersFindParams.serializer(), args))
            "cart.legacyClear" -> handlers.cartLegacyClear(json.decodeFromJsonElement(CartLegacyClearParams.serializer(), args))
            else -> throw IllegalArgumentException("未知工具：$name")
        }
    }
}
