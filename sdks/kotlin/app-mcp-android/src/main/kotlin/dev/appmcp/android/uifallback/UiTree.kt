package dev.appmcp.android.uifallback

import java.lang.ref.WeakReference
import java.util.IdentityHashMap

/**
 * 控件节点的描述（spec/ui-fallback.md 第 4–6 节）。
 *
 * @property kind null = 不列出、只遍历子节点（布局容器等）；此时只有 [declared] 有意义
 * @property value 原始值（未截断）；[secure] 时不读取，只看 [hasSecureValue]
 * @property descend 控件（[UiEntryKind.ITEM]）是否仍遍历子节点（可点击的布局容器里还有按钮）
 */
class UiDescription(
    val kind: UiEntryKind?,
    val role: String = "generic",
    val name: String = "",
    val value: String? = null,
    val states: List<String> = emptyList(),
    val required: Boolean = false,
    val secure: Boolean = false,
    val hasSecureValue: Boolean = false,
    val declared: String? = null,
    val level: Int? = null,
    val descend: Boolean = false,
) {
    companion object {
        /** 不列出、只遍历子节点。 */
        val TRANSPARENT = UiDescription(null)
    }
}

/**
 * 兜底遍历的控件节点（平台适配：Android View、Compose 语义节点）。所有方法只在主线程调用。
 *
 * 动作方法返回 false 表示该控件不支持此动作；需要更具体的说明时抛 [dev.appmcp.ToolCallException]（用 [UiFallbackInput] 构造）。
 */
interface UiElement {
    /** 弱引用的键：同一控件多次遍历得到的值相同（View 本身、Compose 的 LayoutInfo）。 */
    val identity: Any

    /** 同一 [identity] 被框架复用给另一个控件时变化（Compose 的 semanticsId）；没有复用概念时为 0。 */
    val generation: Long get() = 0

    /**
     * 框架会把同一控件对象复用给另一个数据项、且 [generation] 不反映复用（RecyclerView / ListView 的子 View）：
     * 引用按指纹核对，指纹变了即作废。其余控件改名称或值时引用不变。
     */
    val reusable: Boolean get() = false

    /** 本身（连同子树）不可见：隐藏、折叠、零尺寸、被裁剪或滚出视口。 */
    fun isHidden(): Boolean

    fun describe(): UiDescription

    fun children(): List<UiElement>

    /** `ui.read` 中本节点自身的文本（名称、值；密码类只给掩码）。 */
    fun readText(): List<String>

    /** `ui.read` 是否继续读子节点（文本框内部不再读）。 */
    fun readChildren(): Boolean = true

    fun isEnabled(): Boolean

    /** 激活：点击、切换、选中、展开 / 收起。 */
    suspend fun click(): Boolean

    suspend fun setText(text: String): Boolean = false

    /** 下拉框按选项文本选中（先精确后忽略大小写）；没有该选项时抛 INVALID_INPUT 并列出可选项。 */
    suspend fun selectOption(text: String): Boolean = false

    suspend fun setRange(value: Double): Boolean = false

    fun focus(): Boolean = false

    /** 文本框的输入法动作（完成 / 提交）。 */
    suspend fun imeAction(): Boolean = false

    /** 把本控件滚动到可见。 */
    suspend fun scrollIntoView(): Boolean = false

    /** 本控件或最近的可滚动祖先按查看方向滚动一页；已到尽头或不可滚动时 false。 */
    suspend fun scrollPage(direction: UiScrollDirection): Boolean = false
}

/** 顶层窗口（Activity / 对话框 / 弹出层）：作为 `window` / `dialog` 分组，并承接按键。 */
interface UiWindow : UiElement {
    /** 把按键（按下 + 抬起）交给窗口的焦点控件；返回是否被处理。 */
    fun sendKey(key: UiKey): Boolean

    /** 焦点移到下一个 / 上一个控件。 */
    fun moveFocus(forward: Boolean): Boolean

    /** Escape 未被处理时的关闭 / 取消（对话框取消；主窗口不处理）。 */
    fun dismiss(): Boolean = false
}

/** 一个可见节点及其条目。 */
class UiNode(val element: UiElement, val entry: UiEntry, val secure: Boolean, val window: UiWindow) {
    val label: String get() = "${entry.ref} ${entry.described}"
}

/** 一次收集的结果：条目（按树序）与引用 → 节点。 */
class UiSnapshot(val entries: List<UiEntry>, val byRef: Map<String, UiNode>)

/**
 * 引用 `eN` ↔ 控件（spec/ui-fallback.md 第 3 节）：弱引用 + 指纹（角色 + 名称 + 所在分组）。
 * 控件对象被框架重建（Compose 节点、RecyclerView 复用）时，按指纹在当前界面中唯一匹配到的新控件沿用原引用；
 * 原对象还在但指纹变了（复用给了另一个控件）时旧引用作废。
 *
 * @invariant 只在主线程访问。
 */
class UiRefRegistry {
    private class Record(identity: Any, var generation: Long, val fingerprint: String) {
        var identity = WeakReference(identity)
    }

    private val byRef = LinkedHashMap<String, Record>()
    private var next = 1

    private fun find(identity: Any, generation: Long): String? =
        byRef.entries.firstOrNull { it.value.identity.get() === identity && it.value.generation == generation }?.key

    /**
     * 取得（必要时分配或按指纹沿用）控件的引用。[taken] 为本轮已分配的引用，[fingerprintCount] 为本轮各指纹出现次数；
     * [verify] 为 false（子树收集分组路径不完整，或控件不会被复用）时不核对指纹。[live] 判断旧对象是否仍在界面上。
     */
    fun refOf(
        identity: Any,
        generation: Long,
        fingerprint: String,
        taken: Set<String>,
        fingerprintCount: Map<String, Int>,
        verify: Boolean,
        live: (Any) -> Boolean,
    ): String {
        find(identity, generation)?.let { existing ->
            if (!verify || byRef[existing]?.fingerprint == fingerprint) return existing
            // @why 框架把控件对象复用给了另一个控件（RecyclerView 复用、改名）：旧引用作废，不能让它指向新控件。
            byRef.remove(existing)
        }
        if (fingerprintCount[fingerprint] == 1) {
            val orphans = byRef.entries.filter { (r, rec) ->
                if (r in taken || rec.fingerprint != fingerprint) return@filter false
                val old = rec.identity.get()
                old == null || old === identity || !live(old)
            }
            if (orphans.size == 1) {
                val rec = orphans.single().value
                rec.identity = WeakReference(identity)
                rec.generation = generation
                return orphans.single().key
            }
        }
        val r = "e${next++}"
        byRef[r] = Record(identity, generation, fingerprint)
        return r
    }

    /** 引用记录的控件对象（可能已不在界面上）。 */
    fun target(reference: String): Pair<Any, Long>? = byRef[reference]?.let { rec -> rec.identity.get()?.let { it to rec.generation } }

    /** 丢弃控件已被回收的记录。 */
    fun prune() {
        byRef.entries.removeAll { it.value.identity.get() == null }
    }

    fun clear() = byRef.clear()
}

/** 从窗口收集兜底大纲条目（spec/ui-fallback.md 第 4 节）。只能在主线程上调用。 */
object UiTreeCollector {
    private class Raw(
        val element: UiElement,
        val window: UiWindow,
        val desc: UiDescription,
        val name: String,
        val path: String,
        val depth: Int,
        val chain: List<Int>,
        val containers: List<Int>,
    ) {
        val kind get() = desc.kind!!
    }

    private class Frame(val container: Int) {
        val headings = mutableListOf<Pair<Int, Int>>() // (level, index)
    }

    private fun nameOf(d: UiDescription): String {
        val max = if (d.role == "status" || d.role == "alert") UiOutlineFormat.STATUS_NAME_MAX else UiOutlineFormat.NAME_MAX
        return UiOutlineFormat.truncate(UiOutlineFormat.collapse(d.name), max)
    }

    private fun valueOf(d: UiDescription): String? {
        if (d.secure) return if (d.hasSecureValue) UiOutlineFormat.SECURE_MASK else null
        return UiOutlineFormat.collapse(d.value).takeIf { it.isNotEmpty() }?.let { UiOutlineFormat.truncate(it, UiOutlineFormat.VALUE_MAX) }
    }

    /**
     * 收集 [windows]（或 [start] 子树）下可见的条目，并为控件与分组分配引用。
     * [start] 给出时不按指纹沿用 / 核对引用（分组路径不完整）。
     */
    fun collect(
        refs: UiRefRegistry,
        windows: List<UiWindow>,
        start: Pair<UiElement, UiWindow>? = null,
        live: (Any) -> Boolean = { true },
    ): UiSnapshot {
        val full = start == null
        val raw = mutableListOf<Raw>()
        val declaredGroups = mutableListOf<Triple<String, Int, Int>>()
        val frames = mutableListOf(Frame(-1))
        val containerStack = mutableListOf<Int>()

        fun chain() = frames.flatMap { f -> listOfNotNull(f.container.takeIf { it >= 0 }) + f.headings.map { it.second } }
        fun path(c: List<Int>) = c.joinToString(" › ") { i ->
            raw[i].let { if (it.kind == UiEntryKind.HEADING) it.name else "${UiOutlineFormat.label(it.desc.role)}「${it.name}」" }
        }

        fun walk(e: UiElement, window: UiWindow) {
            if (e.isHidden()) return
            val d = e.describe()
            val kind = d.kind
            var pushed = false
            val declaredFrom = raw.size
            when (kind) {
                null -> Unit
                UiEntryKind.HEADING -> {
                    val name = nameOf(d)
                    if (name.isNotEmpty()) {
                        val frame = frames.last()
                        val level = (d.level ?: 2).coerceIn(1, 3)
                        while (frame.headings.isNotEmpty() && frame.headings.last().first >= level) frame.headings.removeAt(frame.headings.lastIndex)
                        val c = chain()
                        raw += Raw(e, window, UiDescription(UiEntryKind.HEADING, "heading", name, level = level), name, path(c), frames.size - 1, c, containerStack.toList())
                        frame.headings += level to raw.lastIndex
                    }
                }
                else -> {
                    val c = chain()
                    raw += Raw(e, window, d, nameOf(d), path(c), frames.size - 1, c, containerStack.toList())
                    if (kind == UiEntryKind.CONTAINER) {
                        frames += Frame(raw.lastIndex)
                        containerStack += raw.lastIndex
                        pushed = true
                    }
                }
            }
            if (kind != UiEntryKind.ITEM || d.descend) e.children().forEach { walk(it, window) }
            if (pushed) {
                frames.removeAt(frames.lastIndex)
                containerStack.removeAt(containerStack.lastIndex)
            }
            if (kind == null && d.declared != null) declaredGroups += Triple(d.declared, declaredFrom, raw.size)
        }

        if (start != null) walk(start.first, start.second) else windows.forEach { walk(it, it) }

        // 第二遍：指纹计数 → 分配引用 → 条目。
        fun fingerprintOf(r: Raw) = "${r.desc.role}|${r.name}|${r.path}"
        val counts = mutableMapOf<String, Int>()
        raw.filter { it.kind != UiEntryKind.HEADING }.forEach { counts.merge(fingerprintOf(it), 1, Int::plus) }
        val taken = mutableSetOf<String>()
        val refsByIndex = arrayOfNulls<String>(raw.size)
        raw.forEachIndexed { i, r ->
            if (r.kind == UiEntryKind.HEADING) return@forEachIndexed
            val ref = refs.refOf(r.element.identity, r.element.generation, fingerprintOf(r), taken, if (full) counts else emptyMap(), full && r.element.reusable, live)
            taken += ref
            refsByIndex[i] = ref
        }
        val headingKeys = IdentityHashMap<Any, Int>()
        fun keyOf(i: Int) = refsByIndex[i] ?: "h${headingKeys.getOrPut(raw[i].element.identity) { headingKeys.size }}"

        val entries = ArrayList<UiEntry>(raw.size)
        val byRef = LinkedHashMap<String, UiNode>()
        raw.forEachIndexed { i, r ->
            val d = r.desc
            val isItem = r.kind == UiEntryKind.ITEM
            val entry = UiEntry(
                kind = r.kind,
                key = keyOf(i),
                role = d.role,
                label = if (d.secure) UiOutlineFormat.SECURE_LABEL else UiOutlineFormat.label(d.role),
                ref = refsByIndex[i],
                name = r.name,
                value = if (isItem) valueOf(d) else null,
                states = if (isItem) d.states else emptyList(),
                required = isItem && d.required,
                declared = if (r.kind == UiEntryKind.HEADING) null else d.declared,
                level = d.level.takeIf { r.kind == UiEntryKind.HEADING },
                depth = r.depth,
                chain = r.chain,
                containers = r.containers.map(::keyOf),
            )
            entries += entry
            refsByIndex[i]?.let { byRef[it] = UiNode(r.element, entry, d.secure, r.window) }
        }
        // 非控件节点上的声明（如按钮外层的 Modifier / 布局）：子树中恰好一个控件时标给它。
        for ((tool, from, to) in declaredGroups) {
            val items = (from until to).map { entries[it] }.filter { it.kind == UiEntryKind.ITEM }
            if (items.size == 1 && items.single().declared == null) items.single().declared = tool
        }
        refs.prune()
        return UiSnapshot(entries, byRef)
    }

    /** 按引用在全部窗口中（含不可见的节点）找到仍在界面上的控件。 */
    fun findAttached(refs: UiRefRegistry, reference: String, windows: List<UiWindow>): Pair<UiElement, UiWindow>? {
        val (identity, generation) = refs.target(reference) ?: return null
        fun find(e: UiElement, w: UiWindow): Pair<UiElement, UiWindow>? {
            if (e.identity === identity && e.generation == generation) return e to w
            return e.children().firstNotNullOfOrNull { find(it, w) }
        }
        return windows.firstNotNullOfOrNull { find(it, it) }
    }
}
