// 由 app-mcp-codegen 生成（target: kotlin），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.hub

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** 给联系人发送一条消息 */
@Serializable
data class MessageComposeParams(
    /**
     * 收件人
     * 元素个数：≥ 1
     */
    @SerialName("to") val to: List<String>,
    /** 正文 */
    @SerialName("text") val text: String,
    /** 主题 */
    @SerialName("subject") val subject: String? = null,
)

/** 在日历中新建一个日程 */
@Serializable
data class CalendarAddParams(
    @SerialName("title") val title: String,
    /** 格式：date-time */
    @SerialName("start") val start: String,
    /** 格式：date-time */
    @SerialName("end") val end: String? = null,
    @SerialName("allDay") val allDay: Boolean? = null,
    @SerialName("location") val location: String? = null,
)

@Serializable
enum class PlayerPlayKind {
    @SerialName("song") SONG,
    @SerialName("album") ALBUM,
    @SerialName("artist") ARTIST,
    @SerialName("playlist") PLAYLIST,
    @SerialName("podcast") PODCAST,
    @SerialName("video") VIDEO,
}

/** 搜索并播放音乐 */
@Serializable
data class PlayerPlayParams(
    @SerialName("query") val query: String,
    @SerialName("kind") val kind: PlayerPlayKind? = null,
)

/** 把文件分享给联系人 */
@Serializable
data class FilesShareParams(
    /** 元素个数：≥ 1 */
    @SerialName("files") val files: List<String>,
    @SerialName("mimeType") val mimeType: String? = null,
    @SerialName("text") val text: String? = null,
)

/** 在内置浏览器中打开网址 */
@Serializable
data class BrowserOpenParams(
    /** 格式：uri */
    @SerialName("url") val url: String,
)

@Serializable
data class MapNavigateDestination(
    @SerialName("name") val name: String? = null,
    @SerialName("address") val address: String? = null,
    @SerialName("lat") val lat: Double? = null,
    @SerialName("lng") val lng: Double? = null,
)

@Serializable
enum class MapNavigateMode {
    @SerialName("drive") DRIVE,
    @SerialName("walk") WALK,
    @SerialName("transit") TRANSIT,
    @SerialName("bike") BIKE,
}

/** 导航到目的地 */
@Serializable
data class MapNavigateParams(
    @SerialName("destination") val destination: MapNavigateDestination,
    @SerialName("mode") val mode: MapNavigateMode? = null,
)

/** 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个） */
@Serializable
data class MessageQuickParams(
    @SerialName("to") val to: List<String>,
    @SerialName("text") val text: String,
)

/** 词表外动词 */
@Serializable
data class NotesCustomParams(
    @SerialName("body") val body: String? = null,
)

/** 未声明意图 */
@Serializable
class NotesPlainParams {
    override fun equals(other: Any?): Boolean = other is NotesPlainParams
    override fun hashCode(): Int = 0
    override fun toString(): String = "NotesPlainParams()"
}

/**
 * 生活助手 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，
 * 返回值作为工具结果（JSON）。
 */
interface HubToolHandlers {
    /**
     * 给联系人发送一条消息
     *
     * 工具 `message.compose`「发消息」，风险：write
     */
    suspend fun messageCompose(params: MessageComposeParams): JsonElement
    /**
     * 在日历中新建一个日程
     *
     * 工具 `calendar.add`「新建日程」，风险：write
     */
    suspend fun calendarAdd(params: CalendarAddParams): JsonElement
    /**
     * 搜索并播放音乐
     *
     * 工具 `player.play`「播放」，风险：write
     */
    suspend fun playerPlay(params: PlayerPlayParams): JsonElement
    /**
     * 把文件分享给联系人
     *
     * 工具 `files.share`「分享文件」，风险：write
     */
    suspend fun filesShare(params: FilesShareParams): JsonElement
    /**
     * 在内置浏览器中打开网址
     *
     * 工具 `browser.open`「打开链接」，风险：read
     */
    suspend fun browserOpen(params: BrowserOpenParams): JsonElement
    /**
     * 导航到目的地
     *
     * 工具 `map.navigate`「开始导航」，风险：write
     */
    suspend fun mapNavigate(params: MapNavigateParams): JsonElement
    /**
     * 快捷回复（与 message.compose 声明同一动词，系统意图只绑定第一个）
     *
     * 工具 `message.quick`，风险：write
     */
    suspend fun messageQuick(params: MessageQuickParams): JsonElement
    /**
     * 词表外动词
     *
     * 工具 `notes.custom`，风险：write
     */
    suspend fun notesCustom(params: NotesCustomParams): JsonElement
    /**
     * 未声明意图
     *
     * 工具 `notes.plain`，风险：read
     */
    suspend fun notesPlain(params: NotesPlainParams): JsonElement
}

/** 工具名与分派辅助。 */
object HubTools {
    /** 清单中的全部工具名。 */
    val names: List<String> = listOf(
        "message.compose",
        "calendar.add",
        "player.play",
        "files.share",
        "browser.open",
        "map.navigate",
        "message.quick",
        "notes.custom",
        "notes.plain",
    )

    /** 解析参数使用的 Json 实例（忽略未知字段）。 */
    val json: Json = Json { ignoreUnknownKeys = true }

    /** 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。 */
    suspend fun dispatch(handlers: HubToolHandlers, name: String, arguments: JsonElement?): JsonElement {
        val args = arguments ?: JsonObject(emptyMap())
        return when (name) {
            "message.compose" -> handlers.messageCompose(json.decodeFromJsonElement(MessageComposeParams.serializer(), args))
            "calendar.add" -> handlers.calendarAdd(json.decodeFromJsonElement(CalendarAddParams.serializer(), args))
            "player.play" -> handlers.playerPlay(json.decodeFromJsonElement(PlayerPlayParams.serializer(), args))
            "files.share" -> handlers.filesShare(json.decodeFromJsonElement(FilesShareParams.serializer(), args))
            "browser.open" -> handlers.browserOpen(json.decodeFromJsonElement(BrowserOpenParams.serializer(), args))
            "map.navigate" -> handlers.mapNavigate(json.decodeFromJsonElement(MapNavigateParams.serializer(), args))
            "message.quick" -> handlers.messageQuick(json.decodeFromJsonElement(MessageQuickParams.serializer(), args))
            "notes.custom" -> handlers.notesCustom(json.decodeFromJsonElement(NotesCustomParams.serializer(), args))
            "notes.plain" -> handlers.notesPlain(json.decodeFromJsonElement(NotesPlainParams.serializer(), args))
            else -> throw IllegalArgumentException("未知工具：$name")
        }
    }
}
