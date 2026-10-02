package dev.appmcp.android.uifallback

import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.ProcessLifecycleOwner
import dev.appmcp.AppMcpRegistrar
import dev.appmcp.ErrorKind
import dev.appmcp.Risk
import dev.appmcp.Scope
import dev.appmcp.ToolAnnotations
import dev.appmcp.ToolCallException
import dev.appmcp.ToolHandle
import dev.appmcp.ToolSurface
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

/**
 * [AndroidUiFallback.enable] 的选项。
 *
 * @property prefix 工具名前缀（`ui.outline` 等）
 * @property maxItems `outline` 缺省列出的控件数
 * @property settleDelayMillis 操作后在一帧之外再等的时长（再取变化摘要）
 * @property expanders 嵌入式界面（如 Compose 的 `AndroidComposeView`）的展开器：命中的 View 以其返回的节点代替子 View；
 *   Compose 传 `dev.appmcp.compose.uifallback.ComposeUiExpander`
 */
data class UiFallbackOptions @JvmOverloads constructor(
    val prefix: String = "ui",
    val maxItems: Int = 60,
    val settleDelayMillis: Long = 50,
    val expanders: List<UiViewExpander> = emptyList(),
)

/**
 * Android 进程内控件兜底（spec/ui-fallback.md，第 4c 项 H）：开发者显式开启后注册
 * `<prefix>.outline` / `click` / `fill` / `press` / `scroll` / `read`，经 View 与无障碍接口（Compose 经语义树）直接操作控件，
 * 不截图、不按坐标。
 *
 * - 工具以 `surface = VIEW` 注册，只在 App 有可见 Activity（进程生命周期至少 STARTED）时启用；
 * - 每个动作在主线程上按引用重新取控件，核对仍在界面上、可见、启用；
 * - 密码类输入框（inputType 为密码 / Compose `Modifier.semantics { password() }`）的值只显示 `••••`，拒绝填写与按键；
 * - 用 [declareMcpTools]（View）或 `Modifier.mcpDeclared`（Compose）标出的控件在大纲中显示 `[已声明：…]`。
 *
 * ```kotlin
 * if (BuildConfig.DEBUG) fallback = AndroidUiFallback.enable(client)   // fallback.close() 注销全部兜底工具
 * ```
 *
 * @invariant [enable] 与 [close] 必须在主线程调用（登记生命周期观察者）。
 */
class AndroidUiFallback internal constructor(
    private val scope: Scope,
    val inspector: UiInspector,
    private val lifecycle: Lifecycle,
    private val uiDispatcher: CoroutineDispatcher,
) : AutoCloseable {
    private val tools = mutableListOf<ToolHandle>()

    @Volatile private var visible = false

    @Volatile private var closed = false

    private val observer = LifecycleEventObserver { _, _ -> recompute() }

    /** 兜底工具当前是否启用（App 有可见 Activity）。 */
    val enabled: Boolean get() = !closed && visible

    private fun recompute() {
        val want = lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
        if (closed || want == visible) return
        visible = want
        tools.forEach { it.setEnabled(want) }
        if (!want) inspector.clear()
    }

    private fun guarded(run: suspend (JsonObject) -> JsonObject): suspend (JsonObject, dev.appmcp.ToolContext) -> Any? = { args, _ ->
        withContext(uiDispatcher) {
            if (!enabled) throw ToolCallException(ErrorKind.TOOL_DISABLED, "应用当前没有可见窗口，兜底工具不可用")
            run(args)
        }
    }

    private fun register(prefix: String) {
        fun add(name: String, title: String, description: String, schema: JsonObject, readOnly: Boolean, run: suspend (JsonObject) -> JsonObject) {
            tools += scope.tool(
                "$prefix.$name", description, schema,
                risk = if (readOnly) Risk.READ else Risk.WRITE,
                title = title,
                enabled = false,
                annotations = ToolAnnotations(title = title, readOnlyHint = readOnly),
                surface = ToolSurface.VIEW,
                handler = guarded(run),
            )
        }
        val ui = inspector
        val input = UiFallbackInput
        add(
            "outline", "界面控件大纲",
            "兜底能力：列出当前窗口可见的可交互控件（按钮、输入框、复选框等），每行一个，带引用 eN，按窗口 / 对话框 / 分组归类。" +
                "应用已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / scroll / read。",
            UiFallbackInput.Schemas.outline(ui.maxItems), readOnly = true,
        ) { a -> ui.outline(input.string(a, "query"), input.ref(a, "within", required = false), input.int(a, "limit")).toJson() }
        add(
            "click", "点击控件", "兜底能力：激活 $prefix.outline 中的控件（点击按钮、切换复选框、选中选项、展开 / 收起），返回界面变化摘要。",
            UiFallbackInput.Schemas.ref, readOnly = false,
        ) { a -> ui.click(input.ref(a, "ref")!!).toJson() }
        add(
            "fill", "填写控件",
            "兜底能力：填写文本框；复选框 / 开关 / 单选框传 true / false；下拉框传选项文本；滑块传数字。密码类控件不支持。返回界面变化摘要。",
            UiFallbackInput.Schemas.fill, readOnly = false,
        ) { a -> ui.fill(input.ref(a, "ref")!!, input.value(a)).toJson() }
        add(
            "press", "按键",
            "兜底能力：按键（ref 缺省为当前焦点控件）：Enter（提交文本框 / 激活）、Escape（关闭对话框）、Tab / Shift+Tab（移动焦点）、Space（激活）。" +
                "输入文本请用 fill。返回界面变化摘要。",
            UiFallbackInput.Schemas.press, readOnly = false,
        ) { a -> ui.press(input.ref(a, "ref", required = false), input.string(a, "key") ?: throw input.invalid("缺少参数 key")).toJson() }
        add(
            "scroll", "滚动",
            "兜底能力：无 direction 时把控件滚动到可见；direction 为 up / down / left / right 时滚动该控件所在的滚动区一页（down = 向下翻看更多内容）。返回界面变化摘要。",
            UiFallbackInput.Schemas.scroll, readOnly = false,
        ) { a -> ui.scroll(input.ref(a, "ref")!!, input.string(a, "direction")).toJson() }
        add(
            "read", "读取控件文本",
            "兜底能力：读取控件的可见文本（折叠空白，默认最多 ${UiOutlineFormat.READ_DEFAULT} 字）；ref 缺省为全部窗口。",
            UiFallbackInput.Schemas.read, readOnly = true,
        ) { a -> ui.read(input.ref(a, "ref", required = false), input.int(a, "maxChars")).toJson() }
    }

    /** 注销全部兜底工具（幂等）。 */
    override fun close() {
        if (closed) return
        closed = true
        lifecycle.removeObserver(observer)
        inspector.clear()
        tools.clear()
        scope.dispose()
    }

    companion object {
        /**
         * 在 [registrar]（客户端或作用域）下建子作用域 `<prefix>-fallback` 并注册兜底工具。必须在主线程调用。
         *
         * @input lifecycle 判断"有可见窗口"的生命周期，缺省为 `ProcessLifecycleOwner`（任一 Activity 已 STARTED）
         */
        @JvmStatic
        @JvmOverloads
        fun enable(
            registrar: AppMcpRegistrar,
            options: UiFallbackOptions = UiFallbackOptions(),
            lifecycle: Lifecycle = ProcessLifecycleOwner.get().lifecycle,
        ): AndroidUiFallback {
            val platform = AndroidUiPlatform(options.expanders, options.settleDelayMillis)
            val inspector = UiInspector(platform, options.prefix, options.maxItems.coerceAtLeast(1))
            return start(registrar, options.prefix, inspector, lifecycle, Dispatchers.Main.immediate)
        }

        internal fun start(
            registrar: AppMcpRegistrar,
            prefix: String,
            inspector: UiInspector,
            lifecycle: Lifecycle,
            uiDispatcher: CoroutineDispatcher,
        ): AndroidUiFallback {
            val fallback = AndroidUiFallback(registrar.scope("$prefix-fallback"), inspector, lifecycle, uiDispatcher)
            try {
                fallback.register(prefix)
                lifecycle.addObserver(fallback.observer)
                fallback.recompute()
                return fallback
            } catch (e: Throwable) {
                fallback.close()
                throw e
            }
        }
    }
}
