package dev.appmcp.android.uifallback

// 兜底引擎测试用的假控件树（不依赖 Android 框架）。

/** 可变的假控件：测试直接改字段模拟界面变化。 */
open class FakeElement(
    var role: String?,
    var name: String = "",
    var kind: UiEntryKind? = if (role == null) null else UiEntryKind.ITEM,
    var value: String? = null,
    var states: MutableList<String> = mutableListOf(),
    var secure: Boolean = false,
    var declared: String? = null,
    var hidden: Boolean = false,
    var enabled: Boolean = true,
    val children: MutableList<FakeElement> = mutableListOf(),
) : UiElement {
    /** 模拟框架复用对象：改为另一个身份。 */
    var id: Any = Any()
    val log = mutableListOf<String>()
    var onClick: () -> Unit = {}
    var scrollable = false
    override var reusable = false

    override val identity: Any get() = id

    override fun isHidden() = hidden

    override fun describe() = UiDescription(
        kind, role ?: "generic", name, value, states.toList(),
        secure = secure, hasSecureValue = secure && !value.isNullOrEmpty(), declared = declared,
        level = if (kind == UiEntryKind.HEADING) 2 else null,
    )

    override fun children(): List<UiElement> = children

    override fun readText() = listOfNotNull(name, if (secure) UiOutlineFormat.SECURE_MASK.takeIf { !value.isNullOrEmpty() } else value)

    override fun isEnabled() = enabled

    override suspend fun click(): Boolean {
        log += "click"
        onClick()
        return true
    }

    override suspend fun setText(text: String): Boolean {
        log += "setText:$text"
        value = text
        return true
    }

    override fun focus(): Boolean {
        log += "focus"
        return true
    }

    override suspend fun imeAction(): Boolean {
        log += "ime"
        return true
    }

    override suspend fun scrollIntoView(): Boolean {
        log += "scrollIntoView"
        hidden = false
        return true
    }

    override suspend fun scrollPage(direction: UiScrollDirection): Boolean {
        log += "scroll:${direction.wire}"
        return scrollable
    }
}

class FakeWindow(name: String, role: String = "window") : FakeElement(role, name, UiEntryKind.CONTAINER), UiWindow {
    val keys = mutableListOf<UiKey>()
    var handlesKeys = false

    override fun sendKey(key: UiKey): Boolean {
        keys += key
        return handlesKeys
    }

    override fun moveFocus(forward: Boolean): Boolean {
        keys += if (forward) UiKey.TAB else UiKey.SHIFT_TAB
        return true
    }

    override fun dismiss(): Boolean {
        log += "dismiss"
        return role == "dialog"
    }
}

class FakePlatform(vararg windows: FakeWindow) : UiPlatform {
    val windows = windows.toMutableList()
    var settles = 0

    override fun windows(): List<UiWindow> = windows

    override fun isLive(identity: Any): Boolean {
        fun has(e: FakeElement): Boolean = e.id === identity || e.children.any(::has)
        return windows.any(::has)
    }

    override suspend fun settle() {
        settles++
    }
}
