// 由 app-mcp-codegen 生成（target: kotlin-appfunctions），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// Android 系统意图（spec/intents.md 第 3 节，--standard-intents）：把系统 Intent 解析为工具调用，
// handler 与 MCP / AppFunctions 共用 HubToolHandlers。依赖同目录下的 HubTools.kt；
// intent-filter 见 HubStandardIntentFilters.xml。
//
// 绑定：
//   message.send@1 → message.compose（SENDTO、SEND）
//   calendar.create@1 → calendar.add（INSERT）
//   media.play@1 → player.play（MEDIA_PLAY_FROM_SEARCH）
//   file.share@1 → files.share（SEND、SEND_MULTIPLE）
//   link.open@1 → browser.open（VIEW）
//   navigation.start@1 → map.navigate（VIEW）
//
// 在处理这些意图的 Activity 的 onCreate 与 onNewIntent 中：
//   when (val r = HubStandardIntents.parse(intent)) {
//       is HubStandardIntentResult.Matched -> lifecycleScope.launch { r.call.call(handlers) }
//       is HubStandardIntentResult.Invalid -> { /* 显示 r.reason 或回到普通界面 */ }
//       HubStandardIntentResult.Unrecognized -> { /* 普通启动 */ }
//   }
//
// @security Intent 可来自任意 App（导出的 Activity 不鉴权调用方）：参数不可信；有副作用的工具（r.call.risk）
// 由 App 在调用前向用户确认。content: URI 的读权限随 Intent 授予本 Activity，handler 应在 Activity 结束前读取。
// calendar.create 使用 java.time（API 26+，更低版本需开启 core library desugaring）。
@file:Suppress("unused", "RedundantVisibilityModifier")

package appmcp.generated.hub

import android.app.SearchManager
import android.content.Intent
import android.net.MailTo
import android.net.ParseException
import android.net.Uri
import android.provider.CalendarContract
import android.provider.MediaStore
import java.net.URLDecoder
import java.time.Instant
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** 标准意图解析出的工具调用（参数已解码为工具参数类型）。 */
sealed interface HubStandardIntentCall {
    /** 工具全名。 */
    val tool: String
    /** 清单声明的风险（read / write / destructive / payment / os-sensitive），供 App 决定是否先确认。 */
    val risk: String
    /** 调用 handler（与 MCP、AppFunctions 共用 HubToolHandlers）。 */
    suspend fun call(handlers: HubToolHandlers): JsonElement

    /** message.send@1 → 工具 `message.compose`「发消息」。 */
    class MessageCompose(val params: MessageComposeParams) : HubStandardIntentCall {
        override val tool: String get() = "message.compose"
        override val risk: String get() = "write"
        override suspend fun call(handlers: HubToolHandlers): JsonElement = handlers.messageCompose(params)
    }

    /** calendar.create@1 → 工具 `calendar.add`「新建日程」。 */
    class CalendarAdd(val params: CalendarAddParams) : HubStandardIntentCall {
        override val tool: String get() = "calendar.add"
        override val risk: String get() = "write"
        override suspend fun call(handlers: HubToolHandlers): JsonElement = handlers.calendarAdd(params)
    }

    /** media.play@1 → 工具 `player.play`「播放」。 */
    class PlayerPlay(val params: PlayerPlayParams) : HubStandardIntentCall {
        override val tool: String get() = "player.play"
        override val risk: String get() = "write"
        override suspend fun call(handlers: HubToolHandlers): JsonElement = handlers.playerPlay(params)
    }

    /** file.share@1 → 工具 `files.share`「分享文件」。 */
    class FilesShare(val params: FilesShareParams) : HubStandardIntentCall {
        override val tool: String get() = "files.share"
        override val risk: String get() = "write"
        override suspend fun call(handlers: HubToolHandlers): JsonElement = handlers.filesShare(params)
    }

    /** link.open@1 → 工具 `browser.open`「打开链接」。 */
    class BrowserOpen(val params: BrowserOpenParams) : HubStandardIntentCall {
        override val tool: String get() = "browser.open"
        override val risk: String get() = "read"
        override suspend fun call(handlers: HubToolHandlers): JsonElement = handlers.browserOpen(params)
    }

    /** navigation.start@1 → 工具 `map.navigate`「开始导航」。 */
    class MapNavigate(val params: MapNavigateParams) : HubStandardIntentCall {
        override val tool: String get() = "map.navigate"
        override val risk: String get() = "write"
        override suspend fun call(handlers: HubToolHandlers): JsonElement = handlers.mapNavigate(params)
    }
}

/** HubStandardIntents.parse 的结果。 */
sealed interface HubStandardIntentResult {
    /** Intent 对应本 App 绑定的一个标准意图。 */
    class Matched(val call: HubStandardIntentCall) : HubStandardIntentResult
    /** Intent 对应工具 `tool`，但无法读取、缺少必填参数或参数不合法；`reason` 可记录或提示用户。 */
    class Invalid(val tool: String, val reason: String) : HubStandardIntentResult
    /** 不是本 App 绑定的标准意图（如从桌面启动）。 */
    object Unrecognized : HubStandardIntentResult
}

/** Android 系统意图 → 工具调用。 */
object HubStandardIntents {
    /** 绑定表：标准意图（动词@主版本）→ 工具全名。 */
    val bindings: Map<String, String> = mapOf(
        "message.send@1" to "message.compose",
        "calendar.create@1" to "calendar.add",
        "media.play@1" to "player.play",
        "file.share@1" to "files.share",
        "link.open@1" to "browser.open",
        "navigation.start@1" to "map.navigate",
    )

    /**
     * 把 Activity 收到的 Intent 解析为工具调用；不调用 handler。
     *
     * ACTION_SEND 带 EXTRA_STREAM 归 file.share，不带归 message.send；对应动词未绑定时为 Unrecognized。
     */
    fun parse(intent: Intent): HubStandardIntentResult = when {
        HubStandardIntentSupport.isMessageSendTo(intent) || HubStandardIntentSupport.isTextSend(intent) -> bind(
            "message.compose",
            { HubStandardIntentSupport.messageSend(intent) },
            listOf(HubStandardIntentSupport.Arg("to", required = true), HubStandardIntentSupport.Arg("text", required = true), HubStandardIntentSupport.Arg("subject", required = false)),
        ) {
            HubStandardIntentCall.MessageCompose(HubTools.json.decodeFromJsonElement(MessageComposeParams.serializer(), it))
        }
        HubStandardIntentSupport.isCalendarInsert(intent) -> bind(
            "calendar.add",
            { HubStandardIntentSupport.calendarCreate(intent) },
            listOf(HubStandardIntentSupport.Arg("title", required = true), HubStandardIntentSupport.Arg("start", required = true), HubStandardIntentSupport.Arg("end", required = false), HubStandardIntentSupport.Arg("allDay", required = false), HubStandardIntentSupport.Arg("location", required = false)),
        ) {
            HubStandardIntentCall.CalendarAdd(HubTools.json.decodeFromJsonElement(CalendarAddParams.serializer(), it))
        }
        HubStandardIntentSupport.isMediaSearch(intent) -> bind(
            "player.play",
            { HubStandardIntentSupport.mediaPlay(intent) },
            listOf(HubStandardIntentSupport.Arg("query", required = true), HubStandardIntentSupport.Arg("kind", required = false, allowed = setOf("song", "album", "artist", "playlist", "podcast", "video"))),
        ) {
            HubStandardIntentCall.PlayerPlay(HubTools.json.decodeFromJsonElement(PlayerPlayParams.serializer(), it))
        }
        HubStandardIntentSupport.isFileSend(intent) -> bind(
            "files.share",
            { HubStandardIntentSupport.fileShare(intent) },
            listOf(HubStandardIntentSupport.Arg("files", required = true), HubStandardIntentSupport.Arg("mimeType", required = false), HubStandardIntentSupport.Arg("text", required = false)),
        ) {
            HubStandardIntentCall.FilesShare(HubTools.json.decodeFromJsonElement(FilesShareParams.serializer(), it))
        }
        HubStandardIntentSupport.isHttpsView(intent) -> bind(
            "browser.open",
            { HubStandardIntentSupport.linkOpen(intent) },
            listOf(HubStandardIntentSupport.Arg("url", required = true)),
        ) {
            HubStandardIntentCall.BrowserOpen(HubTools.json.decodeFromJsonElement(BrowserOpenParams.serializer(), it))
        }
        HubStandardIntentSupport.isGeoView(intent) -> bind(
            "map.navigate",
            { HubStandardIntentSupport.navigationStart(intent) },
            listOf(HubStandardIntentSupport.Arg("destination", required = true, keys = setOf("name", "address", "lat", "lng"))),
        ) {
            HubStandardIntentCall.MapNavigate(HubTools.json.decodeFromJsonElement(MapNavigateParams.serializer(), it))
        }
        else -> HubStandardIntentResult.Unrecognized
    }

    /**
     * 提取参数、按工具的参数规则筛选并解码。
     *
     * @error Intent 无法读取（如 extras 反序列化失败）、缺少必填参数或解码失败时返回 Invalid，不抛出。
     */
    private fun bind(
        tool: String,
        extract: () -> Map<String, JsonElement>,
        args: List<HubStandardIntentSupport.Arg>,
        build: (JsonObject) -> HubStandardIntentCall,
    ): HubStandardIntentResult {
        val raw = try {
            extract()
        } catch (e: RuntimeException) {
            return HubStandardIntentResult.Invalid(tool, "Intent 无法读取：${e.message}")
        }
        val picked = LinkedHashMap<String, JsonElement>()
        for (arg in args) {
            val value = arg.pick(raw[arg.name])
            if (value != null) {
                picked[arg.name] = value
            } else if (arg.required) {
                return HubStandardIntentResult.Invalid(tool, "Intent 中缺少参数 ${arg.name}")
            }
        }
        return try {
            HubStandardIntentResult.Matched(build(JsonObject(picked)))
        } catch (e: IllegalArgumentException) {
            HubStandardIntentResult.Invalid(tool, "参数不合法：${e.message}")
        }
    }
}

/** Intent 识别与参数提取（映射表见 app-mcp-codegen 的 appfunctions/standard/table.rs）。 */
internal object HubStandardIntentSupport {
    /** 一个工具参数的取值规则：`allowed` 限定字符串取值（枚举），`keys` 限定对象的键；缺省不限定。 */
    class Arg(val name: String, val required: Boolean, val allowed: Set<String>? = null, val keys: Set<String>? = null) {
        fun pick(value: JsonElement?): JsonElement? = when {
            value == null -> null
            allowed != null -> value.takeIf { v -> (v as? JsonPrimitive)?.content?.let { it in allowed } == true }
            keys != null -> (value as? JsonObject)?.filterKeys { it in keys }?.takeIf { it.isNotEmpty() }?.let(::JsonObject)
            else -> value
        }
    }

    fun scheme(intent: Intent): String? = intent.data?.scheme?.lowercase()

    fun hasStream(intent: Intent): Boolean = intent.hasExtra(Intent.EXTRA_STREAM)

    /** 文本 extra；发送方可能放 CharSequence（带格式的文本）。 */
    fun text(intent: Intent, key: String): JsonElement? = intent.getCharSequenceExtra(key)?.toString()?.let(::JsonPrimitive)

    /** 去掉空白项；没有剩余时为 null（不传该参数）。 */
    fun strings(values: List<String>): JsonElement? =
        values.map { it.trim() }.filter { it.isNotEmpty() }.takeIf { it.isNotEmpty() }?.let { list -> JsonArray(list.map(::JsonPrimitive)) }

    /**
     * EXTRA_STREAM 中的 URI：单个 Uri 或 Uri 列表（不论 action，按实际类型取）。
     *
     * @why 用 `Bundle.get` 而非类型化的 getParcelableExtra：发送方给错类型时类型化读取会在调用点抛 ClassCastException。
     */
    @Suppress("DEPRECATION")
    fun streams(intent: Intent): List<String> = when (val value = intent.extras?.get(Intent.EXTRA_STREAM)) {
        is Uri -> listOf(value.toString())
        is List<*> -> value.filterIsInstance<Uri>().map { it.toString() }
        else -> emptyList()
    }

    /** 逗号分隔的收件人（RFC 5724 / RFC 6068）。 */
    fun recipients(text: String?): List<String> = text.orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }

    /** 不透明 URI（`sms:` / `geo:`）查询部分中名为 `name` 的字段，按 URL 编码解码；格式错误时为 null。 */
    fun queryField(uri: Uri, name: String): String? =
        uri.encodedSchemeSpecificPart.orEmpty().substringAfter('?', "").split('&')
            .firstOrNull { it.startsWith("$name=") }?.substring(name.length + 1)?.let(::decodeComponent)

    private fun decodeComponent(text: String): String? = try {
        URLDecoder.decode(text, "UTF-8")
    } catch (e: IllegalArgumentException) {
        null
    }

    // ---- message.send
    /** message.send：ACTION_SENDTO 的收件人 URI scheme。 */
    private val MESSAGE_SCHEMES = setOf("smsto", "sms", "mmsto", "mms", "mailto")

    /** 短信正文 extra（Android 文档 Compose an SMS/MMS message；SDK 无常量）。 */
    private const val SMS_BODY = "sms_body"

    fun isMessageSendTo(intent: Intent): Boolean =
        intent.action == Intent.ACTION_SENDTO && scheme(intent)?.let { it in MESSAGE_SCHEMES } == true

    /** ACTION_SEND 不带 EXTRA_STREAM 归 message.send；带 EXTRA_STREAM 的归 file.share（isFileSend）。 */
    fun isTextSend(intent: Intent): Boolean = intent.action == Intent.ACTION_SEND && !hasStream(intent)

    fun messageSend(intent: Intent): Map<String, JsonElement> {
        val data = intent.data
        val mail = data?.takeIf { scheme(intent) == "mailto" }?.let { parseMailTo(it.toString()) }
        val uriRecipients = when {
            mail != null -> recipients(mail.to)
            data != null && intent.action == Intent.ACTION_SENDTO -> recipients(data.schemeSpecificPart?.substringBefore('?'))
            else -> emptyList()
        }
        val uriBody = mail?.body ?: data?.takeIf { mail == null && intent.action == Intent.ACTION_SENDTO }?.let { queryField(it, "body") }
        return buildMap {
            strings(uriRecipients + intent.getStringArrayExtra(Intent.EXTRA_EMAIL).orEmpty())?.let { put("to", it) }
            (text(intent, Intent.EXTRA_TEXT) ?: text(intent, SMS_BODY) ?: uriBody?.let(::JsonPrimitive))?.let { put("text", it) }
            (text(intent, Intent.EXTRA_SUBJECT) ?: mail?.subject?.let(::JsonPrimitive))?.let { put("subject", it) }
            strings(streams(intent))?.let { put("attachments", it) }
        }
    }

    private fun parseMailTo(uri: String): MailTo? = try {
        MailTo.parse(uri)
    } catch (e: ParseException) {
        null
    }

    // ---- calendar.create
    /** calendar.create：插入事件的 MIME 类型（Android 文档 Add a calendar event）。 */
    private const val EVENT_TYPE = "vnd.android.cursor.dir/event"

    fun isCalendarInsert(intent: Intent): Boolean =
        intent.action == Intent.ACTION_INSERT && (intent.data == CalendarContract.Events.CONTENT_URI || intent.type == EVENT_TYPE)

    fun calendarCreate(intent: Intent): Map<String, JsonElement> = buildMap {
        text(intent, CalendarContract.Events.TITLE)?.let { put("title", it) }
        time(intent, CalendarContract.EXTRA_EVENT_BEGIN_TIME)?.let { put("start", it) }
        time(intent, CalendarContract.EXTRA_EVENT_END_TIME)?.let { put("end", it) }
        if (intent.hasExtra(CalendarContract.EXTRA_EVENT_ALL_DAY)) {
            put("allDay", JsonPrimitive(intent.getBooleanExtra(CalendarContract.EXTRA_EVENT_ALL_DAY, false)))
        }
        text(intent, CalendarContract.Events.EVENT_LOCATION)?.let { put("location", it) }
        text(intent, CalendarContract.Events.DESCRIPTION)?.let { put("notes", it) }
        val emails = intent.getStringArrayExtra(Intent.EXTRA_EMAIL)?.toList() ?: recipients(intent.getStringExtra(Intent.EXTRA_EMAIL))
        strings(emails)?.let { put("attendees", it) }
    }

    /** 毫秒时间戳 extra → RFC 3339（UTC）；缺少或类型不对时为 null。 */
    private fun time(intent: Intent, key: String): JsonElement? =
        intent.getLongExtra(key, Long.MIN_VALUE).takeIf { it != Long.MIN_VALUE }?.let { JsonPrimitive(Instant.ofEpochMilli(it).toString()) }

    // ---- media.play
    /**
     * media.play：EXTRA_MEDIA_FOCUS → kind；podcast / video 与流派、任意焦点没有对应值，不传 kind。
     *
     * @compat MediaStore.Audio.Playlists 自 API 31 弃用，但 Common Intents 文档仍以其 ENTRY_CONTENT_TYPE 作播放列表焦点。
     */
    @Suppress("DEPRECATION")
    private val MEDIA_KINDS = mapOf(
        MediaStore.Audio.Media.ENTRY_CONTENT_TYPE to "song",
        MediaStore.Audio.Albums.ENTRY_CONTENT_TYPE to "album",
        MediaStore.Audio.Artists.ENTRY_CONTENT_TYPE to "artist",
        MediaStore.Audio.Playlists.ENTRY_CONTENT_TYPE to "playlist",
    )

    fun isMediaSearch(intent: Intent): Boolean = intent.action == MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH

    fun mediaPlay(intent: Intent): Map<String, JsonElement> = buildMap {
        intent.getStringExtra(SearchManager.QUERY)?.let { put("query", JsonPrimitive(it)) }
        MEDIA_KINDS[intent.getStringExtra(MediaStore.EXTRA_MEDIA_FOCUS)]?.let { put("kind", JsonPrimitive(it)) }
    }

    // ---- file.share
    fun isFileSend(intent: Intent): Boolean =
        (intent.action == Intent.ACTION_SEND && hasStream(intent)) || intent.action == Intent.ACTION_SEND_MULTIPLE

    fun fileShare(intent: Intent): Map<String, JsonElement> = buildMap {
        strings(streams(intent))?.let { put("files", it) }
        intent.type?.let { put("mimeType", JsonPrimitive(it)) }
        text(intent, Intent.EXTRA_TEXT)?.let { put("text", it) }
        strings(intent.getStringArrayExtra(Intent.EXTRA_EMAIL).orEmpty().toList())?.let { put("to", it) }
    }

    // ---- link.open
    fun isHttpsView(intent: Intent): Boolean = intent.action == Intent.ACTION_VIEW && scheme(intent) == "https"

    fun linkOpen(intent: Intent): Map<String, JsonElement> = buildMap {
        intent.data?.toString()?.let { put("url", JsonPrimitive(it)) }
    }

    // ---- navigation.start
    /** `lat,lng` 与可选的 `(名称)`（Android 文档 Show a location on a map）。 */
    private val GEO_POINT = Regex("""^\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*(?:\((.*)\))?\s*$""")

    fun isGeoView(intent: Intent): Boolean = intent.action == Intent.ACTION_VIEW && scheme(intent) == "geo"

    /** `geo:lat,lng`、`geo:0,0?q=lat,lng(名称)`、`geo:0,0?q=地址`；有 q 时以 q 为准。 */
    fun navigationStart(intent: Intent): Map<String, JsonElement> {
        val uri = intent.data ?: return emptyMap()
        val query = queryField(uri, "q")?.takeIf { it.isNotBlank() }
        val place = if (query != null && !GEO_POINT.matches(query)) {
            JsonObject(mapOf("address" to JsonPrimitive(query.trim())))
        } else if (query != null) {
            geoPlace(query)
        } else {
            geoPlace(uri.schemeSpecificPart.orEmpty().substringBefore('?').substringBefore(';'))
        }
        return place?.let { mapOf("destination" to it) }.orEmpty()
    }

    /** 坐标（可带名称）；不是坐标或超出范围时为 null。 */
    private fun geoPlace(text: String): JsonObject? {
        val match = GEO_POINT.matchEntire(text) ?: return null
        val lat = match.groupValues[1].toDouble()
        val lng = match.groupValues[2].toDouble()
        if (lat !in -90.0..90.0 || lng !in -180.0..180.0) return null
        return JsonObject(buildMap {
            put("lat", JsonPrimitive(lat))
            put("lng", JsonPrimitive(lng))
            match.groupValues[3].trim().takeIf { it.isNotEmpty() }?.let { put("name", JsonPrimitive(it)) }
        })
    }
}
