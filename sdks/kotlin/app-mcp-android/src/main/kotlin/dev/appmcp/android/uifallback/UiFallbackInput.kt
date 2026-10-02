package dev.appmcp.android.uifallback

import dev.appmcp.ErrorKind
import dev.appmcp.ToolCallException
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/** 支持的按键（spec/ui-fallback.md 2.1）。 */
enum class UiKey { ENTER, ESCAPE, TAB, SHIFT_TAB, SPACE }

/** 查看方向（spec/ui-fallback.md 第 2 节：`DOWN` = 向下翻看更多内容）。 */
enum class UiScrollDirection(val wire: String) { UP("up"), DOWN("down"), LEFT("left"), RIGHT("right") }

/** `ui.fill` 的值：文本、数字或布尔。 */
sealed interface UiFillValue {
    data class Text(val text: String) : UiFillValue
    data class Number(val number: Double) : UiFillValue
    data class Bool(val value: Boolean) : UiFillValue
}

/** 兜底工具的参数解析、输入 schema 与错误（spec/ui-fallback.md 第 2 节、7.1），View 与 Compose 共用。 */
object UiFallbackInput {
    fun invalid(message: String, reference: String? = null, reason: String? = null): ToolCallException {
        val details = buildMap {
            reference?.let { put("ref", JsonPrimitive(it)) }
            reason?.let { put("reason", JsonPrimitive(it)) }
        }
        return ToolCallException(ErrorKind.INVALID_INPUT, message, details.takeIf { it.isNotEmpty() }?.let(::JsonObject))
    }

    fun stale(reference: String, prefix: String) = invalid("引用 $reference 已失效，请重新调用 $prefix.outline", reference)

    fun hidden(label: String, reference: String, prefix: String) =
        invalid("${label}当前不可见，请重新调用 $prefix.outline", reference, "hidden")

    fun disabled(label: String, reference: String) = invalid("${label}已禁用，当前无法操作", reference, "TOOL_DISABLED")

    fun secure(label: String, reference: String?) = invalid("${label}是密码类控件，兜底工具不填写", reference, "secure")

    fun unsupported(label: String, reference: String?, why: String? = null) =
        invalid("${label}不支持该操作" + (why?.let { "：$it" } ?: ""), reference, "unsupported")

    private fun get(args: JsonObject, key: String): JsonElement? = args[key]?.takeIf { it != JsonNull }

    fun string(args: JsonObject, key: String): String? {
        val v = get(args, key) ?: return null
        if (v !is JsonPrimitive || !v.isString) throw invalid("$key 应为字符串")
        return v.content
    }

    fun int(args: JsonObject, key: String): Int? {
        val v = get(args, key) ?: return null
        val d = (v as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull
        if (d == null || !d.isFinite()) throw invalid("$key 应为正整数")
        return kotlin.math.floor(d).coerceIn(Int.MIN_VALUE.toDouble(), Int.MAX_VALUE.toDouble()).toInt()
    }

    fun ref(args: JsonObject, key: String, required: Boolean = true): String? {
        val v = get(args, key)
        if (v == null || (v is JsonPrimitive && v.isString && v.content.isEmpty())) {
            if (required) throw invalid("缺少参数 $key（控件引用，如 \"e12\"）")
            return null
        }
        val s = (v as? JsonPrimitive)?.takeIf { it.isString }?.content?.trim()
        if (s == null || !UiOutlineFormat.refPattern.matches(s)) throw invalid("$key 应为控件引用，如 \"e12\"")
        return s
    }

    fun value(args: JsonObject): UiFillValue {
        val v = args["value"] ?: throw invalid("缺少参数 value")
        val p = v as? JsonPrimitive ?: throw invalid("value 应为文本、数字或布尔")
        return when {
            p is JsonNull -> throw invalid("value 应为文本、数字或布尔")
            p.isString -> UiFillValue.Text(p.content)
            p.booleanOrNull != null -> UiFillValue.Bool(p.booleanOrNull!!)
            p.doubleOrNull != null -> UiFillValue.Number(p.doubleOrNull!!)
            else -> throw invalid("value 应为文本、数字或布尔")
        }
    }

    private val keys = mapOf(
        "enter" to UiKey.ENTER, "return" to UiKey.ENTER, "escape" to UiKey.ESCAPE, "esc" to UiKey.ESCAPE,
        "tab" to UiKey.TAB, "shift+tab" to UiKey.SHIFT_TAB, " " to UiKey.SPACE, "space" to UiKey.SPACE,
    )

    fun key(key: String): UiKey =
        keys[if (key == " ") key else key.trim().lowercase()]
            ?: throw invalid("不支持的按键「$key」；支持 Enter、Escape、Tab、Shift+Tab、Space")

    fun direction(direction: String?): UiScrollDirection? {
        if (direction == null) return null
        return UiScrollDirection.entries.firstOrNull { it.wire == direction } ?: throw invalid("direction 应为 up / down / left / right")
    }

    /** 文本值：字符串原样，数字转十进制文本（整数不带小数点）。 */
    fun text(value: UiFillValue, label: String, reference: String?): String = when (value) {
        is UiFillValue.Text -> value.text
        is UiFillValue.Number -> if (value.number % 1.0 == 0.0 && kotlin.math.abs(value.number) < 1e15) value.number.toLong().toString() else value.number.toString()
        is UiFillValue.Bool -> throw invalid("${label}需要文本值", reference)
    }

    fun bool(value: UiFillValue, label: String, reference: String?): Boolean =
        (value as? UiFillValue.Bool)?.value ?: throw invalid("${label}需要 true / false", reference)

    fun number(value: UiFillValue, label: String, reference: String?): Double = when (value) {
        is UiFillValue.Number -> value.number
        is UiFillValue.Text -> value.text.trim().toDoubleOrNull()?.takeIf { it.isFinite() } ?: throw invalid("${label}需要数字", reference)
        is UiFillValue.Bool -> throw invalid("${label}需要数字", reference)
    }

    /** 输入 schema（spec/ui-fallback.md 第 2 节）。 */
    object Schemas {
        private fun refProp(description: String = "控件引用，如 \"e12\"（来自 outline）") =
            buildJsonObject { put("type", "string"); put("description", description) }

        private fun obj(required: List<String>, properties: Map<String, JsonObject>) = buildJsonObject {
            put("type", "object")
            put("properties", JsonObject(properties))
            if (required.isNotEmpty()) put("required", buildJsonArray { required.forEach { add(JsonPrimitive(it)) } })
            put("additionalProperties", false)
        }

        fun outline(maxItems: Int) = obj(
            emptyList(),
            mapOf(
                "query" to buildJsonObject { put("type", "string"); put("description", "按名称模糊过滤（空格分隔多个词，全部匹配）") },
                "within" to refProp("只列出该引用分组的子树，如 \"e40\""),
                "limit" to buildJsonObject {
                    put("type", "integer"); put("minimum", 1); put("maximum", UiOutlineFormat.LIMIT_MAX)
                    put("description", "最多列出的控件数，默认 $maxItems")
                },
            ),
        )

        val ref = obj(listOf("ref"), mapOf("ref" to refProp()))

        val fill = obj(
            listOf("ref", "value"),
            mapOf(
                "ref" to refProp(),
                "value" to buildJsonObject {
                    put("description", "文本、数字（滑块）、布尔（复选框 / 开关 / 单选框）或选项文本（下拉框）")
                    put("anyOf", buildJsonArray {
                        listOf("string", "number", "boolean").forEach { t -> add(buildJsonObject { put("type", t) }) }
                    })
                },
            ),
        )

        val press = obj(
            listOf("key"),
            mapOf(
                "ref" to refProp("目标控件引用；缺省为当前焦点控件"),
                "key" to buildJsonObject { put("type", "string"); put("description", "按键，如 \"Enter\"、\"Escape\"、\"Tab\"、\"Shift+Tab\"、\"Space\"") },
            ),
        )

        val scroll = obj(
            listOf("ref"),
            mapOf(
                "ref" to refProp(),
                "direction" to buildJsonObject {
                    put("type", "string")
                    put("enum", buildJsonArray { UiScrollDirection.entries.forEach { add(JsonPrimitive(it.wire)) } })
                    put("description", "查看方向；缺省为滚动到该控件可见")
                },
            ),
        )

        val read = buildJsonObject {
            put("type", "object")
            putJsonObject("properties") {
                put("ref", refProp("控件引用；缺省为全部窗口"))
                putJsonObject("maxChars") {
                    put("type", "integer"); put("minimum", 1); put("maximum", UiOutlineFormat.READ_MAX)
                    put("description", "默认 ${UiOutlineFormat.READ_DEFAULT}")
                }
            }
            put("additionalProperties", false)
        }
    }
}
