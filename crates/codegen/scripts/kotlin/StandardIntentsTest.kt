// verify.sh 使用：在 Robolectric（真实 android.content.Intent / Uri / MailTo / PackageManager）上检查
// kotlin-appfunctions --standard-intents 对 tests/fixtures/intents.json 的输出。
package appmcp.generated.hub

import android.app.SearchManager
import android.content.ComponentName
import android.content.Intent
import android.net.Uri
import android.provider.CalendarContract
import android.provider.MediaStore
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class StandardIntentsTest {
    private inline fun <reified T : HubStandardIntentCall> matched(intent: Intent): T {
        val r = HubStandardIntents.parse(intent)
        assertTrue("应命中：$r ${(r as? HubStandardIntentResult.Invalid)?.reason}", r is HubStandardIntentResult.Matched)
        val call = (r as HubStandardIntentResult.Matched).call
        assertTrue("调用类型 ${call::class}", call is T)
        return call as T
    }

    private fun invalid(intent: Intent, tool: String, reason: String) {
        val r = HubStandardIntents.parse(intent)
        assertTrue("应为 Invalid：$r", r is HubStandardIntentResult.Invalid)
        r as HubStandardIntentResult.Invalid
        assertEquals(tool, r.tool)
        assertTrue(r.reason, r.reason.contains(reason))
    }

    private fun unrecognized(intent: Intent) {
        assertEquals(HubStandardIntentResult.Unrecognized, HubStandardIntents.parse(intent))
    }

    private val uri1 = Uri.parse("content://media/1")
    private val uri2 = Uri.parse("content://media/2")

    // ---- message.send
    @Test fun smstoRecipientsAndBody() {
        val c = matched<HubStandardIntentCall.MessageCompose>(
            Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:+15105550101,+15105550102")).putExtra("sms_body", "你好"),
        )
        assertEquals(listOf("+15105550101", "+15105550102"), c.params.to)
        assertEquals("你好", c.params.text)
        assertNull(c.params.subject)
    }

    @Test fun smsQueryBody() {
        val c = matched<HubStandardIntentCall.MessageCompose>(Intent(Intent.ACTION_SENDTO, Uri.parse("sms:123?body=hello%20there")))
        assertEquals(listOf("123"), c.params.to)
        assertEquals("hello there", c.params.text)
    }

    @Test fun mailtoSubjectBody() {
        val c = matched<HubStandardIntentCall.MessageCompose>(
            Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:a@example.com,b@example.com?subject=Hi&body=Text")),
        )
        assertEquals(listOf("a@example.com", "b@example.com"), c.params.to)
        assertEquals("Text", c.params.text)
        assertEquals("Hi", c.params.subject)
    }

    @Test fun extrasOverrideUri() {
        val c = matched<HubStandardIntentCall.MessageCompose>(
            Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:a@example.com?subject=U&body=U"))
                .putExtra(Intent.EXTRA_TEXT, "正文").putExtra(Intent.EXTRA_SUBJECT, "主题")
                .putExtra(Intent.EXTRA_EMAIL, arrayOf("c@example.com")),
        )
        assertEquals(listOf("a@example.com", "c@example.com"), c.params.to)
        assertEquals("正文", c.params.text)
        assertEquals("主题", c.params.subject)
    }

    @Test fun sendWithoutStreamIsMessage() {
        val c = matched<HubStandardIntentCall.MessageCompose>(
            Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, "t")
                .putExtra(Intent.EXTRA_EMAIL, arrayOf("x@example.com")),
        )
        assertEquals(listOf("x@example.com"), c.params.to)
    }

    @Test fun messageMissingRecipientIsInvalid() {
        invalid(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, "t"), "message.compose", "to")
        invalid(Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:123")), "message.compose", "text")
    }

    @Test fun sendtoOtherSchemeUnrecognized() = unrecognized(Intent(Intent.ACTION_SENDTO, Uri.parse("tel:123")))

    // ---- file.share 与 ACTION_SEND 区分
    @Test fun sendWithStreamIsFileShare() {
        val c = matched<HubStandardIntentCall.FilesShare>(
            Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_STREAM, uri1).putExtra(Intent.EXTRA_TEXT, "说明"),
        )
        assertEquals(listOf(uri1.toString()), c.params.files)
        assertEquals("text/plain", c.params.mimeType)
        assertEquals("说明", c.params.text)
    }

    @Test fun sendMultiple() {
        val c = matched<HubStandardIntentCall.FilesShare>(
            Intent(Intent.ACTION_SEND_MULTIPLE).setType("image/*").putParcelableArrayListExtra(Intent.EXTRA_STREAM, arrayListOf(uri1, uri2)),
        )
        assertEquals(listOf(uri1.toString(), uri2.toString()), c.params.files)
        assertEquals("image/*", c.params.mimeType)
    }

    @Test fun sendMultipleWithoutStreamIsInvalid() =
        invalid(Intent(Intent.ACTION_SEND_MULTIPLE).setType("image/*"), "files.share", "files")

    @Test fun wrongStreamTypeDoesNotCrash() =
        invalid(Intent(Intent.ACTION_SEND).setType("*/*").putExtra(Intent.EXTRA_STREAM, "not-a-uri"), "files.share", "files")

    // ---- calendar.create
    @Test fun calendarInsert() {
        val c = matched<HubStandardIntentCall.CalendarAdd>(
            Intent(Intent.ACTION_INSERT).setData(CalendarContract.Events.CONTENT_URI)
                .putExtra(CalendarContract.Events.TITLE, "Yoga")
                .putExtra(CalendarContract.EXTRA_EVENT_BEGIN_TIME, 1_326_958_200_000L)
                .putExtra(CalendarContract.EXTRA_EVENT_END_TIME, 1_326_961_800_000L)
                .putExtra(CalendarContract.EXTRA_EVENT_ALL_DAY, true)
                .putExtra(CalendarContract.Events.EVENT_LOCATION, "The gym"),
        )
        assertEquals("Yoga", c.params.title)
        assertEquals("2012-01-19T07:30:00Z", c.params.start)
        assertEquals("2012-01-19T08:30:00Z", c.params.end)
        assertEquals(true, c.params.allDay)
        assertEquals("The gym", c.params.location)
    }

    @Test fun calendarByTypeAndMissingStart() {
        invalid(
            Intent(Intent.ACTION_INSERT).setType("vnd.android.cursor.dir/event").putExtra(CalendarContract.Events.TITLE, "x"),
            "calendar.add",
            "start",
        )
        unrecognized(Intent(Intent.ACTION_INSERT).setData(Uri.parse("content://contacts/people")))
    }

    // ---- media.play
    @Test fun mediaSearchWithFocus() {
        val c = matched<HubStandardIntentCall.PlayerPlay>(
            Intent(MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH).putExtra(SearchManager.QUERY, "Abbey Road")
                .putExtra(MediaStore.EXTRA_MEDIA_FOCUS, MediaStore.Audio.Albums.ENTRY_CONTENT_TYPE),
        )
        assertEquals("Abbey Road", c.params.query)
        assertEquals(PlayerPlayKind.ALBUM, c.params.kind)
    }

    @Test fun mediaAnyFocusEmptyQuery() {
        val c = matched<HubStandardIntentCall.PlayerPlay>(
            Intent(MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH).putExtra(SearchManager.QUERY, "")
                .putExtra(MediaStore.EXTRA_MEDIA_FOCUS, "vnd.android.cursor.item/*"),
        )
        assertEquals("", c.params.query)
        assertNull(c.params.kind)
        invalid(Intent(MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH), "player.play", "query")
    }

    // ---- link.open
    @Test fun httpsLink() {
        val c = matched<HubStandardIntentCall.BrowserOpen>(Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com/a?b=1")))
        assertEquals("https://example.com/a?b=1", c.params.url)
        unrecognized(Intent(Intent.ACTION_VIEW, Uri.parse("http://example.com")))
    }

    // ---- navigation.start
    @Test fun geoCoordinates() {
        val d = matched<HubStandardIntentCall.MapNavigate>(Intent(Intent.ACTION_VIEW, Uri.parse("geo:47.6,-122.3?z=11"))).params.destination
        assertEquals(47.6, d.lat!!, 1e-9)
        assertEquals(-122.3, d.lng!!, 1e-9)
        assertNull(d.name)
    }

    @Test fun geoLabelledQuery() {
        val d = matched<HubStandardIntentCall.MapNavigate>(Intent(Intent.ACTION_VIEW, Uri.parse("geo:0,0?q=34.99,-106.61(Treasure)"))).params.destination
        assertEquals(34.99, d.lat!!, 1e-9)
        assertEquals("Treasure", d.name)
    }

    @Test fun geoAddressQuery() {
        val d = matched<HubStandardIntentCall.MapNavigate>(
            Intent(Intent.ACTION_VIEW, Uri.parse("geo:0,0?q=1600+Amphitheatre+Parkway%2C+CA")),
        ).params.destination
        assertEquals("1600 Amphitheatre Parkway, CA", d.address)
        assertNull(d.lat)
    }

    @Test fun geoOutOfRangeIsInvalid() =
        invalid(Intent(Intent.ACTION_VIEW, Uri.parse("geo:0,0?q=200,10")), "map.navigate", "destination")

    // ---- 兜底与调用
    @Test fun unrelatedIntents() {
        unrecognized(Intent(Intent.ACTION_MAIN))
        unrecognized(Intent())
        unrecognized(Intent(Intent.ACTION_VIEW, Uri.parse("tel:123")))
    }

    @Test fun callDispatchesToHandler() = runBlocking {
        val c = matched<HubStandardIntentCall.BrowserOpen>(Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com")))
        assertEquals("browser.open", c.tool)
        assertEquals("read", c.risk)
        assertEquals(JsonPrimitive("browser:https://example.com"), c.call(FakeHandlers))
    }

    /** 片段中的 intent-filter（verify.sh 合并进 verify.StandardIntentActivity）能让系统把这些 Intent 解析到本 Activity。 */
    @Test fun manifestFiltersResolve() {
        val pm = RuntimeEnvironment.getApplication().packageManager
        val target = ComponentName(RuntimeEnvironment.getApplication(), "verify.StandardIntentActivity")
        fun resolves(intent: Intent): Boolean =
            pm.queryIntentActivities(intent, 0).any { it.activityInfo.name == target.className }
        val hits = listOf(
            Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:1")),
            Intent(Intent.ACTION_SENDTO, Uri.parse("mailto:a@example.com")),
            Intent(Intent.ACTION_SEND).setType("text/plain"),
            Intent(Intent.ACTION_SEND).setType("image/png"),
            Intent(Intent.ACTION_SEND_MULTIPLE).setType("image/png"),
            Intent(Intent.ACTION_INSERT).setDataAndType(CalendarContract.Events.CONTENT_URI, "vnd.android.cursor.dir/event"),
            Intent(MediaStore.INTENT_ACTION_MEDIA_PLAY_FROM_SEARCH),
            Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com")).addCategory(Intent.CATEGORY_BROWSABLE),
            Intent(Intent.ACTION_VIEW, Uri.parse("geo:1,2")),
        )
        for (i in hits) assertTrue("应解析到本 Activity：$i", resolves(i))
        val misses = listOf(
            Intent(Intent.ACTION_VIEW, Uri.parse("http://example.com")),
            Intent(Intent.ACTION_SENDTO, Uri.parse("tel:1")),
            Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:1")).addCategory(Intent.CATEGORY_BROWSABLE),
        )
        for (i in misses) assertTrue("不应解析到本 Activity：$i", !resolves(i))
    }

    private object FakeHandlers : HubToolHandlers {
        private fun r(text: String): JsonElement = JsonPrimitive(text)
        override suspend fun messageCompose(params: MessageComposeParams) = r("message")
        override suspend fun calendarAdd(params: CalendarAddParams) = r("calendar")
        override suspend fun playerPlay(params: PlayerPlayParams) = r("player")
        override suspend fun filesShare(params: FilesShareParams) = r("files")
        override suspend fun browserOpen(params: BrowserOpenParams) = r("browser:${params.url}")
        override suspend fun mapNavigate(params: MapNavigateParams) = r("map")
        override suspend fun messageQuick(params: MessageQuickParams) = r("quick")
        override suspend fun notesCustom(params: NotesCustomParams) = r("custom")
        override suspend fun notesPlain(params: NotesPlainParams) = r("plain")
    }
}
