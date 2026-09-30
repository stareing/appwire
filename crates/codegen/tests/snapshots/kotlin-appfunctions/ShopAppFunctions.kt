// 由 app-mcp-codegen 生成（target: kotlin-appfunctions），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// Android AppFunctions（androidx.appfunctions:1.0.0-alpha12），需要 Android 16（API 36）+。
// 依赖同目录下的 ShopTools.kt（参数类型与 ShopToolHandlers 接口）。
//
// build.gradle.kts：
//   implementation("androidx.appfunctions:appfunctions:1.0.0-alpha12")
//   ksp("androidx.appfunctions:appfunctions-compiler:1.0.0-alpha12")
//   ksp { arg("appfunctions:aggregateAppFunctions", "true") }  // 应用模块
//   以及 kotlinx-serialization-json、kotlinx-coroutines-android（KSP 生成的服务类使用）
//   与 plugin.serialization。
//
// Application 实现 ShopToolHandlersProvider（可直接复用 MCP 的业务实现），并在 AndroidManifest.xml 中声明：
//   <service android:name="appmcp.generated.shop.ShopAppFunctionService"
//       android:permission="android.permission.BIND_APP_FUNCTION_SERVICE"
//       android:exported="true" tools:targetApi="36">
//     <property android:name="android.app.appfunctions.schema" android:value="app_functions_schema.xsd" />
//     <property android:name="android.app.appfunctions.v2" android:value="shop_app_function_service.xml" />
//     <intent-filter><action android:name="android.app.appfunctions.AppFunctionService" /></intent-filter>
//   </service>
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.shop

import androidx.annotation.RequiresApi
import androidx.appfunctions.AppFunction
import androidx.appfunctions.AppFunctionAppUnknownException
import androidx.appfunctions.AppFunctionInvalidArgumentException
import androidx.appfunctions.AppFunctionSerializable
import androidx.appfunctions.AppFunctionService
import androidx.appfunctions.AppFunctionServiceEntryPoint
import androidx.appfunctions.AppFunctionStringValueConstraint
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** 由 Application 实现，向 AppFunctions 服务提供 ShopToolHandlers。 */
interface ShopToolHandlersProvider {
    val shopToolHandlers: ShopToolHandlers
}

/** 配送选项 */
@AppFunctionSerializable(isDescribedByKDoc = true)
data class CartCheckoutShippingInput(
    /** 配送方式 */
    @property:AppFunctionStringValueConstraint(enumValues = ["standard", "express"])
    val method: String,
    /**
     * 最早送达时间
     * 格式：date-time
     */
    val deliverAfter: String? = null,
) {
    fun toJson(): JsonObject = buildJsonObject {
        put("method", JsonPrimitive(method))
        deliverAfter?.let { put("deliverAfter", JsonPrimitive(it)) }
    }
}

/** CartCheckoutItemsItem 的 AppFunctions 表示。 */
@AppFunctionSerializable(isDescribedByKDoc = true)
data class CartCheckoutItemsItemInput(
    val itemId: String,
    /** 取值范围：≥ 1 */
    val qty: Long? = null,
) {
    fun toJson(): JsonObject = buildJsonObject {
        put("itemId", JsonPrimitive(itemId))
        qty?.let { put("qty", JsonPrimitive(it)) }
    }
}

/** 参数组装与结果转换。 */
internal object ShopAppFunctionsSupport {
    fun parseJson(text: String): JsonElement = try {
        ShopTools.json.parseToJsonElement(text)
    } catch (e: SerializationException) {
        throw AppFunctionInvalidArgumentException("JSON 参数无法解析：${e.message}")
    }

    /** handler 结果转文本：JSON 字符串取其内容，其他值输出 JSON。 */
    fun text(result: JsonElement): String =
        (result as? JsonPrimitive)?.takeIf { it.isString }?.content ?: result.toString()
}

/**
 * 示例商城 的 AppFunctions 入口。KSP 生成具体服务类 `ShopAppFunctionService`。
 */
@RequiresApi(36)
@AppFunctionServiceEntryPoint(
    serviceName = "ShopAppFunctionService",
    appFunctionXmlFileName = "shop_app_function_service",
)
abstract class BaseShopAppFunctionService : AppFunctionService() {
    private val handlers: ShopToolHandlers
        get() = (applicationContext as? ShopToolHandlersProvider)?.shopToolHandlers
            ?: throw AppFunctionAppUnknownException("Application 未实现 ShopToolHandlersProvider")

    private suspend fun call(args: JsonObject, block: suspend (JsonObject) -> JsonElement): String {
        val result = try {
            block(args)
        } catch (e: SerializationException) {
            throw AppFunctionInvalidArgumentException("参数不合法：${e.message}")
        }
        return ShopAppFunctionsSupport.text(result)
    }

    /**
     * 按关键词搜索商品，返回商品 ID、名称、价格
     *
     * @param keyword 商品名或分类，如“耳机”
     * @param category 商品分类
     * @param limit 最多返回条数 取值范围：≥ 1，≤ 100；默认值：20
     * @param inStock 只看有货
     * @param maxPrice 价格上限（元） 取值范围：> 0
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun catalogSearch(
        keyword: String? = null,
        @AppFunctionStringValueConstraint(enumValues = ["electronics", "home-goods", "food"]) category: String? = null,
        limit: Long? = null,
        inStock: Boolean? = null,
        maxPrice: Double? = null,
    ): String {
        val args = buildJsonObject {
            keyword?.let { put("keyword", JsonPrimitive(it)) }
            category?.let { put("category", JsonPrimitive(it)) }
            limit?.let { put("limit", JsonPrimitive(it)) }
            inStock?.let { put("inStock", JsonPrimitive(it)) }
            maxPrice?.let { put("maxPrice", JsonPrimitive(it)) }
        }
        return call(args) { handlers.catalogSearch(ShopTools.json.decodeFromJsonElement(CatalogSearchParams.serializer(), it)) }
    }

    /**
     * 把商品加入购物车
     *
     * @param productId 商品 ID
     * @param qty 数量 取值范围：≥ 1
     * @param note 备注，可为 null
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun cartAdd(
        productId: String,
        qty: Long,
        note: String? = null,
    ): String {
        val args = buildJsonObject {
            put("productId", JsonPrimitive(productId))
            put("qty", JsonPrimitive(qty))
            note?.let { put("note", JsonPrimitive(it)) }
        }
        return call(args) { handlers.cartAdd(ShopTools.json.decodeFromJsonElement(CartAddParams.serializer(), it)) }
    }

    /**
     * 从购物车移除一个条目
     *
     * 风险：destructive。调用方应在执行前向用户确认。
     *
     * @param itemId itemId
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun cartRemoveItem(
        itemId: String,
    ): String {
        val args = buildJsonObject {
            put("itemId", JsonPrimitive(itemId))
        }
        return call(args) { handlers.cartRemoveItem(ShopTools.json.decodeFromJsonElement(CartRemoveItemParams.serializer(), it)) }
    }

    /**
     * 提交订单并支付
     *
     * 风险：payment。调用方应在执行前向用户确认。
     *
     * @param addressId 收货地址 ID
     * @param coupon 优惠码
     * @param shipping 配送选项
     * @param items 只结算这些条目；省略时结算全部 元素个数：≥ 1
     * @param giftWrap 默认值：false
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun cartCheckout(
        addressId: String,
        coupon: String? = null,
        shipping: CartCheckoutShippingInput? = null,
        items: List<CartCheckoutItemsItemInput>? = null,
        giftWrap: Boolean? = null,
    ): String {
        val args = buildJsonObject {
            put("addressId", JsonPrimitive(addressId))
            coupon?.let { put("coupon", JsonPrimitive(it)) }
            shipping?.let { put("shipping", it.toJson()) }
            items?.let { put("items", JsonArray(it.map { e -> e.toJson() })) }
            giftWrap?.let { put("giftWrap", JsonPrimitive(it)) }
        }
        return call(args) { handlers.cartCheckout(ShopTools.json.decodeFromJsonElement(CartCheckoutParams.serializer(), it)) }
    }

    /**
     * 新增一条待办
     *
     * @param title 待办内容 长度：1–200
     * @param priority 优先级 可选值：1, 2, 3
     * @param tags 标签
     * @param dueDate 截止日期 格式：date
     * @param class 分类（属性名是保留字）
     * @param isUrgent 属性名含连字符
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun todosAdd(
        title: String,
        priority: Long? = null,
        tags: List<String>? = null,
        dueDate: String? = null,
        `class`: String? = null,
        isUrgent: Boolean? = null,
    ): String {
        val args = buildJsonObject {
            put("title", JsonPrimitive(title))
            priority?.let { put("priority", JsonPrimitive(it)) }
            tags?.let { put("tags", JsonArray(it.map { e -> JsonPrimitive(e) })) }
            dueDate?.let { put("dueDate", JsonPrimitive(it)) }
            `class`?.let { put("class", JsonPrimitive(it)) }
            isUrgent?.let { put("is-urgent", JsonPrimitive(it)) }
        }
        return call(args) { handlers.todosAdd(ShopTools.json.decodeFromJsonElement(TodosAddParams.serializer(), it)) }
    }

    /**
     * 删除全部已完成的待办
     *
     * 风险：destructive。调用方应在执行前向用户确认。
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun todosClear(): String {
        val args = JsonObject(emptyMap())
        return call(args) { handlers.todosClear(ShopTools.json.decodeFromJsonElement(TodosClearParams.serializer(), it)) }
    }

    /**
     * 按条件导出订单
     *
     * @param filter 筛选条件（任意形式） 原始 JSON（不支持一般形式的 `oneOf`） 以 JSON 字符串传入。
     * @param labels 附加标签 以 JSON 字符串传入。
     * @param extra 透传给导出器的任意 JSON 以 JSON 字符串传入。
     * @param template 原始 JSON（不支持 `$ref`） 以 JSON 字符串传入。
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun ordersExport(
        filter: String? = null,
        labels: String? = null,
        extra: String? = null,
        template: String? = null,
    ): String {
        val args = buildJsonObject {
            filter?.let { put("filter", ShopAppFunctionsSupport.parseJson(it)) }
            labels?.let { put("labels", ShopAppFunctionsSupport.parseJson(it)) }
            extra?.let { put("extra", ShopAppFunctionsSupport.parseJson(it)) }
            template?.let { put("template", ShopAppFunctionsSupport.parseJson(it)) }
        }
        return call(args) { handlers.ordersExport(ShopTools.json.decodeFromJsonElement(OrdersExportParams.serializer(), it)) }
    }

    /**
     * 查看销售概览
     *
     * @param period 统计周期
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun statsSummary(
        @AppFunctionStringValueConstraint(enumValues = ["day", "week", "month"]) period: String? = null,
    ): String {
        val args = buildJsonObject {
            period?.let { put("period", JsonPrimitive(it)) }
        }
        return call(args) { handlers.statsSummary(ShopTools.json.decodeFromJsonElement(StatsSummaryParams.serializer(), it)) }
    }
}
