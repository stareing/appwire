// 由 app-mcp-codegen 生成（target: kotlin-appfunctions），请勿手动修改。
// App：弃用示例（legacy）
// 简介：演示工具级与参数级弃用的生成结果
//
// Android AppFunctions（androidx.appfunctions:1.0.0-alpha12），需要 Android 16（API 36）+。
// 依赖同目录下的 LegacyTools.kt（参数类型与 LegacyToolHandlers 接口）。
//
// build.gradle.kts：
//   implementation("androidx.appfunctions:appfunctions:1.0.0-alpha12")
//   ksp("androidx.appfunctions:appfunctions-compiler:1.0.0-alpha12")
//   ksp { arg("appfunctions:aggregateAppFunctions", "true") }  // 应用模块
//   以及 kotlinx-serialization-json、kotlinx-coroutines-android（KSP 生成的服务类使用）
//   与 plugin.serialization。
//
// Application 实现 LegacyToolHandlersProvider（可直接复用 MCP 的业务实现），并在 AndroidManifest.xml 中声明：
//   <service android:name="appmcp.generated.legacy.LegacyAppFunctionService"
//       android:permission="android.permission.BIND_APP_FUNCTION_SERVICE"
//       android:exported="true" tools:targetApi="36">
//     <property android:name="android.app.appfunctions.schema" android:value="app_functions_schema.xsd" />
//     <property android:name="android.app.appfunctions.v2" android:value="legacy_app_function_service.xml" />
//     <intent-filter><action android:name="android.app.appfunctions.AppFunctionService" /></intent-filter>
//   </service>
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.legacy

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

/** 由 Application 实现，向 AppFunctions 服务提供 LegacyToolHandlers。 */
interface LegacyToolHandlersProvider {
    val legacyToolHandlers: LegacyToolHandlers
}

/** 过滤条件 */
@AppFunctionSerializable(isDescribedByKDoc = true)
data class OrdersFindFilterInput(
    /** 关键词 */
    val keyword: String? = null,
    // 参数已弃用（inputSchema 中 deprecated: true）
    /** 旧标签 */
    val legacyTag: String? = null,
) {
    fun toJson(): JsonObject = buildJsonObject {
        keyword?.let { put("keyword", JsonPrimitive(it)) }
        legacyTag?.let { put("legacyTag", JsonPrimitive(it)) }
    }
}

/** 参数组装与结果转换。 */
internal object LegacyAppFunctionsSupport {
    fun parseJson(text: String): JsonElement = try {
        LegacyTools.json.parseToJsonElement(text)
    } catch (e: SerializationException) {
        throw AppFunctionInvalidArgumentException("JSON 参数无法解析：${e.message}")
    }

    /** handler 结果转文本：JSON 字符串取其内容，其他值输出 JSON。 */
    fun text(result: JsonElement): String =
        (result as? JsonPrimitive)?.takeIf { it.isString }?.content ?: result.toString()
}

/**
 * 弃用示例 的 AppFunctions 入口。KSP 生成具体服务类 `LegacyAppFunctionService`。
 */
@RequiresApi(36)
@AppFunctionServiceEntryPoint(
    serviceName = "LegacyAppFunctionService",
    appFunctionXmlFileName = "legacy_app_function_service",
)
abstract class BaseLegacyAppFunctionService : AppFunctionService() {
    private val handlers: LegacyToolHandlers
        get() = (applicationContext as? LegacyToolHandlersProvider)?.legacyToolHandlers
            ?: throw AppFunctionAppUnknownException("Application 未实现 LegacyToolHandlersProvider")

    private suspend fun call(args: JsonObject, block: suspend (JsonObject) -> JsonElement): String {
        val result = try {
            block(args)
        } catch (e: SerializationException) {
            throw AppFunctionInvalidArgumentException("参数不合法：${e.message}")
        }
        return LegacyAppFunctionsSupport.text(result)
    }

    /**
     * 按状态列出订单（旧版）
     *
     * @param status 订单状态
     * @param page 页码 取值范围：≥ 1
     * @param limit 最多返回条数
     * @return 工具结果（文本或 JSON）。
     */
    @Deprecated("旧版 \"列表\" 接口 */ 不再维护：\$x \${y} \\(z) 'q' <b>&amp; #{w}\n请改用分页更好的新版（改用工具 orders.find；计划于 2027-06-30 移除）")
    @Suppress("DEPRECATION")
    @AppFunction(isDescribedByKDoc = true)
    suspend fun ordersList(
        // 参数已弃用（inputSchema 中 deprecated: true）
        status: String,
        // 参数已弃用（inputSchema 中 deprecated: true）
        page: Long? = null,
        limit: Long? = null,
    ): String {
        val args = buildJsonObject {
            put("status", JsonPrimitive(status))
            page?.let { put("page", JsonPrimitive(it)) }
            limit?.let { put("limit", JsonPrimitive(it)) }
        }
        return call(args) { handlers.ordersList(LegacyTools.json.decodeFromJsonElement(OrdersListParams.serializer(), it)) }
    }

    /**
     * 按状态查找订单
     *
     * @param state 订单状态
     * @param cursor 分页游标
     * @param filter 过滤条件
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun ordersFind(
        state: String,
        cursor: String? = null,
        filter: OrdersFindFilterInput? = null,
    ): String {
        val args = buildJsonObject {
            put("state", JsonPrimitive(state))
            cursor?.let { put("cursor", JsonPrimitive(it)) }
            filter?.let { put("filter", it.toJson()) }
        }
        return call(args) { handlers.ordersFind(LegacyTools.json.decodeFromJsonElement(OrdersFindParams.serializer(), it)) }
    }

    /**
     * 清空购物车（旧版）
     * @return 工具结果（文本或 JSON）。
     */
    @Deprecated("改用 cart.clear")
    @Suppress("DEPRECATION")
    @AppFunction(isDescribedByKDoc = true)
    suspend fun cartLegacyClear(): String {
        val args = JsonObject(emptyMap())
        return call(args) { handlers.cartLegacyClear(LegacyTools.json.decodeFromJsonElement(CartLegacyClearParams.serializer(), it)) }
    }
}
