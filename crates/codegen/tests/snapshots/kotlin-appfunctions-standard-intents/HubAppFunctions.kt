// 由 app-mcp-codegen 生成（target: kotlin-appfunctions），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// Android AppFunctions（androidx.appfunctions:1.0.0-alpha12），需要 Android 16（API 36）+。
// 依赖同目录下的 HubTools.kt（参数类型与 HubToolHandlers 接口）。
//
// build.gradle.kts：
//   implementation("androidx.appfunctions:appfunctions:1.0.0-alpha12")
//   ksp("androidx.appfunctions:appfunctions-compiler:1.0.0-alpha12")
//   ksp { arg("appfunctions:aggregateAppFunctions", "true") }  // 应用模块
//   以及 kotlinx-serialization-json、kotlinx-coroutines-android（KSP 生成的服务类使用）
//   与 plugin.serialization。
//
// Application 实现 HubToolHandlersProvider（可直接复用 MCP 的业务实现），并在 AndroidManifest.xml 中声明：
//   <service android:name="appmcp.generated.hub.HubAppFunctionService"
//       android:permission="android.permission.BIND_APP_FUNCTION_SERVICE"
//       android:exported="true" tools:targetApi="36">
//     <property android:name="android.app.appfunctions.schema" android:value="app_functions_schema.xsd" />
//     <property android:name="android.app.appfunctions.v2" android:value="hub_app_function_service.xml" />
//     <intent-filter><action android:name="android.app.appfunctions.AppFunctionService" /></intent-filter>
//   </service>
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.hub

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

/** 由 Application 实现，向 AppFunctions 服务提供 HubToolHandlers。 */
interface HubToolHandlersProvider {
    val hubToolHandlers: HubToolHandlers
}

/** MapNavigateDestination 的 AppFunctions 表示。 */
@AppFunctionSerializable(isDescribedByKDoc = true)
data class MapNavigateDestinationInput(
    val name: String? = null,
    val address: String? = null,
    val lat: Double? = null,
    val lng: Double? = null,
) {
    fun toJson(): JsonObject = buildJsonObject {
        name?.let { put("name", JsonPrimitive(it)) }
        address?.let { put("address", JsonPrimitive(it)) }
        lat?.let { put("lat", JsonPrimitive(it)) }
        lng?.let { put("lng", JsonPrimitive(it)) }
    }
}

/** 参数组装与结果转换。 */
internal object HubAppFunctionsSupport {
    fun parseJson(text: String): JsonElement = try {
        HubTools.json.parseToJsonElement(text)
    } catch (e: SerializationException) {
        throw AppFunctionInvalidArgumentException("JSON 参数无法解析：${e.message}")
    }

    /** handler 结果转文本：JSON 字符串取其内容，其他值输出 JSON。 */
    fun text(result: JsonElement): String =
        (result as? JsonPrimitive)?.takeIf { it.isString }?.content ?: result.toString()
}

/**
 * 生活助手 的 AppFunctions 入口。KSP 生成具体服务类 `HubAppFunctionService`。
 */
@RequiresApi(36)
@AppFunctionServiceEntryPoint(
    serviceName = "HubAppFunctionService",
    appFunctionXmlFileName = "hub_app_function_service",
)
abstract class BaseHubAppFunctionService : AppFunctionService() {
    private val handlers: HubToolHandlers
        get() = (applicationContext as? HubToolHandlersProvider)?.hubToolHandlers
            ?: throw AppFunctionAppUnknownException("Application 未实现 HubToolHandlersProvider")

    private suspend fun call(args: JsonObject, block: suspend (JsonObject) -> JsonElement): String {
        val result = try {
            block(args)
        } catch (e: SerializationException) {
            throw AppFunctionInvalidArgumentException("参数不合法：${e.message}")
        }
        return HubAppFunctionsSupport.text(result)
    }

    /**
     * 给联系人发送一条消息
     *
     * @param to 收件人 元素个数：≥ 1
     * @param text 正文
     * @param subject 主题
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun messageCompose(
        to: List<String>,
        text: String,
        subject: String? = null,
    ): String {
        val args = buildJsonObject {
            put("to", JsonArray(to.map { e -> JsonPrimitive(e) }))
            put("text", JsonPrimitive(text))
            subject?.let { put("subject", JsonPrimitive(it)) }
        }
        return call(args) { handlers.messageCompose(HubTools.json.decodeFromJsonElement(MessageComposeParams.serializer(), it)) }
    }

    /**
     * 在日历中新建一个日程
     *
     * @param title title
     * @param start 格式：date-time
     * @param end 格式：date-time
     * @param allDay allDay
     * @param location location
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun calendarAdd(
        title: String,
        start: String,
        end: String? = null,
        allDay: Boolean? = null,
        location: String? = null,
    ): String {
        val args = buildJsonObject {
            put("title", JsonPrimitive(title))
            put("start", JsonPrimitive(start))
            end?.let { put("end", JsonPrimitive(it)) }
            allDay?.let { put("allDay", JsonPrimitive(it)) }
            location?.let { put("location", JsonPrimitive(it)) }
        }
        return call(args) { handlers.calendarAdd(HubTools.json.decodeFromJsonElement(CalendarAddParams.serializer(), it)) }
    }

    /**
     * 搜索并播放音乐
     *
     * @param query query
     * @param kind kind
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun playerPlay(
        query: String,
        @AppFunctionStringValueConstraint(enumValues = ["song", "album", "artist", "playlist", "podcast", "video"]) kind: String? = null,
    ): String {
        val args = buildJsonObject {
            put("query", JsonPrimitive(query))
            kind?.let { put("kind", JsonPrimitive(it)) }
        }
        return call(args) { handlers.playerPlay(HubTools.json.decodeFromJsonElement(PlayerPlayParams.serializer(), it)) }
    }

    /**
     * 把文件分享给联系人
     *
     * @param files 元素个数：≥ 1
     * @param mimeType mimeType
     * @param text text
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun filesShare(
        files: List<String>,
        mimeType: String? = null,
        text: String? = null,
    ): String {
        val args = buildJsonObject {
            put("files", JsonArray(files.map { e -> JsonPrimitive(e) }))
            mimeType?.let { put("mimeType", JsonPrimitive(it)) }
            text?.let { put("text", JsonPrimitive(it)) }
        }
        return call(args) { handlers.filesShare(HubTools.json.decodeFromJsonElement(FilesShareParams.serializer(), it)) }
    }

    /**
     * 在内置浏览器中打开网址
     *
     * @param url 格式：uri
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun browserOpen(
        url: String,
    ): String {
        val args = buildJsonObject {
            put("url", JsonPrimitive(url))
        }
        return call(args) { handlers.browserOpen(HubTools.json.decodeFromJsonElement(BrowserOpenParams.serializer(), it)) }
    }

    /**
     * 导航到目的地
     *
     * @param destination destination
     * @param mode mode
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun mapNavigate(
        destination: MapNavigateDestinationInput,
        @AppFunctionStringValueConstraint(enumValues = ["drive", "walk", "transit", "bike"]) mode: String? = null,
    ): String {
        val args = buildJsonObject {
            put("destination", destination.toJson())
            mode?.let { put("mode", JsonPrimitive(it)) }
        }
        return call(args) { handlers.mapNavigate(HubTools.json.decodeFromJsonElement(MapNavigateParams.serializer(), it)) }
    }

    /**
     * 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
     *
     * @param to to
     * @param text text
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun messageQuick(
        to: List<String>,
        text: String,
    ): String {
        val args = buildJsonObject {
            put("to", JsonArray(to.map { e -> JsonPrimitive(e) }))
            put("text", JsonPrimitive(text))
        }
        return call(args) { handlers.messageQuick(HubTools.json.decodeFromJsonElement(MessageQuickParams.serializer(), it)) }
    }

    /**
     * 词表外动词
     *
     * @param body body
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun notesCustom(
        body: String? = null,
    ): String {
        val args = buildJsonObject {
            body?.let { put("body", JsonPrimitive(it)) }
        }
        return call(args) { handlers.notesCustom(HubTools.json.decodeFromJsonElement(NotesCustomParams.serializer(), it)) }
    }

    /**
     * 未声明意图
     * @return 工具结果（文本或 JSON）。
     */
    @AppFunction(isDescribedByKDoc = true)
    suspend fun notesPlain(): String {
        val args = JsonObject(emptyMap())
        return call(args) { handlers.notesPlain(HubTools.json.decodeFromJsonElement(NotesPlainParams.serializer(), it)) }
    }
}
