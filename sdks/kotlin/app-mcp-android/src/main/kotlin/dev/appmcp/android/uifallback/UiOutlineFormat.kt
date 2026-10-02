package dev.appmcp.android.uifallback

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// 进程内控件兜底的大纲与变化摘要格式：唯一定义见 spec/ui-fallback.md 第 4–7 节，本文件只是 Kotlin 实现
// （与 UI 框架无关，View 与 Compose 共用；纯 JVM，可直接单元测试）。

/** 条目类别：控件、分组（带引用，可用于 within）、标题（只作分组）。 */
enum class UiEntryKind { ITEM, CONTAINER, HEADING }

/**
 * 大纲中的一个条目。
 *
 * @property key 前后两次快照中对应同一控件的键（控件与分组为引用）
 * @property depth 所在分组层数（缩进）
 * @property chain 祖先分组（分组与标题）在条目列表中的下标，外层在前
 * @property containers 祖先分组的键，外层在前
 */
class UiEntry(
    val kind: UiEntryKind,
    val key: String,
    val role: String,
    val label: String,
    val ref: String? = null,
    val name: String = "",
    val value: String? = null,
    val states: List<String> = emptyList(),
    val required: Boolean = false,
    declared: String? = null,
    val level: Int? = null,
    val depth: Int = 0,
    val chain: List<Int> = emptyList(),
    val containers: List<String> = emptyList(),
) {
    /** 已声明的工具名；外层非控件节点上的声明在收集结束时补给子树中唯一的控件。 */
    var declared: String? = declared
        internal set

    /** `按钮「结算」`。 */
    val described: String get() = if (name.isEmpty()) label else "$label「$name」"
}

/** `ui.outline` 的结果（spec/ui-fallback.md 4.1 / 4.3）。 */
class UiOutline(val text: String, val items: List<UiEntry>, val groups: List<String?>, val total: Int, val remaining: Int, val hint: String?) {
    fun toJson(): JsonObject = buildJsonObject {
        put("text", text)
        put("items", buildJsonArray {
            items.forEachIndexed { i, e ->
                add(buildJsonObject {
                    put("ref", e.ref)
                    put("role", e.role)
                    put("name", e.name)
                    e.value?.let { put("value", it) }
                    if (e.states.isNotEmpty()) put("states", buildJsonArray { e.states.forEach { add(JsonPrimitive(it)) } })
                    if (e.required) put("required", true)
                    e.declared?.let { put("declared", it) }
                    groups[i]?.let { put("group", it) }
                })
            }
        })
        put("total", total)
        if (remaining > 0) put("remaining", remaining)
        hint?.let { put("hint", it) }
    }
}

/** 操作类工具的结果（spec/ui-fallback.md 7.2）。 */
data class UiActionResult(val changes: List<String>, val hint: String? = null) {
    fun toJson(): JsonObject = buildJsonObject {
        put("ok", true)
        put("changes", buildJsonArray { changes.forEach { add(JsonPrimitive(it)) } })
        hint?.let { put("hint", it) }
    }
}

/** `ui.read` 的结果。 */
data class UiReadResult(val ref: String, val text: String, val truncated: Boolean) {
    fun toJson(): JsonObject = buildJsonObject {
        put("ref", ref)
        put("text", text)
        put("truncated", truncated)
    }
}

object UiOutlineFormat {
    /** 角色 → 中文标签（spec/ui-fallback.md 第 5 节）。 */
    val roleLabels: Map<String, String> = mapOf(
        "button" to "按钮", "link" to "链接", "textbox" to "输入框", "searchbox" to "搜索框",
        "checkbox" to "复选框", "radio" to "单选框", "switch" to "开关", "combobox" to "下拉框",
        "listbox" to "列表框", "option" to "选项", "slider" to "滑块", "spinbutton" to "数字框",
        "tab" to "标签页", "menuitem" to "菜单项", "treeitem" to "树节点", "gridcell" to "单元格",
        "status" to "提示", "alert" to "警告", "generic" to "元素",
        "window" to "窗口", "dialog" to "对话框", "alertdialog" to "警告框", "navigation" to "导航",
        "main" to "主区域", "form" to "表单", "search" to "搜索区", "group" to "分组", "list" to "列表",
        "scrollable" to "滚动区", "heading" to "标题",
    )

    /** 密码类控件的标签与固定掩码（spec/ui-fallback.md 8.1：不泄露长度）。 */
    const val SECURE_LABEL = "密码框"
    const val SECURE_MASK = "••••"

    const val NAME_MAX = 40
    const val STATUS_NAME_MAX = 80
    const val VALUE_MAX = 30
    const val MAX_CHANGES = 15
    const val LIMIT_MAX = 500
    const val READ_DEFAULT = 1000
    const val READ_MAX = 20000

    /** 引用格式（spec/ui-fallback.md 第 3 节）。 */
    val refPattern = Regex("^e[1-9]\\d*$")

    private val whitespace = Regex("\\s+")

    fun label(role: String): String = roleLabels[role] ?: "元素"

    fun collapse(s: CharSequence?): String = s?.toString()?.replace(whitespace, " ")?.trim() ?: ""

    /** 截断到 [max] 个码点（保留前 max − 1 个、去掉尾部空白再加 `…`）。 */
    fun truncate(s: String, max: Int): String {
        val count = s.codePointCount(0, s.length)
        if (count <= max) return s
        val end = s.offsetByCodePoints(0, (max - 1).coerceAtLeast(0))
        return s.substring(0, end).trimEnd() + "…"
    }

    private fun groupLabel(e: UiEntry) = if (e.kind == UiEntryKind.HEADING) e.name else e.described

    private fun haystack(e: UiEntry, entries: List<UiEntry>): String =
        (listOf(e.label, e.role, e.name, e.value ?: "", e.declared ?: "") + e.chain.map { groupLabel(entries[it]) })
            .joinToString(" ").lowercase()

    private fun itemLine(e: UiEntry) = buildString {
        append("${e.ref} ${e.described}")
        e.value?.let { append("= \"$it\"") }
        if (e.states.isNotEmpty()) append(' ').append(e.states.joinToString(" "))
        if (e.required) append(" (必填)")
        e.declared?.let { append(" [已声明：$it]") }
    }

    private fun containerLine(e: UiEntry) = "» ${e.ref} ${e.described}" + (e.declared?.let { " [已声明：$it]" } ?: "")

    private fun headingLine(e: UiEntry) = "${"#".repeat(e.level ?: 2)} ${e.name}"

    /** 过滤、截断、分组并渲染大纲（spec/ui-fallback.md 4.2 / 4.3）。 */
    fun render(entries: List<UiEntry>, query: String?, limit: Int): UiOutline {
        val tokens = (query ?: "").lowercase().split(whitespace).filter { it.isNotEmpty() }
        val matched = entries.indices.filter { i ->
            val e = entries[i]
            e.kind == UiEntryKind.ITEM && (tokens.isEmpty() || haystack(e, entries).let { h -> tokens.all { h.contains(it) } })
        }
        val shown = matched.take(limit).toSet()
        val keptGroups = shown.flatMap { entries[it].chain }.toSet()
        val lines = mutableListOf<String>()
        val items = mutableListOf<UiEntry>()
        val groups = mutableListOf<String?>()
        var declared = false
        entries.forEachIndexed { i, e ->
            val indent = " ".repeat(e.depth * 2)
            when {
                e.kind == UiEntryKind.ITEM -> if (i in shown) {
                    lines += indent + itemLine(e)
                    declared = declared || e.declared != null
                    items += e
                    groups += e.chain.joinToString(" › ") { groupLabel(entries[it]) }.ifEmpty { null }
                }
                i !in keptGroups -> Unit
                e.kind == UiEntryKind.CONTAINER -> {
                    lines += indent + containerLine(e)
                    declared = declared || e.declared != null
                }
                else -> lines += indent + headingLine(e)
            }
        }
        val remaining = matched.size - shown.size
        if (lines.isEmpty()) lines += if (tokens.isNotEmpty()) "（没有与「$query」匹配的可交互元素）" else "（没有可见的可交互元素）"
        if (remaining > 0) lines += "…另有 $remaining 个元素未列出，可用 query 或 within 缩小范围"
        return UiOutline(
            lines.joinToString("\n"), items, groups, matched.size, remaining,
            if (declared) "标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具" else null,
        )
    }

    private val ignoredStates = setOf("focused")
    private val exclusiveStates = listOf(setOf("checked", "unchecked", "mixed"), setOf("expanded", "collapsed"))

    private fun stateChange(before: List<String>, after: List<String>): List<String> {
        val b = before.filter { it !in ignoredStates }
        val a = after.filter { it !in ignoredStates }
        val added = a.filter { it !in b }
        val removed = b.filter { it !in a }.filter { r -> exclusiveStates.none { g -> r in g && added.any { it in g } } }
        return listOfNotNull(
            added.takeIf { it.isNotEmpty() }?.let { "变为 ${it.joinToString("、")}" },
            removed.takeIf { it.isNotEmpty() }?.let { "不再 ${it.joinToString("、")}" },
        )
    }

    private fun visibleStates(e: UiEntry): String =
        e.states.filter { it !in ignoredStates }.let { if (it.isEmpty()) "" else " " + it.joinToString(" ") }

    private fun changeLabel(e: UiEntry) = if (e.kind == UiEntryKind.HEADING) "标题「${e.name}」" else "${e.described}(${e.ref})"

    /** 外层最先出现的"同样是新增 / 同样被移除"的祖先分组。 */
    private fun outerChanged(e: UiEntry, set: Map<String, UiEntry>, other: Map<String, UiEntry>): String? =
        e.containers.firstOrNull { it in set && it !in other }

    private fun childCounts(list: List<UiEntry>, set: Map<String, UiEntry>, other: Map<String, UiEntry>): Map<String, Int> {
        val counts = mutableMapOf<String, Int>()
        for (e in list) {
            if (e.key in other) continue
            val outer = outerChanged(e, set, other) ?: continue
            counts[outer] = (counts[outer] ?: 0) + if (e.kind == UiEntryKind.ITEM) 1 else 0
        }
        return counts
    }

    /** 操作前后的变化摘要（spec/ui-fallback.md 7.2）。 */
    fun diff(before: List<UiEntry>, after: List<UiEntry>, prefix: String): List<String> {
        val byBefore = before.associateBy { it.key }
        val byAfter = after.associateBy { it.key }
        val added = childCounts(after, byAfter, byBefore)
        val removed = childCounts(before, byBefore, byAfter)
        val output = mutableListOf<String>()
        for (e in after) {
            val old = byBefore[e.key]
            if (old == null) {
                if (outerChanged(e, byAfter, byBefore) != null) continue
                output += buildString {
                    append("新增${changeLabel(e)}")
                    if (e.kind == UiEntryKind.ITEM) {
                        e.value?.let { append(" = \"$it\"") }
                        append(visibleStates(e))
                    }
                    val n = added[e.key] ?: 0
                    if (n > 0) append("，含 $n 个可交互元素（可用 $prefix.outline({ within: \"${e.ref}\" }) 查看）")
                }
                continue
            }
            if (e.kind == UiEntryKind.HEADING) {
                if (old.name != e.name) output += "标题「${old.name}」变为「${e.name}」"
                continue
            }
            val parts = buildList {
                if (old.name != e.name) add("名称变为「${e.name}」")
                if (old.value != e.value) add(e.value?.let { "值变为 \"$it\"" } ?: "值已清空")
                addAll(stateChange(old.states, e.states))
            }
            if (parts.isNotEmpty()) output += "${e.ref} ${old.name.ifEmpty { old.label }} ${parts.joinToString("，")}"
        }
        for (e in before) {
            if (e.key in byAfter || outerChanged(e, byBefore, byAfter) != null) continue
            val n = removed[e.key] ?: 0
            output += "${changeLabel(e)} 已消失" + if (n > 0) "（含 $n 个可交互元素）" else ""
        }
        if (output.size <= MAX_CHANGES) return output
        val rest = output.size - MAX_CHANGES
        return output.take(MAX_CHANGES) + "…另有 $rest 项变化，请调用 $prefix.outline 查看"
    }

    /** `ui.read` 的文本：折叠空白、相邻重复只保留一次（按钮名称与其内部文本相同）、截断到 [max] 个码点。 */
    fun readText(parts: List<String>, reference: String?, max: Int): UiReadResult {
        val texts = parts.map(::collapse).filter { it.isNotEmpty() }
        val full = texts.filterIndexed { i, s -> i == 0 || s != texts[i - 1] }.joinToString(" ")
        val truncated = full.codePointCount(0, full.length) > max
        return UiReadResult(reference ?: "root", if (truncated) truncate(full, max) else full, truncated)
    }
}
