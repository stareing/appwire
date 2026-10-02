package dev.appmcp.android.uifallback

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.text.InputType
import android.text.method.PasswordTransformationMethod
import android.view.FocusFinder
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.view.accessibility.AccessibilityNodeInfo
import android.view.inspector.WindowInspector
import android.widget.AbsListView
import android.widget.AbsSeekBar
import android.widget.AbsSpinner
import android.widget.AdapterView
import android.widget.Button
import android.widget.Checkable
import android.widget.CompoundButton
import android.widget.EditText
import android.widget.ImageButton
import android.widget.RadioButton
import android.widget.TextView
import android.widget.ToggleButton
import java.util.WeakHashMap
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine

// Android View 体系的兜底适配（spec/ui-fallback.md 8.2 Android View 列）：窗口来自 WindowInspector，控件按 View 属性与
// AccessibilityNodeInfo 分类，动作经 performClick / 无障碍动作 / 输入法动作执行。只能在主线程上调用。

/**
 * 嵌入式界面的展开器：命中的 View（如 Compose 的 `AndroidComposeView`）以返回的节点代替其子 View。
 * 返回 null 表示不处理该 View。
 */
interface UiViewExpander {
    fun expand(view: View): List<UiElement>?

    /** [identity] 属于本展开器时返回它是否仍在界面上，否则 null。 */
    fun isLive(identity: Any): Boolean?
}

/** 控件 → 已声明的工具名（[declareMcpTools]）。@invariant 只在主线程访问。 */
private val declaredTools = WeakHashMap<View, MutableList<String>>()

/**
 * 标出控件已有声明的工具：兜底大纲中显示 `[已声明：<tool>]`，提示模型优先调用该工具（spec/ui-fallback.md 8.1）。
 * 标在控件外层的布局上时，标给其子树中唯一的控件。必须在主线程调用。
 *
 * ```kotlin
 * clearButton.declareMcpTools("cart.clear")
 * ```
 */
fun View.declareMcpTools(vararg tools: String) {
    val list = declaredTools.getOrPut(this) { mutableListOf() }
    tools.filter { it !in list }.forEach { list += it }
}

/** 撤销 [declareMcpTools]。 */
fun View.undeclareMcpTools(vararg tools: String) {
    declaredTools[this]?.removeAll(tools.toSet())
}

internal fun declaredOf(view: View): String? = declaredTools[view]?.takeIf { it.isNotEmpty() }?.joinToString(", ")

/** 密码类输入：inputType 为密码变体，或显示为掩码。 */
internal fun isPasswordInput(view: View): Boolean {
    if (view !is TextView) return false
    val type = view.inputType
    val variation = type and InputType.TYPE_MASK_VARIATION
    val textPassword = (type and InputType.TYPE_MASK_CLASS) == InputType.TYPE_CLASS_TEXT &&
        variation in setOf(InputType.TYPE_TEXT_VARIATION_PASSWORD, InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD, InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD)
    val numberPassword = (type and InputType.TYPE_MASK_CLASS) == InputType.TYPE_CLASS_NUMBER && variation == InputType.TYPE_NUMBER_VARIATION_PASSWORD
    return textPassword || numberPassword || view.transformationMethod is PasswordTransformationMethod
}

private fun Context.activity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

private fun keyCode(key: UiKey) = when (key) {
    UiKey.ENTER -> KeyEvent.KEYCODE_ENTER
    UiKey.ESCAPE -> KeyEvent.KEYCODE_ESCAPE
    UiKey.TAB, UiKey.SHIFT_TAB -> KeyEvent.KEYCODE_TAB
    UiKey.SPACE -> KeyEvent.KEYCODE_SPACE
}

/** 把一次按下 + 抬起交给 [root]（经 DecorView → Activity / Dialog → 焦点 View）；返回是否被处理。 */
private fun dispatchKey(root: View, code: Int, meta: Int = 0): Boolean {
    val now = SystemClock.uptimeMillis()
    val down = root.dispatchKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_DOWN, code, 0, meta))
    val up = root.dispatchKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, code, 0, meta))
    return down || up
}

/** Android 平台：窗口来自 [WindowInspector]（API 29+），等一帧用 Choreographer。 */
class AndroidUiPlatform(private val expanders: List<UiViewExpander>, private val settleDelayMillis: Long) : UiPlatform {
    override fun windows(): List<UiWindow> {
        val roots = rootViews().filter { it.isAttachedToWindow && it.visibility == View.VISIBLE }
        // 最上层的模态窗口（Activity 主窗口、对话框、触摸模态的弹出层）遮挡其下全部窗口。
        // @why 不看 windowVisibility：已停止的 Activity 总在更新的 Activity 之下、被这条规则排除；后台时工具已禁用。
        val top = roots.indexOfLast(::isCovering).coerceAtLeast(0)
        return roots.drop(top).map { ViewWindowElement(it, expanders) }
    }

    override fun isLive(identity: Any): Boolean = when (identity) {
        is View -> identity.isAttachedToWindow
        else -> expanders.firstNotNullOfOrNull { it.isLive(identity) } ?: false
    }

    override suspend fun settle() {
        awaitFrame()
        if (settleDelayMillis > 0) {
            delay(settleDelayMillis)
            awaitFrame()
        }
    }

    private suspend fun awaitFrame() = suspendCancellableCoroutine { cont ->
        val callback = android.view.Choreographer.FrameCallback { if (cont.isActive) cont.resumeWith(Result.success(Unit)) }
        val choreographer = android.view.Choreographer.getInstance()
        choreographer.postFrameCallback(callback)
        cont.invokeOnCancellation { choreographer.removeFrameCallback(callback) }
    }

    companion object {
        /** 本进程的全部窗口根 View（添加顺序即 Z 序）。 */
        internal fun rootViews(): List<View> =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) WindowInspector.getGlobalWindowViews() else legacyRootViews()

        /**
         * @compat API 24–28 没有 WindowInspector：读 WindowManagerGlobal.mViews（API 28 为浅灰名单，允许访问并记日志）；
         *   读取失败时返回空列表（兜底工具报告没有可见元素，而不是崩溃）。
         */
        private fun legacyRootViews(): List<View> = try {
            val global = Class.forName("android.view.WindowManagerGlobal")
            val instance = global.getMethod("getInstance").invoke(null)
            val field = global.getDeclaredField("mViews").apply { isAccessible = true }
            @Suppress("UNCHECKED_CAST")
            (field.get(instance) as? List<View>)?.toList() ?: emptyList()
        } catch (_: ReflectiveOperationException) {
            emptyList()
        }

        private fun isCovering(root: View): Boolean {
            val lp = root.layoutParams as? WindowManager.LayoutParams ?: return false
            if (lp.type == WindowManager.LayoutParams.TYPE_BASE_APPLICATION) return true
            val appOrSub = lp.type in WindowManager.LayoutParams.FIRST_APPLICATION_WINDOW..WindowManager.LayoutParams.LAST_SUB_WINDOW
            val passThrough = WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
            return appOrSub && (lp.flags and passThrough) == 0
        }
    }
}

/** 窗口根 View：Activity 主窗口为 `window`，其余（对话框、弹出层）为 `dialog`。 */
class ViewWindowElement internal constructor(val root: View, private val expanders: List<UiViewExpander>) : UiWindow {
    private val lp get() = root.layoutParams as? WindowManager.LayoutParams
    private val isMain get() = lp?.type == WindowManager.LayoutParams.TYPE_BASE_APPLICATION

    override val identity: Any get() = root

    override fun isHidden() = false

    private fun title(): String {
        if (isMain) (root.findViewById<View>(android.R.id.content)?.context ?: root.context).activity()?.let { return it.title?.toString() ?: "" }
        return findTitle(root)?.let { UiOutlineFormat.collapse(it.text) } ?: ""
    }

    /** 对话框标题：资源名为 `title` / `alertTitle` 的可见 TextView（框架与 AppCompat 的 AlertDialog 布局）。 */
    private fun findTitle(v: View): TextView? {
        if (v is TextView && v.isShown && v.id != View.NO_ID && entryName(v) in TITLE_IDS) return v
        if (v !is ViewGroup) return null
        return (0 until v.childCount).firstNotNullOfOrNull { findTitle(v.getChildAt(it)) }
    }

    private fun entryName(v: View): String? = try {
        v.resources.getResourceEntryName(v.id)
    } catch (_: android.content.res.Resources.NotFoundException) {
        null
    }

    private companion object {
        val TITLE_IDS = setOf("title", "alertTitle")
    }

    override fun describe() = UiDescription(UiEntryKind.CONTAINER, if (isMain) "window" else "dialog", title())

    override fun children(): List<UiElement> = ViewUiElement.childrenOf(root, expanders)

    override fun readText() = listOf(title())

    override fun isEnabled() = true

    override suspend fun click() = false

    override fun sendKey(key: UiKey): Boolean =
        dispatchKey(root, keyCode(key), if (key == UiKey.SHIFT_TAB) KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON else 0)

    /** 先交给窗口（Compose 在按键分发中处理 Tab），未处理时按 View 的焦点顺序移动。 */
    override fun moveFocus(forward: Boolean): Boolean {
        if (sendKey(if (forward) UiKey.TAB else UiKey.SHIFT_TAB)) return true
        val group = root as? ViewGroup ?: return false
        val direction = if (forward) View.FOCUS_FORWARD else View.FOCUS_BACKWARD
        val next = FocusFinder.getInstance().findNextFocus(group, root.findFocus(), direction) ?: return false
        return next.requestFocus(direction)
    }

    /** @why 对话框对 Escape 的处理随系统版本不同；未处理时按返回键取消。主窗口不按返回键（会结束 Activity）。 */
    override fun dismiss(): Boolean = !isMain && dispatchKey(root, KeyEvent.KEYCODE_BACK)
}

/** 一个 View 控件。 */
class ViewUiElement internal constructor(
    val view: View,
    private val expanders: List<UiViewExpander>,
    private val inRecycler: Boolean,
) : UiElement {
    override val identity: Any get() = view

    override val reusable: Boolean get() = inRecycler

    override fun isHidden(): Boolean = !view.isShown || view.alpha == 0f || !view.getGlobalVisibleRect(Rect())

    override fun children(): List<UiElement> = childrenOf(view, expanders, inRecycler)

    private fun nodeInfo(): AccessibilityNodeInfo = view.createAccessibilityNodeInfo()

    private val textView get() = view as? TextView

    private fun isTextBox(info: AccessibilityNodeInfo) = view is EditText || (view is TextView && info.isEditable)

    private fun role(info: AccessibilityNodeInfo): String? {
        val cls = info.className?.toString() ?: ""
        return when {
            isTextBox(info) -> "textbox"
            view is AbsSpinner -> "combobox"
            view is AbsSeekBar -> "slider"
            view is RadioButton -> "radio"
            view is ToggleButton || cls.endsWith("Switch") || cls.endsWith("SwitchCompat") -> "switch"
            view is CompoundButton || info.isCheckable -> "checkbox"
            cls.endsWith("ActionBar.Tab") || cls.endsWith("ActionBar\$Tab") -> "tab"
            view is Button || view is ImageButton || cls == Button::class.java.name -> "button"
            else -> null
        }
    }

    private fun ownText(): String = UiOutlineFormat.collapse(textView?.text)

    private fun descendantText(v: View = view, out: MutableList<String> = mutableListOf()): String {
        if (v !== view && (!v.isShown || v.isClickable)) return out.joinToString(" ")
        v.contentDescription?.let { out += it.toString() } ?: (v as? TextView)?.text?.let { out += it.toString() }
        if (v.contentDescription == null && v is ViewGroup) (0 until v.childCount).forEach { descendantText(v.getChildAt(it), out) }
        return UiOutlineFormat.collapse(out.joinToString(" "))
    }

    private fun nameOf(role: String?): String {
        view.contentDescription?.toString()?.takeIf { it.isNotBlank() }?.let { return it }
        if (role == "textbox") return textView?.hint?.toString() ?: ""
        if (role == "combobox" || role == "slider") return ""
        ownText().takeIf { it.isNotEmpty() }?.let { return it }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) view.tooltipText?.toString()?.takeIf { it.isNotBlank() }?.let { return it }
        return if (view is ViewGroup) descendantText() else ""
    }

    private fun valueOf(role: String, secure: Boolean): String? = when {
        secure -> null
        role == "textbox" -> textView?.text?.toString()
        role == "combobox" -> ((view as AdapterView<*>).selectedView as? TextView)?.text?.toString()
            ?: (view as AdapterView<*>).selectedItem?.toString()
        role == "slider" -> (view as AbsSeekBar).progress.toString()
        else -> null
    }

    private fun statesOf(role: String, info: AccessibilityNodeInfo): List<String> = buildList {
        if (!view.isEnabled) add("disabled")
        if (view is Checkable || info.isCheckable) add(if ((view as? Checkable)?.isChecked ?: info.isChecked) "checked" else "unchecked")
        val actions = info.actionList.map { it.id }
        if (AccessibilityNodeInfo.ACTION_COLLAPSE in actions) add("expanded") else if (AccessibilityNodeInfo.ACTION_EXPAND in actions) add("collapsed")
        if (view.isSelected && role in setOf("tab", "generic", "option")) add("selected")
        if (role == "textbox" && !info.isEditable) add("readonly")
        if (textView?.error != null) add("invalid")
        if (view.isFocused) add("focused")
    }

    override fun describe(): UiDescription {
        val info = nodeInfo()
        val declared = declaredOf(view)
        val itemRole = role(info) ?: when {
            view is TextView && view.urls.isNotEmpty() && view.isClickable -> "link"
            view.isClickable || view.hasOnClickListeners() -> "generic"
            else -> null
        }
        if (itemRole != null) {
            val secure = itemRole == "textbox" && (isPasswordInput(view) || info.isPassword)
            return UiDescription(
                UiEntryKind.ITEM, itemRole, nameOf(itemRole),
                value = valueOf(itemRole, secure),
                states = statesOf(itemRole, info),
                secure = secure,
                hasSecureValue = secure && !textView?.text.isNullOrEmpty(),
                declared = declared,
                descend = itemRole == "generic" && view is ViewGroup,
            )
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && info.isHeading && ownText().isNotEmpty()) {
            return UiDescription(UiEntryKind.HEADING, "heading", ownText(), level = 2)
        }
        if (view.accessibilityLiveRegion != View.ACCESSIBILITY_LIVE_REGION_NONE && nameOf(null).isNotEmpty()) {
            return UiDescription(UiEntryKind.ITEM, "status", nameOf(null), declared = declared)
        }
        if (view is ViewGroup) {
            val pane = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) view.accessibilityPaneTitle?.toString() else null
            val role = when {
                view is AdapterView<*> || info.collectionInfo != null -> "list"
                info.isScrollable -> "scrollable"
                !pane.isNullOrBlank() -> "group"
                else -> null
            }
            if (role != null) return UiDescription(UiEntryKind.CONTAINER, role, pane ?: view.contentDescription?.toString() ?: "", declared = declared)
        }
        return UiDescription(null, declared = declared)
    }

    override fun readText(): List<String> {
        if (isPasswordInput(view)) return listOfNotNull(textView?.hint?.toString(), UiOutlineFormat.SECURE_MASK.takeIf { !textView?.text.isNullOrEmpty() })
        return listOfNotNull(view.contentDescription?.toString() ?: textView?.text?.toString())
    }

    override fun readChildren() = view !is EditText

    override fun isEnabled() = view.isEnabled

    override suspend fun click(): Boolean {
        if (view.isClickable || view.hasOnClickListeners()) {
            // @why performClick 只在有 OnClickListener 时返回 true；CompoundButton 等的切换不依赖监听器。
            view.performClick()
            return true
        }
        val actions = nodeInfo().actionList.map { it.id }
        val action = listOf(AccessibilityNodeInfo.ACTION_CLICK, AccessibilityNodeInfo.ACTION_EXPAND, AccessibilityNodeInfo.ACTION_COLLAPSE)
            .firstOrNull { it in actions } ?: return false
        return view.performAccessibilityAction(action, null)
    }

    /** 无障碍 ACTION_SET_TEXT（会触发 TextWatcher）；不接受时（非 EDITABLE 缓冲）退回 setText。 */
    override suspend fun setText(text: String): Boolean {
        val tv = textView ?: return false
        val args = Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) }
        if (!view.performAccessibilityAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)) tv.text = text
        if (tv is EditText) tv.setSelection(tv.text.length)
        return true
    }

    override suspend fun selectOption(text: String): Boolean {
        val spinner = view as? AdapterView<*> ?: return false
        val adapter = spinner.adapter ?: return false
        val labels = (0 until adapter.count).map { adapter.getItem(it)?.toString() ?: "" }
        val index = labels.indexOf(text).takeIf { it >= 0 } ?: labels.indexOfFirst { it.equals(text, ignoreCase = true) }
        if (index < 0) {
            throw UiFallbackInput.invalid("下拉框没有选项「$text」；可选：${labels.filter { it.isNotEmpty() }.take(20).joinToString("、")}", reason = "unsupported")
        }
        spinner.setSelection(index)
        return true
    }

    /** SeekBar：无障碍 SET_PROGRESS（API 24+，回调中 fromUser = true）。 */
    override suspend fun setRange(value: Double): Boolean {
        if (view !is AbsSeekBar) return false
        val args = Bundle().apply { putFloat(AccessibilityNodeInfo.ACTION_ARGUMENT_PROGRESS_VALUE, value.toFloat()) }
        return view.performAccessibilityAction(android.R.id.accessibilityActionSetProgress, args)
    }

    override fun focus(): Boolean = view.isFocused || view.requestFocus()

    override suspend fun imeAction(): Boolean {
        val tv = textView ?: return false
        val action = (tv.imeOptions and android.view.inputmethod.EditorInfo.IME_MASK_ACTION)
            .takeIf { it != android.view.inputmethod.EditorInfo.IME_ACTION_UNSPECIFIED && it != android.view.inputmethod.EditorInfo.IME_ACTION_NONE }
            ?: android.view.inputmethod.EditorInfo.IME_ACTION_DONE
        tv.onEditorAction(action)
        return true
    }

    override suspend fun scrollIntoView(): Boolean = view.requestRectangleOnScreen(Rect(0, 0, view.width, view.height), true)

    override suspend fun scrollPage(direction: UiScrollDirection): Boolean {
        var cur: View? = view
        while (cur != null) {
            // @why 只认容器：内容略高于自身的 TextView（按钮文字）也报告可滚动，但不是滚动区。
            if (cur is ViewGroup && canScroll(cur, direction)) return scrollOnePage(cur, direction)
            cur = cur.parent as? View
        }
        return false
    }

    companion object {
        internal fun childrenOf(view: View, expanders: List<UiViewExpander>, inRecycler: Boolean = false): List<UiElement> {
            expanders.firstNotNullOfOrNull { it.expand(view) }?.let { return it }
            if (view !is ViewGroup) return emptyList()
            // 列表容器复用子 View 显示不同的数据项：其下的控件按指纹核对引用（spec/ui-fallback.md 第 3 节）。
            val recycled = inRecycler || view is AdapterView<*> || view.javaClass.name.endsWith("RecyclerView")
            return (0 until view.childCount).map { ViewUiElement(view.getChildAt(it), expanders, recycled) }
        }

        private fun canScroll(v: View, d: UiScrollDirection) = when (d) {
            UiScrollDirection.DOWN -> v.canScrollVertically(1)
            UiScrollDirection.UP -> v.canScrollVertically(-1)
            UiScrollDirection.RIGHT -> v.canScrollHorizontally(1)
            UiScrollDirection.LEFT -> v.canScrollHorizontally(-1)
        }

        /**
         * 滚动一页（视口大小）。
         * @why 不用无障碍滚动动作：ScrollView 等对其做平滑滚动，变化摘要会取到动画中途的状态；scrollBy 立即生效且由容器夹紧到边界。
         */
        private fun scrollOnePage(v: View, d: UiScrollDirection): Boolean {
            val sign = if (d == UiScrollDirection.DOWN || d == UiScrollDirection.RIGHT) 1 else -1
            when {
                d == UiScrollDirection.DOWN || d == UiScrollDirection.UP ->
                    if (v is AbsListView) v.scrollListBy(sign * v.height) else v.scrollBy(0, sign * v.height)
                else -> v.scrollBy(sign * v.width, 0)
            }
            return true
        }
    }
}
