package dev.appmcp.agent

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.ParcelFileDescriptor
import dev.appmcp.binder.ChannelOpenException
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.ServiceChannelDialer
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import java.io.Closeable

/**
 * Agent 侧客户端（TASKS 4g f）：绑定独立 Hub App 的 `dev.appmcp.HUB` Service（低优先级标志，Hub App 未运行即由系统
 * 激活），Binder 上换得一个 socketpair fd，在其上跑 MCP（[McpLineClient]）。Hub 再按名寻址绑定目标 App
 * （spec/naming.md 4.2）——Agent 只看到 MCP 的 list / call。
 *
 * ```kotlin
 * HubClient.connect(context).use { hub ->
 *     hub.initialize("my-agent")
 *     val tools = hub.listTools()
 *     val result = hub.callTool("sample-android.demo.echo", buildJsonObject { put("text", "hi") })
 * }
 * ```
 *
 * 用完即 [close]：关闭 fd、解除绑定，Hub App 没有其他绑定时由系统回收（召之即来，挥之即去）。
 */
class HubClient private constructor(
    private val session: McpLineClient,
    private val dialed: ServiceChannelDialer.Dialed,
    /** 绑定的 Hub App 组件。 */
    val component: ComponentName,
) : Closeable {

    suspend fun initialize(clientName: String, clientVersion: String = "0.1.0"): JsonObject =
        io { session.initialize(clientName, clientVersion) }

    suspend fun listTools(): List<JsonObject> = io { session.listTools() }

    /** 调用工具；工具失败体现在结果的 `isError`。 */
    suspend fun callTool(name: String, arguments: JsonObject = JsonObject(emptyMap())): JsonObject =
        io { session.callTool(name, arguments) }

    /** 任意 MCP 请求（如 `resources/read`）。 */
    suspend fun request(method: String, params: JsonElement?): JsonElement = io { session.request(method, params) }

    /** 关闭会话与 fd，解除绑定。幂等。 */
    override fun close() {
        session.close()
        dialed.release()
    }

    private suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

    companion object {
        /** 独立 Hub App 的 Intent 动作。 */
        const val ACTION_HUB = "dev.appmcp.HUB"

        /**
         * 查找并绑定 Hub App。[packageName] 为空时取第一个声明了 [ACTION_HUB] 的导出 Service（按包名排序）。
         *
         * @error 没有 Hub App → [ChannelOpenException]（`NAME_NOT_FOUND`）；绑定被拒 / 超时 → 对应的码。
         */
        suspend fun connect(context: Context, packageName: String? = null, timeoutMillis: Long = 15_000): HubClient =
            withContext(Dispatchers.IO) {
                val app = context.applicationContext
                val component = findHub(app, packageName)
                    ?: throw ChannelOpenException("NAME_NOT_FOUND", "没有找到 Hub App（未安装声明 $ACTION_HUB 的应用）")
                val dialed = ServiceChannelDialer(app)
                    .dial(Intent(ACTION_HUB).setComponent(component), FdChannel.HUB_DESCRIPTOR, timeoutMillis)
                try {
                    val readSide = dialed.fd.dup()
                    val session = McpLineClient(
                        ParcelFileDescriptor.AutoCloseInputStream(readSide),
                        ParcelFileDescriptor.AutoCloseOutputStream(dialed.fd),
                    )
                    HubClient(session, dialed, component)
                } catch (e: Exception) {
                    runCatching { dialed.fd.close() }
                    dialed.release()
                    throw e
                }
            }

        /** 声明了 [ACTION_HUB] 的导出 Service。 */
        @JvmStatic
        fun findHub(context: Context, packageName: String? = null): ComponentName? {
            val intent = Intent(ACTION_HUB).apply { packageName?.let(::setPackage) }
            val pm = context.packageManager
            @Suppress("DEPRECATION")
            val found = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                pm.queryIntentServices(intent, PackageManager.ResolveInfoFlags.of(0))
            } else {
                pm.queryIntentServices(intent, 0)
            }
            return found.mapNotNull { it.serviceInfo }
                .filter { it.exported }
                .minByOrNull { it.packageName }
                ?.let { ComponentName(it.packageName, it.name) }
        }
    }
}
