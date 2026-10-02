package dev.appmcp.hubapp

import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.util.Log
import dev.appmcp.binder.BinderCaller
import dev.appmcp.binder.ChannelOpenException
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.FdChannelBinder
import dev.appmcp.binder.PackageIdentity
import dev.appmcp.hub.Hub
import dev.appmcp.hub.HubConfig
import dev.appmcp.hub.android.AndroidNameService
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.io.File

/**
 * 独立 Hub App 的 Agent 入口（TASKS 4g d）。`onBind` 只返回 Binder；Agent 在 `onServiceConnected` 后调用一次 `open()`：
 * 记录调用方身份（uid → 包名、签名证书），创建 socketpair，一端交给 Hub 的 MCP 出口（[McpBackend]），另一端返回 Agent。
 *
 * - Hub 惰性启动：第一个 Agent `open()` 时才创建（按名寻址的 [AndroidNameService]，不开 HTTP / IPC 端口）；
 * - 不常驻：只有绑定、没有 `startService`；最后一个 Agent 解绑后系统销毁本 Service（[onDestroy] 关闭 Hub、解除对全部 App 的绑定），
 *   进程回到缓存态由系统回收；不加前台服务、WakeLock、定时器。
 *
 * TODO（第 16 项 P1 Agent 身份 / P2 策略）：用户授权的 Agent 名单。当前只记录调用方身份（[AgentLog]），全部放行。
 */
class HubService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + CoroutineName("hub-app-mcp"))

    @Volatile private var backend: McpBackend? = null

    override fun onBind(intent: Intent?): IBinder? {
        if (intent?.action != ACTION_HUB) return null
        return FdChannelBinder(FdChannel.HUB_DESCRIPTOR) { caller, _ -> open(caller) }
    }

    /** 在 Binder 线程上、调用方事务内执行。 */
    internal fun open(caller: BinderCaller): ParcelFileDescriptor {
        val identity = PackageIdentity.of(packageManager, caller.uid)
        AgentLog.record(identity)
        Log.i(TAG, "Agent 连接：uid=${identity.uid} 包=${identity.packages} 证书=${identity.certificates}")
        val mcp = ensureBackend()
        val (hubEnd, agentEnd) = channelPairs.create()
        scope.launch {
            runCatching { mcp.serve(hubEnd) }
                .onSuccess { Log.i(TAG, "Agent 会话结束：uid=${identity.uid}") }
                .onFailure { Log.w(TAG, "Agent 会话异常结束：${it.message}") }
        }
        return agentEnd
    }

    @Synchronized
    private fun ensureBackend(): McpBackend = backend ?: try {
        backendFactory(this).also { backend = it }
    } catch (e: Exception) {
        Log.e(TAG, "无法启动 Hub", e)
        throw ChannelOpenException("ACTIVATION_DENIED", "Hub 启动失败：${e.message}", e)
    }

    override fun onDestroy() {
        scope.cancel()
        synchronized(this) {
            backend?.close()
            backend = null
        }
        Log.i(TAG, "没有 Agent 绑定，Hub 已关闭")
        super.onDestroy()
    }

    companion object {
        const val ACTION_HUB = "dev.appmcp.HUB"
        private const val TAG = "AppWireHub"

        /** 创建 MCP 后端（测试可替换；默认内嵌 Hub + Android 名字服务）。 */
        @Volatile
        internal var backendFactory: (Context) -> McpBackend = ::EmbeddedHub

        /** 创建 socketpair：`first` 交给 Hub，`second` 返回 Agent（测试可替换）。 */
        @Volatile
        internal var channelPairs: ChannelPairs = ChannelPairs {
            val p = ParcelFileDescriptor.createSocketPair()
            p[0] to p[1]
        }
    }
}

/** socketpair 工厂。 */
fun interface ChannelPairs {
    fun create(): Pair<ParcelFileDescriptor, ParcelFileDescriptor>
}

/** fd 上的 MCP 出口。 */
interface McpBackend : AutoCloseable {
    /** 在 [end] 上提供 MCP，挂起到对端关闭（接管 [end]）。 */
    suspend fun serve(end: ParcelFileDescriptor)
}

/** 默认后端：内嵌 Hub，按名寻址连接本机 App（spec/naming.md 4.2），休眠快照存在 App 私有目录（重启后仍可列出）。 */
internal class EmbeddedHub(context: Context) : McpBackend {
    private val names = AndroidNameService(context)
    private val hub: Hub = names.start(
        HubConfig(
            enableListen = false,
            enableIpc = false,
            stateDir = File(context.filesDir, "hub-state").absolutePath,
        ),
    )

    override suspend fun serve(end: ParcelFileDescriptor) = hub.serveMcpFd(end.detachFd())

    override fun close() {
        names.close()
        hub.close()
    }
}

/** 最近连接过的 Agent（调用方身份，诊断用；只在内存中，最多 [CAPACITY] 条）。 */
object AgentLog {
    private const val CAPACITY = 16
    private val entries = ArrayDeque<PackageIdentity>()

    @Synchronized
    fun record(identity: PackageIdentity) {
        if (entries.size >= CAPACITY) entries.removeFirst()
        entries.addLast(identity)
    }

    @Synchronized
    fun recent(): List<PackageIdentity> = entries.toList()
}
