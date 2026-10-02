package dev.appmcp.android.uifallback

import dev.appmcp.ToolCallException

/** 平台：当前可操作的窗口与"等界面稳定"（Android 见 [AndroidUiPlatform]；测试用假实现）。 */
interface UiPlatform {
    /** 当前可见、未被模态层遮挡的窗口，按 Z 序（最上层在后）。 */
    fun windows(): List<UiWindow>

    /** 控件对象（[UiElement.identity]）仍在界面上（可能不可见）。 */
    fun isLive(identity: Any): Boolean

    /** 执行动作后等界面稳定（一帧 + 短暂延迟）。 */
    suspend fun settle()
}

/**
 * 兜底工具的执行引擎（spec/ui-fallback.md 第 2–7 节）：大纲、点击、填写、按键、滚动、读取。
 * 与客户端、具体 UI 框架无关；所有方法都必须在主线程上调用（[AndroidUiFallback] 负责切换）。
 */
class UiInspector(
    private val platform: UiPlatform,
    val prefix: String = "ui",
    val maxItems: Int = 60,
) {
    private val refs = UiRefRegistry()

    /** 丢弃全部引用（之后旧引用一律失效）。 */
    fun clear() = refs.clear()

    private fun snapshot(): UiSnapshot = UiTreeCollector.collect(refs, platform.windows(), live = platform::isLive)

    /** 按引用在当前界面中取控件（spec/ui-fallback.md 7.1）。 */
    private fun resolve(reference: String, snap: UiSnapshot): UiNode {
        snap.byRef[reference]?.let { return it }
        if (UiTreeCollector.findAttached(refs, reference, platform.windows()) != null) {
            throw UiFallbackInput.hidden("引用 $reference 对应的控件", reference, prefix)
        }
        throw UiFallbackInput.stale(reference, prefix)
    }

    private fun actionable(reference: String, snap: UiSnapshot): UiNode {
        val n = resolve(reference, snap)
        if (!n.element.isEnabled()) throw UiFallbackInput.disabled(n.label, reference)
        return n
    }

    private fun refuseSecure(n: UiNode) {
        if (n.secure) throw UiFallbackInput.secure(n.label, n.entry.ref)
    }

    private fun unsupported(n: UiNode, why: String? = null): Nothing = throw UiFallbackInput.unsupported(n.label, n.entry.ref, why)

    /** [untilStable]：滚动等带动画的动作，再等到相邻两次快照没有变化（最多 [STABLE_ROUNDS] 轮）。 */
    private suspend fun act(before: UiSnapshot, target: UiNode?, untilStable: Boolean = false, run: suspend () -> Unit): UiActionResult {
        run()
        platform.settle()
        var after = snapshot()
        for (round in 0 until if (untilStable) STABLE_ROUNDS else 0) {
            platform.settle()
            val next = snapshot()
            val moving = UiOutlineFormat.diff(after.entries, next.entries, prefix).isNotEmpty()
            after = next
            if (!moving) break
        }
        return UiActionResult(
            UiOutlineFormat.diff(before.entries, after.entries, prefix),
            target?.entry?.declared?.let { "该元素已声明为工具 $it，下次可直接调用" },
        )
    }

    /** `ui.outline`（spec/ui-fallback.md 第 4 节）。 */
    fun outline(query: String?, within: String?, limit: Int?): UiOutline {
        val n = (limit ?: maxItems).coerceIn(1, UiOutlineFormat.LIMIT_MAX)
        val snap = snapshot()
        if (within == null) return UiOutlineFormat.render(snap.entries, query, n)
        val scope = resolve(within, snap)
        val sub = UiTreeCollector.collect(refs, emptyList(), scope.element to scope.window, platform::isLive)
        return UiOutlineFormat.render(sub.entries, query, n)
    }

    /** `ui.click`：激活控件。 */
    suspend fun click(reference: String): UiActionResult {
        val before = snapshot()
        val n = actionable(reference, before)
        return act(before, n) { if (!n.element.click()) unsupported(n) }
    }

    /** `ui.fill`：文本框写入文本；复选框 / 开关 / 单选框按布尔值切换；下拉框按选项文本；滑块按数字。 */
    suspend fun fill(reference: String, value: UiFillValue): UiActionResult {
        val before = snapshot()
        val n = actionable(reference, before)
        refuseSecure(n)
        val e = n.entry
        return when (e.role) {
            "textbox", "searchbox" -> {
                val text = UiFallbackInput.text(value, n.label, reference)
                if ("readonly" in e.states) unsupported(n, "只读")
                act(before, n) { if (!n.element.setText(text)) unsupported(n) }
            }
            "checkbox", "switch", "radio" -> {
                val want = UiFallbackInput.bool(value, n.label, reference)
                if (("checked" in e.states) == want) return act(before, n) {}
                if (e.role == "radio" && !want) unsupported(n, "单选框不能直接取消选中，请选中同组的其他项")
                act(before, n) { if (!n.element.click()) unsupported(n) }
            }
            "combobox" -> {
                val text = UiFallbackInput.text(value, n.label, reference)
                act(before, n) { if (!n.element.selectOption(text)) unsupported(n, "请先 click 展开再 click 选项") }
            }
            "slider", "spinbutton" -> {
                val number = UiFallbackInput.number(value, n.label, reference)
                act(before, n) { if (!n.element.setRange(number)) unsupported(n) }
            }
            else -> unsupported(n, "只能填写文本框、复选框、开关、单选框、下拉框与滑块")
        }
    }

    /** `ui.press`（spec/ui-fallback.md 2.1）：[reference] 给出时先聚焦该控件。 */
    suspend fun press(reference: String?, key: String): UiActionResult {
        val k = UiFallbackInput.key(key)
        val before = snapshot()
        val target = reference?.let { actionable(it, before) }
        target?.let(::refuseSecure)
        val focused = before.byRef.values.lastOrNull { "focused" in it.entry.states }
        if (target == null && focused?.secure == true) {
            throw UiFallbackInput.invalid("当前焦点在密码类控件上，兜底工具不对其按键", reason = "secure")
        }
        val window = (target ?: focused)?.window ?: platform.windows().lastOrNull()
            ?: throw ToolCallException(dev.appmcp.ErrorKind.TOOL_DISABLED, "应用当前没有可见窗口")
        var handled = true
        val result = act(before, target) {
            target?.element?.focus()
            val subject = target ?: focused
            handled = when (k) {
                UiKey.TAB -> window.moveFocus(forward = true)
                UiKey.SHIFT_TAB -> window.moveFocus(forward = false)
                UiKey.ESCAPE -> platform.windows().lastOrNull().let { top -> top != null && (top.sendKey(k) || top.dismiss()) }
                UiKey.ENTER -> (subject?.entry?.role in TEXT_ROLES && subject!!.element.imeAction()) ||
                    window.sendKey(k) || (subject != null && subject.entry.role !in TEXT_ROLES && subject.element.click())
                UiKey.SPACE -> window.sendKey(k) || (subject != null && subject.entry.role !in TEXT_ROLES && subject.element.click())
            }
        }
        return if (handled) result else result.copy(hint = result.hint ?: "按键 $key 没有被任何控件处理")
    }

    /** `ui.scroll`：无 [direction] 时把控件滚动到可见，否则滚动控件（或其所在滚动区）一页。只核对引用仍在界面上。 */
    suspend fun scroll(reference: String, direction: String?): UiActionResult {
        val dir = UiFallbackInput.direction(direction)
        val before = snapshot()
        val visible = before.byRef[reference]
        val element = visible?.element
            ?: UiTreeCollector.findAttached(refs, reference, platform.windows())?.first
            ?: throw UiFallbackInput.stale(reference, prefix)
        if (dir == null) {
            if (visible != null) return UiActionResult(emptyList())
            var moved = false
            val result = act(before, null, untilStable = true) { moved = element.scrollIntoView() }
            return if (moved) result else result.copy(hint = "没有可滚动的祖先，无法滚动到可见")
        }
        var moved = false
        val result = act(before, null, untilStable = true) { moved = element.scrollPage(dir) }
        return if (moved) result else result.copy(hint = "已到尽头或不可滚动")
    }

    /** `ui.read`：控件（缺省为全部窗口）的可见文本，折叠空白；密码类控件只返回掩码。 */
    fun read(reference: String?, maxChars: Int?): UiReadResult {
        val max = (maxChars ?: UiOutlineFormat.READ_DEFAULT).coerceIn(1, UiOutlineFormat.READ_MAX)
        val parts = mutableListOf<String>()
        fun walk(e: UiElement) {
            if (e.isHidden()) return
            parts += e.readText()
            if (e.readChildren()) e.children().forEach(::walk)
        }
        if (reference != null) walk(resolve(reference, snapshot()).element) else platform.windows().forEach(::walk)
        return UiOutlineFormat.readText(parts, reference, max)
    }

    private companion object {
        val TEXT_ROLES = setOf("textbox", "searchbox")
        const val STABLE_ROUNDS = 10
    }
}
