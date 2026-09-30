// 由 app-mcp-codegen 生成（target: kotlin），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.shop

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** 商品分类 */
@Serializable
enum class CatalogSearchCategory {
    @SerialName("electronics") ELECTRONICS,
    @SerialName("home-goods") HOME_GOODS,
    @SerialName("food") FOOD,
}

/** 按关键词搜索商品，返回商品 ID、名称、价格 */
@Serializable
data class CatalogSearchParams(
    /** 商品名或分类，如“耳机” */
    @SerialName("keyword") val keyword: String? = null,
    /** 商品分类 */
    @SerialName("category") val category: CatalogSearchCategory? = null,
    /**
     * 最多返回条数
     * 取值范围：≥ 1，≤ 100；默认值：20
     */
    @SerialName("limit") val limit: Long? = null,
    /** 只看有货 */
    @SerialName("inStock") val inStock: Boolean? = null,
    /**
     * 价格上限（元）
     * 取值范围：> 0
     */
    @SerialName("maxPrice") val maxPrice: Double? = null,
)

/** 把商品加入购物车 */
@Serializable
data class CartAddParams(
    /** 商品 ID */
    @SerialName("productId") val productId: String,
    /**
     * 数量
     * 取值范围：≥ 1
     */
    @SerialName("qty") val qty: Long,
    /** 备注，可为 null */
    @SerialName("note") val note: String? = null,
)

/** 从购物车移除一个条目 */
@Serializable
data class CartRemoveItemParams(
    @SerialName("itemId") val itemId: String,
)

/** 配送方式 */
@Serializable
enum class CartCheckoutShippingMethod {
    @SerialName("standard") STANDARD,
    @SerialName("express") EXPRESS,
}

/** 配送选项 */
@Serializable
data class CartCheckoutShipping(
    /** 配送方式 */
    @SerialName("method") val method: CartCheckoutShippingMethod,
    /**
     * 最早送达时间
     * 格式：date-time
     */
    @SerialName("deliverAfter") val deliverAfter: String? = null,
)

@Serializable
data class CartCheckoutItemsItem(
    @SerialName("itemId") val itemId: String,
    /** 取值范围：≥ 1 */
    @SerialName("qty") val qty: Long? = null,
)

/** 提交订单并支付 */
@Serializable
data class CartCheckoutParams(
    /** 收货地址 ID */
    @SerialName("addressId") val addressId: String,
    /** 优惠码 */
    @SerialName("coupon") val coupon: String? = null,
    /** 配送选项 */
    @SerialName("shipping") val shipping: CartCheckoutShipping? = null,
    /**
     * 只结算这些条目；省略时结算全部
     * 元素个数：≥ 1
     */
    @SerialName("items") val items: List<CartCheckoutItemsItem>? = null,
    /** 默认值：false */
    @SerialName("giftWrap") val giftWrap: Boolean? = null,
)

/** 新增一条待办 */
@Serializable
data class TodosAddParams(
    /**
     * 待办内容
     * 长度：1–200
     */
    @SerialName("title") val title: String,
    /**
     * 优先级
     * 可选值：1, 2, 3
     */
    @SerialName("priority") val priority: Long? = null,
    /** 标签 */
    @SerialName("tags") val tags: List<String>? = null,
    /**
     * 截止日期
     * 格式：date
     */
    @SerialName("dueDate") val dueDate: String? = null,
    /** 分类（属性名是保留字） */
    @SerialName("class") val `class`: String? = null,
    /** 属性名含连字符 */
    @SerialName("is-urgent") val isUrgent: Boolean? = null,
)

/** 删除全部已完成的待办 */
@Serializable
class TodosClearParams {
    override fun equals(other: Any?): Boolean = other is TodosClearParams
    override fun hashCode(): Int = 0
    override fun toString(): String = "TodosClearParams()"
}

/** 按条件导出订单 */
@Serializable
data class OrdersExportParams(
    /**
     * 筛选条件（任意形式）
     * 原始 JSON（不支持一般形式的 `oneOf`）
     */
    @SerialName("filter") val filter: JsonElement? = null,
    /** 附加标签 */
    @SerialName("labels") val labels: Map<String, String>? = null,
    /** 透传给导出器的任意 JSON */
    @SerialName("extra") val extra: JsonElement? = null,
    /** 原始 JSON（不支持 `$ref`） */
    @SerialName("template") val template: JsonElement? = null,
)

/** 统计周期 */
@Serializable
enum class StatsSummaryPeriod {
    @SerialName("day") DAY,
    @SerialName("week") WEEK,
    @SerialName("month") MONTH,
}

/** 查看销售概览 */
@Serializable
data class StatsSummaryParams(
    /** 统计周期 */
    @SerialName("period") val period: StatsSummaryPeriod? = null,
)

/**
 * 示例商城 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
 * 返回值作为工具结果（JSON）。
 */
interface ShopToolHandlers {
    /**
     * 按关键词搜索商品，返回商品 ID、名称、价格
     *
     * 工具 `catalog.search`「搜索商品」，风险：read
     */
    suspend fun catalogSearch(params: CatalogSearchParams): JsonElement
    /**
     * 把商品加入购物车
     *
     * 工具 `cart.add`「加入购物车」，风险：write
     */
    suspend fun cartAdd(params: CartAddParams): JsonElement
    /**
     * 从购物车移除一个条目
     *
     * 工具 `cart.removeItem`「移除购物车条目」，风险：destructive
     */
    suspend fun cartRemoveItem(params: CartRemoveItemParams): JsonElement
    /**
     * 提交订单并支付
     *
     * 工具 `cart.checkout`「结算」，风险：payment
     */
    suspend fun cartCheckout(params: CartCheckoutParams): JsonElement
    /**
     * 新增一条待办
     *
     * 工具 `todos.add`「新增待办」，风险：write
     */
    suspend fun todosAdd(params: TodosAddParams): JsonElement
    /**
     * 删除全部已完成的待办
     *
     * 工具 `todos.clear`「清空待办」，风险：destructive
     */
    suspend fun todosClear(params: TodosClearParams): JsonElement
    /**
     * 按条件导出订单
     *
     * 工具 `orders.export`「导出订单」，风险：write
     */
    suspend fun ordersExport(params: OrdersExportParams): JsonElement
    /**
     * 查看销售概览
     *
     * 工具 `stats.summary`「销售概览」，风险：read
     */
    suspend fun statsSummary(params: StatsSummaryParams): JsonElement
}

/** 工具名与分派辅助。 */
object ShopTools {
    /** 清单中的全部工具名。 */
    val names: List<String> = listOf(
        "catalog.search",
        "cart.add",
        "cart.removeItem",
        "cart.checkout",
        "todos.add",
        "todos.clear",
        "orders.export",
        "stats.summary",
    )

    /** 解析参数使用的 Json 实例（忽略未知字段）。 */
    val json: Json = Json { ignoreUnknownKeys = true }

    /** 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。 */
    suspend fun dispatch(handlers: ShopToolHandlers, name: String, arguments: JsonElement?): JsonElement {
        val args = arguments ?: JsonObject(emptyMap())
        return when (name) {
            "catalog.search" -> handlers.catalogSearch(json.decodeFromJsonElement(CatalogSearchParams.serializer(), args))
            "cart.add" -> handlers.cartAdd(json.decodeFromJsonElement(CartAddParams.serializer(), args))
            "cart.removeItem" -> handlers.cartRemoveItem(json.decodeFromJsonElement(CartRemoveItemParams.serializer(), args))
            "cart.checkout" -> handlers.cartCheckout(json.decodeFromJsonElement(CartCheckoutParams.serializer(), args))
            "todos.add" -> handlers.todosAdd(json.decodeFromJsonElement(TodosAddParams.serializer(), args))
            "todos.clear" -> handlers.todosClear(json.decodeFromJsonElement(TodosClearParams.serializer(), args))
            "orders.export" -> handlers.ordersExport(json.decodeFromJsonElement(OrdersExportParams.serializer(), args))
            "stats.summary" -> handlers.statsSummary(json.decodeFromJsonElement(StatsSummaryParams.serializer(), args))
            else -> throw IllegalArgumentException("未知工具：$name")
        }
    }
}
