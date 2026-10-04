package dev.appmcp

import dev.appmcp.ffi.ToolSpec
import kotlinx.serialization.json.JsonObject
import dev.appmcp.ffi.Resource as FfiResource
import dev.appmcp.ffi.Tool as FfiTool

/** 已注册的工具。 */
class ToolHandle internal constructor(private val inner: FfiTool, @Volatile private var spec: ToolSpec) {
    val name: String get() = inner.name()

    fun setEnabled(enabled: Boolean) {
        synchronized(this) {
            inner.setEnabled(enabled)
            // @why update() 整体替换定义，须记住当前启用状态，否则之后的 update 会把它改回去（view 工具随可见性切换）
            spec = spec.copy(enabled = enabled)
        }
    }

    /**
     * 修改定义；为空的参数保持不变（无法清除声明——清除用 `update { annotations = null }`）。
     */
    fun update(
        description: String? = null,
        inputSchema: JsonObject? = null,
        risk: Risk? = null,
        title: String? = null,
        annotations: ToolAnnotations? = null,
        outputSchema: JsonObject? = null,
        surface: ToolSurface? = null,
        page: String? = null,
        backgroundTool: String? = null,
        concurrency: Int? = null,
        exclusive: String? = null,
        implements: List<String>? = null,
        cache: CachePolicy? = null,
        deprecated: Deprecation? = null,
        undoable: Boolean? = null,
    ) = update {
        description?.let { this.description = it }
        inputSchema?.let { this.inputSchema = it }
        risk?.let { this.risk = it }
        title?.let { this.title = it }
        annotations?.let { this.annotations = it }
        outputSchema?.let { this.outputSchema = it }
        surface?.let { this.surface = it }
        page?.let { this.page = it }
        backgroundTool?.let { this.backgroundTool = it }
        concurrency?.let { this.concurrency = it }
        exclusive?.let { this.exclusive = it }
        implements?.let { this.implements = it }
        cache?.let { this.cache = it }
        deprecated?.let { this.deprecated = it }
        undoable?.let { this.undoable = it }
    }

    /**
     * 按补丁修改定义：赋值过的字段替换，**赋值 null 清除该声明**，未赋值的保持不变（与网页 / Rust SDK 一致）。
     *
     * ```kotlin
     * handle.update { description = "新描述"; annotations = null; cache = null; deprecated = null }
     * ```
     */
    fun update(change: ToolUpdate.() -> Unit) {
        synchronized(this) {
            val next = ToolUpdate().apply(change).applyTo(spec)
            inner.update(next)
            spec = next
        }
    }

    /** 注销工具（幂等）。 */
    fun dispose() = inner.dispose()

    internal fun specForTest(): ToolSpec = spec
}

/** 已注册的资源。 */
class ResourceHandle internal constructor(private val inner: FfiResource) {
    val name: String get() = inner.name()
    fun notifyChanged() = inner.notifyChanged()
    fun dispose() = inner.dispose()
}
