//! Android 系统意图映射表（spec/intents.md 第 3 节 Android 列）：每个动词的 intent-filter、Intent 识别谓词、
//! 参数来源与 Kotlin 提取代码。本表是 Android 映射的唯一定义；生成器只按表输出。
//!
//! 依据（2026-10）：
//! - 常量值：本机 `platforms/android-36/android.jar`（`javap -constants`）——`Intent.ACTION_SEND` / `ACTION_SENDTO` /
//!   `ACTION_SEND_MULTIPLE` / `ACTION_INSERT` / `ACTION_VIEW`、`Intent.EXTRA_TEXT` / `EXTRA_SUBJECT` / `EXTRA_STREAM` /
//!   `EXTRA_EMAIL`、`CalendarContract.EXTRA_EVENT_BEGIN_TIME` / `EXTRA_EVENT_END_TIME` / `EXTRA_EVENT_ALL_DAY`、
//!   `CalendarContract.EventsColumns.TITLE` / `DESCRIPTION` / `EVENT_LOCATION`、`MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH`、
//!   `MediaStore.EXTRA_MEDIA_FOCUS`、`MediaStore.Audio.{Media,Albums,Artists,Playlists}.ENTRY_CONTENT_TYPE`、`SearchManager.QUERY`、
//!   `android.net.MailTo`；
//! - 用法与 intent-filter：<https://developer.android.com/guide/components/intents-common>（Calendar、Maps、Music、
//!   Text messaging、Email、Web browser 各节）、<https://developer.android.com/training/sharing/receive>、
//!   <https://developer.android.com/guide/topics/providers/calendar-provider>（Events.CONTENT_URI 与插入事件的 extra）；
//! - `sms:` 多收件人以 `,` 分隔、查询字段 `body`：RFC 5724 第 2.2 节；`mailto:` 由平台 `MailTo` 解析（RFC 6068）。

/// Intent 中取到的值的形态（与工具参数类型对照，见 `standard::accepts`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Str,
    Bool,
    Number,
    StrList,
    /// 目的地对象：键与类型见 [`PLACE_FIELDS`]。
    Place,
}

/// 一个动词参数：名称（词表）、值形态、来源说明（写进生成文件的注释）。
#[derive(Debug)]
pub struct Param {
    pub name: &'static str,
    pub value: Value,
    pub source: &'static str,
}

/// manifest 中的一个 `<intent-filter>`。
#[derive(Debug)]
pub struct Filter {
    pub action: &'static str,
    pub schemes: &'static [&'static str],
    pub mime: Option<&'static str>,
    /// 加 `android.intent.category.BROWSABLE`（允许网页链接启动）。
    pub browsable: bool,
}

/// 一个动词在 Android 上的系统意图。
#[derive(Debug)]
pub struct AndroidIntent {
    pub verb: &'static str,
    /// 词表主版本。
    pub version: u32,
    pub filters: &'static [Filter],
    /// 识别谓词（support 对象中的函数名，签名 `(Intent) -> Boolean`），任一为真即归本动词。
    ///
    /// @invariant 各动词的谓词两两互斥（同一 Intent 至多命中一个动词），由表测试检查 action / 条件组合。
    pub predicates: &'static [&'static str],
    /// 参数提取函数名（support 对象中，签名 `(Intent) -> Map<String, JsonElement>`）。
    pub extractor: &'static str,
    pub params: &'static [Param],
    /// 本动词的 Kotlin 代码（谓词、提取函数与私有常量），放进 support 对象。
    pub code: &'static str,
    pub imports: &'static [&'static str],
}

/// 目的地对象的键与值形态（`navigation.start` 的 `destination`）。
pub const PLACE_FIELDS: [(&str, Value); 4] =
    [("name", Value::Str), ("address", Value::Str), ("lat", Value::Number), ("lng", Value::Number)];

const ACTION_SEND: &str = "android.intent.action.SEND";
const ACTION_SENDTO: &str = "android.intent.action.SENDTO";
const ACTION_SEND_MULTIPLE: &str = "android.intent.action.SEND_MULTIPLE";
const ACTION_INSERT: &str = "android.intent.action.INSERT";
const ACTION_VIEW: &str = "android.intent.action.VIEW";
const ACTION_MEDIA_PLAY_FROM_SEARCH: &str = "android.media.action.MEDIA_PLAY_FROM_SEARCH";

const fn param(name: &'static str, value: Value, source: &'static str) -> Param {
    Param { name, value, source }
}

const fn filter(action: &'static str, schemes: &'static [&'static str], mime: Option<&'static str>) -> Filter {
    Filter { action, schemes, mime, browsable: false }
}

/// 映射表，按 spec/intents.md 第 2 节词表顺序。
pub const ANDROID_INTENTS: [AndroidIntent; 6] = [
    AndroidIntent {
        version: 1,
        verb: "message.send",
        // @security 不加 BROWSABLE：网页不能直接拉起"发消息"（文档示例的短信过滤器带 BROWSABLE，此处有意收紧）
        filters: &[
            filter(ACTION_SENDTO, &["smsto", "sms", "mmsto", "mms", "mailto"], None),
            filter(ACTION_SEND, &[], Some("text/plain")),
        ],
        predicates: &["isMessageSendTo", "isTextSend"],
        extractor: "messageSend",
        params: &[
            param("to", Value::StrList, "SENDTO 的 smsto: / sms: / mmsto: / mms: / mailto: 收件人 + Intent.EXTRA_EMAIL"),
            param("text", Value::Str, "Intent.EXTRA_TEXT，其次 \"sms_body\"，其次 URI 的 body"),
            param("subject", Value::Str, "Intent.EXTRA_SUBJECT，其次 mailto: 的 subject"),
            param("attachments", Value::StrList, "Intent.EXTRA_STREAM（SENDTO 的彩信附件）"),
        ],
        code: MESSAGE_SEND,
        imports: &["import android.net.MailTo", "import android.net.ParseException"],
    },
    AndroidIntent {
        version: 1,
        verb: "calendar.create",
        filters: &[filter(ACTION_INSERT, &[], Some("vnd.android.cursor.dir/event"))],
        predicates: &["isCalendarInsert"],
        extractor: "calendarCreate",
        params: &[
            param("title", Value::Str, "CalendarContract.Events.TITLE"),
            param("start", Value::Str, "CalendarContract.EXTRA_EVENT_BEGIN_TIME（毫秒 → RFC 3339 UTC）"),
            param("end", Value::Str, "CalendarContract.EXTRA_EVENT_END_TIME（毫秒 → RFC 3339 UTC）"),
            param("allDay", Value::Bool, "CalendarContract.EXTRA_EVENT_ALL_DAY"),
            param("location", Value::Str, "CalendarContract.Events.EVENT_LOCATION"),
            param("attendees", Value::StrList, "Intent.EXTRA_EMAIL（逗号分隔的地址）"),
            param("notes", Value::Str, "CalendarContract.Events.DESCRIPTION"),
        ],
        code: CALENDAR_CREATE,
        imports: &["import android.provider.CalendarContract", "import java.time.Instant"],
    },
    AndroidIntent {
        version: 1,
        verb: "media.play",
        filters: &[filter(ACTION_MEDIA_PLAY_FROM_SEARCH, &[], None)],
        predicates: &["isMediaSearch"],
        extractor: "mediaPlay",
        params: &[
            param("query", Value::Str, "SearchManager.QUERY"),
            param("kind", Value::Str, "MediaStore.EXTRA_MEDIA_FOCUS（song / album / artist / playlist）"),
        ],
        code: MEDIA_PLAY,
        imports: &["import android.app.SearchManager", "import android.provider.MediaStore"],
    },
    AndroidIntent {
        version: 1,
        verb: "file.share",
        filters: &[filter(ACTION_SEND, &[], Some("*/*")), filter(ACTION_SEND_MULTIPLE, &[], Some("*/*"))],
        predicates: &["isFileSend"],
        extractor: "fileShare",
        params: &[
            param("files", Value::StrList, "Intent.EXTRA_STREAM（单个 Uri 或 Uri 列表）"),
            param("mimeType", Value::Str, "Intent.getType()"),
            param("text", Value::Str, "Intent.EXTRA_TEXT"),
            param("to", Value::StrList, "Intent.EXTRA_EMAIL"),
        ],
        code: FILE_SHARE,
        imports: &[],
    },
    AndroidIntent {
        version: 1,
        verb: "link.open",
        filters: &[Filter { action: ACTION_VIEW, schemes: &["https"], mime: None, browsable: true }],
        predicates: &["isHttpsView"],
        extractor: "linkOpen",
        params: &[param("url", Value::Str, "Intent.getData()（https:）")],
        code: LINK_OPEN,
        imports: &[],
    },
    AndroidIntent {
        version: 1,
        verb: "navigation.start",
        filters: &[filter(ACTION_VIEW, &["geo"], None)],
        predicates: &["isGeoView"],
        extractor: "navigationStart",
        params: &[param("destination", Value::Place, "geo: URI（lat,lng、q=lat,lng(名称)、q=地址）")],
        code: NAVIGATION_START,
        imports: &[],
    },
];

/// 动词在 Android 上的映射；表中没有时为 `None`。
pub fn lookup(verb: &str, version: u32) -> Option<&'static AndroidIntent> {
    ANDROID_INTENTS.iter().find(|i| i.verb == verb && i.version == version)
}

/// 所有动词共用的 Kotlin 辅助（support 对象内）。
pub const SUPPORT_IMPORTS: &[&str] = &[
    "import android.content.Intent",
    "import android.net.Uri",
    "import java.net.URLDecoder",
    "import kotlinx.serialization.json.JsonArray",
    "import kotlinx.serialization.json.JsonElement",
    "import kotlinx.serialization.json.JsonObject",
    "import kotlinx.serialization.json.JsonPrimitive",
];

pub const HELPERS: &str = r##"/** 一个工具参数的取值规则：`allowed` 限定字符串取值（枚举），`keys` 限定对象的键；缺省不限定。 */
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
}"##;

const MESSAGE_SEND: &str = r##"/** message.send：ACTION_SENDTO 的收件人 URI scheme。 */
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
}"##;

const CALENDAR_CREATE: &str = r##"/** calendar.create：插入事件的 MIME 类型（Android 文档 Add a calendar event）。 */
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
    intent.getLongExtra(key, Long.MIN_VALUE).takeIf { it != Long.MIN_VALUE }?.let { JsonPrimitive(Instant.ofEpochMilli(it).toString()) }"##;

const MEDIA_PLAY: &str = r##"/**
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
}"##;

const FILE_SHARE: &str = r##"fun isFileSend(intent: Intent): Boolean =
    (intent.action == Intent.ACTION_SEND && hasStream(intent)) || intent.action == Intent.ACTION_SEND_MULTIPLE

fun fileShare(intent: Intent): Map<String, JsonElement> = buildMap {
    strings(streams(intent))?.let { put("files", it) }
    intent.type?.let { put("mimeType", JsonPrimitive(it)) }
    text(intent, Intent.EXTRA_TEXT)?.let { put("text", it) }
    strings(intent.getStringArrayExtra(Intent.EXTRA_EMAIL).orEmpty().toList())?.let { put("to", it) }
}"##;

const LINK_OPEN: &str = r##"fun isHttpsView(intent: Intent): Boolean = intent.action == Intent.ACTION_VIEW && scheme(intent) == "https"

fun linkOpen(intent: Intent): Map<String, JsonElement> = buildMap {
    intent.data?.toString()?.let { put("url", JsonPrimitive(it)) }
}"##;

const NAVIGATION_START: &str = r##"/** `lat,lng` 与可选的 `(名称)`（Android 文档 Show a location on a map）。 */
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
}"##;
