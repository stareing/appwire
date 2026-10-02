package dev.appmcp.compose.uifallback

import android.view.View
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.LayoutInfo
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.ScrollAxisRange
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsConfiguration
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.SemanticsPropertyKey
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.AnnotatedString
import dev.appmcp.android.uifallback.UiDescription
import dev.appmcp.android.uifallback.UiElement
import dev.appmcp.android.uifallback.UiEntryKind
import dev.appmcp.android.uifallback.UiOutlineFormat
import dev.appmcp.android.uifallback.UiScrollDirection
import dev.appmcp.android.uifallback.UiViewExpander

// Jetpack Compose 的兜底适配（spec/ui-fallback.md 8.2 Compose 列）：经 AndroidComposeView（ViewRootForTest）的
// SemanticsOwner 读合并后的语义树，动作调用语义动作（OnClick / SetText / SetProgress / ScrollBy 等）。只能在主线程上调用。

/** 已声明工具的语义键（[mcpDeclared]）。 */
val McpDeclaredKey = SemanticsPropertyKey<String>("McpDeclared") { parent, _ -> parent }

/**
 * 标出控件已有声明的工具：兜底大纲中显示 `[已声明：<tool>]`，提示模型优先调用该工具（spec/ui-fallback.md 8.1）。
 * 加在控件本身的 modifier 上；加在外层布局上时标给其子树中唯一的控件。
 *
 * ```kotlin
 * Button(onClick = clear, modifier = Modifier.mcpDeclared("cart.clear")) { Text("清空") }
 * ```
 */
fun Modifier.mcpDeclared(vararg tools: String): Modifier = semantics { this[McpDeclaredKey] = tools.joinToString(", ") }

/**
 * Compose 展开器：把 View 树中的 `AndroidComposeView` 换成其语义树（[dev.appmcp.android.uifallback.UiFallbackOptions.expanders]）。
 *
 * ```kotlin
 * AndroidUiFallback.enable(client, UiFallbackOptions(expanders = listOf(ComposeUiExpander)))
 * ```
 */
object ComposeUiExpander : UiViewExpander {
    override fun expand(view: View): List<UiElement>? {
        val root = view as? ViewRootForTest ?: return null
        return listOf(ComposeUiElement(root.semanticsOwner.rootSemanticsNode))
    }

    override fun isLive(identity: Any): Boolean? = (identity as? LayoutInfo)?.isAttached
}

private val SemanticsConfiguration.texts: String
    get() = getOrNull(SemanticsProperties.Text)?.joinToString(" ") { it.text } ?: ""

private val SemanticsConfiguration.description: String
    get() = getOrNull(SemanticsProperties.ContentDescription)?.joinToString(" ") ?: ""

/** 一个（合并后的）语义节点。 */
class ComposeUiElement internal constructor(val node: SemanticsNode) : UiElement {
    private val config get() = node.config

    override val identity: Any get() = node.layoutInfo

    /** @why LayoutNode 被 Lazy 列表复用时 semanticsId 会重新分配：以它区分"同一对象、另一个控件"。 */
    override val generation: Long get() = node.id.toLong()

    override fun isHidden(): Boolean {
        if (SemanticsProperties.HideFromAccessibility in config) return true
        @Suppress("DEPRECATION")
        if (SemanticsProperties.InvisibleToUser in config) return true
        if (!node.layoutInfo.isPlaced) return true
        // boundsInRoot 已按祖先裁剪：滚出视口或被裁掉时为空。
        val b = node.boundsInRoot
        return b.width <= 0f || b.height <= 0f
    }

    override fun children(): List<UiElement> = node.children.map(::ComposeUiElement)

    private fun has(key: SemanticsPropertyKey<*>) = key in config

    private fun role(): String? {
        val role = config.getOrNull(SemanticsProperties.Role)
        return when {
            has(SemanticsProperties.EditableText) || has(SemanticsActions.SetText) -> "textbox"
            role == Role.DropdownList -> "combobox"
            role == Role.Checkbox -> "checkbox"
            role == Role.Switch -> "switch"
            role == Role.RadioButton -> "radio"
            role == Role.Tab -> "tab"
            has(SemanticsActions.SetProgress) -> "slider"
            role == Role.ValuePicker -> "spinbutton"
            has(SemanticsProperties.ToggleableState) -> "checkbox"
            role == Role.Button -> "button"
            has(SemanticsActions.OnClick) -> "generic"
            else -> null
        }
    }

    private fun nameOf(role: String): String {
        config.description.takeIf { it.isNotBlank() }?.let { return it }
        // 文本框的 Text 是标签 / 占位文字，值在 EditableText 中。
        return config.texts.ifBlank { if (role == "textbox" || role == "combobox") "" else config.getOrNull(SemanticsProperties.StateDescription) ?: "" }
    }

    private fun valueOf(role: String): String? = when (role) {
        "textbox", "combobox" -> config.getOrNull(SemanticsProperties.EditableText)?.text
            ?: config.getOrNull(SemanticsProperties.StateDescription)
        "slider", "spinbutton" -> config.getOrNull(SemanticsProperties.ProgressBarRangeInfo)?.current?.let(::formatNumber)
            ?: config.getOrNull(SemanticsProperties.StateDescription)
        else -> null
    }

    private fun statesOf(role: String): List<String> = buildList {
        if (has(SemanticsProperties.Disabled)) add("disabled")
        val selected = config.getOrNull(SemanticsProperties.Selected)
        when (config.getOrNull(SemanticsProperties.ToggleableState)) {
            ToggleableState.On -> add("checked")
            ToggleableState.Off -> add("unchecked")
            ToggleableState.Indeterminate -> add("mixed")
            null -> if (role == "radio" && selected != null) add(if (selected) "checked" else "unchecked")
        }
        if (selected == true && role != "radio") add("selected")
        if (has(SemanticsActions.Collapse)) add("expanded") else if (has(SemanticsActions.Expand)) add("collapsed")
        if (role == "textbox" && config.getOrNull(SemanticsProperties.IsEditable) == false) add("readonly")
        if (has(SemanticsProperties.Error)) add("invalid")
        if (config.getOrNull(SemanticsProperties.Focused) == true) add("focused")
    }

    override fun describe(): UiDescription {
        val declared = config.getOrNull(McpDeclaredKey)
        role()?.let { role ->
            val secure = has(SemanticsProperties.Password)
            return UiDescription(
                UiEntryKind.ITEM, role, nameOf(role),
                value = if (secure) null else valueOf(role),
                states = statesOf(role),
                secure = secure,
                hasSecureValue = secure && !config.getOrNull(SemanticsProperties.EditableText)?.text.isNullOrEmpty(),
                declared = declared,
                // 合并节点的子节点只剩未合并进来的控件（卡片里的按钮）。
                descend = role != "textbox",
            )
        }
        if (has(SemanticsProperties.Heading) && config.texts.isNotBlank()) return UiDescription(UiEntryKind.HEADING, "heading", config.texts, level = 2)
        if (config.getOrNull(SemanticsProperties.LiveRegion) != null && (config.texts + config.description).isNotBlank()) {
            return UiDescription(UiEntryKind.ITEM, "status", config.description.ifBlank { config.texts }, declared = declared)
        }
        val container = when {
            has(SemanticsProperties.CollectionInfo) -> "list"
            scrollable(config) -> "scrollable"
            config.getOrNull(SemanticsProperties.PaneTitle) != null -> "group"
            else -> null
        }
        if (container != null) {
            val name = config.getOrNull(SemanticsProperties.PaneTitle) ?: config.description
            return UiDescription(UiEntryKind.CONTAINER, container, name, declared = declared)
        }
        return UiDescription(null, declared = declared)
    }

    override fun readText(): List<String> {
        val own = listOf(config.description, config.texts)
        val value = config.getOrNull(SemanticsProperties.EditableText)?.text
        return own + listOfNotNull(if (has(SemanticsProperties.Password)) UiOutlineFormat.SECURE_MASK.takeIf { !value.isNullOrEmpty() } else value)
    }

    override fun isEnabled() = !has(SemanticsProperties.Disabled)

    override suspend fun click(): Boolean {
        config.getOrNull(SemanticsActions.OnClick)?.action?.let { return it() }
        val expand = if (has(SemanticsActions.Collapse)) SemanticsActions.Collapse else SemanticsActions.Expand
        return config.getOrNull(expand)?.action?.invoke() ?: false
    }

    /** 先请求焦点（文本框聚焦后才同步输入法状态），再 SetText。 */
    override suspend fun setText(text: String): Boolean {
        val set = config.getOrNull(SemanticsActions.SetText)?.action ?: return false
        if (config.getOrNull(SemanticsProperties.Focused) != true) config.getOrNull(SemanticsActions.RequestFocus)?.action?.invoke()
        return set(AnnotatedString(text))
    }

    override suspend fun setRange(value: Double): Boolean =
        config.getOrNull(SemanticsActions.SetProgress)?.action?.invoke(value.toFloat()) ?: false

    override fun focus(): Boolean = config.getOrNull(SemanticsActions.RequestFocus)?.action?.invoke() ?: false

    override suspend fun imeAction(): Boolean = config.getOrNull(SemanticsActions.OnImeAction)?.action?.invoke() ?: false

    /** 最近的可滚动祖先按本节点（未裁剪）位置与其视口的差值滚动。 */
    override suspend fun scrollIntoView(): Boolean {
        val coords = node.layoutInfo.coordinates
        val pos = coords.positionInRoot()
        val (w, h) = coords.size.width.toFloat() to coords.size.height.toFloat()
        var cur = node.parent
        while (cur != null) {
            if (SemanticsActions.ScrollBy in cur.config) {
                val view = cur.boundsInRoot
                val dx = delta(pos.x, pos.x + w, view.left, view.right)
                val dy = delta(pos.y, pos.y + h, view.top, view.bottom)
                if (dx != 0f || dy != 0f) return scrollBy(cur.config, dx, dy)
            }
            cur = cur.parent
        }
        return false
    }

    override suspend fun scrollPage(direction: UiScrollDirection): Boolean {
        var cur: SemanticsNode? = node
        while (cur != null) {
            val c = cur.config
            if (canScroll(c, direction)) {
                val b = cur.boundsInRoot
                val (dx, dy) = when (direction) {
                    UiScrollDirection.DOWN -> 0f to b.height
                    UiScrollDirection.UP -> 0f to -b.height
                    UiScrollDirection.RIGHT -> b.width to 0f
                    UiScrollDirection.LEFT -> -b.width to 0f
                }
                return scrollBy(c, dx, dy)
            }
            cur = cur.parent
        }
        return false
    }

    private companion object {
        fun scrollable(c: SemanticsConfiguration) =
            SemanticsActions.ScrollBy in c && (SemanticsProperties.VerticalScrollAxisRange in c || SemanticsProperties.HorizontalScrollAxisRange in c)

        fun canScroll(c: SemanticsConfiguration, d: UiScrollDirection): Boolean {
            if (SemanticsActions.ScrollBy !in c) return false
            val vertical = d == UiScrollDirection.DOWN || d == UiScrollDirection.UP
            val range: ScrollAxisRange = c.getOrNull(if (vertical) SemanticsProperties.VerticalScrollAxisRange else SemanticsProperties.HorizontalScrollAxisRange)
                ?: return false
            val forward = d == UiScrollDirection.DOWN || d == UiScrollDirection.RIGHT
            return if (forward) range.value() < range.maxValue() else range.value() > 0f
        }

        /** 让 [start, end] 落进 [from, to]：已在其中为 0。 */
        fun delta(start: Float, end: Float, from: Float, to: Float): Float = when {
            start < from -> start - from
            end > to -> minOf(end - to, start - from)
            else -> 0f
        }

        /**
         * @why 不用挂起的 ScrollByOffset：它要求调用方协程带 Compose 的帧时钟；ScrollBy 在 Compose 自己的作用域里发起动画，
         *   由引擎等到界面不再变化（UiInspector 的 untilStable）。
         */
        fun scrollBy(c: SemanticsConfiguration, dx: Float, dy: Float): Boolean =
            c.getOrNull(SemanticsActions.ScrollBy)?.action?.invoke(dx, dy) ?: false

        fun formatNumber(f: Float): String = if (f % 1f == 0f) f.toLong().toString() else f.toString()
    }
}
