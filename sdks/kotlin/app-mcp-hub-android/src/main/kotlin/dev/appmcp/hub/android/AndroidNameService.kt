package dev.appmcp.hub.android

import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.content.pm.ResolveInfo
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import dev.appmcp.binder.BlockedTarget
import dev.appmcp.binder.ChannelOpenException
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.NamingCodes
import dev.appmcp.binder.ServiceChannelDialer
import dev.appmcp.hub.ffi.DialOutcome
import dev.appmcp.hub.Hub
import dev.appmcp.hub.HubConfig
import dev.appmcp.hub.HubNameService
import dev.appmcp.hub.NamedApp
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * Android 名字服务（spec/naming.md 4.2）：Hub 一侧的发现与拨号。
 *
 * - **发现**：`queryIntentServices(Intent("dev.appmcp.TOOLS"), GET_META_DATA)` 一次性枚举，读 Service 的
 *   `<meta-data android:name="dev.appmcp.manifest">` 指向的清单资源（PackageManager 读目标包的资源，不经 App 进程）；
 *   appId 取自清单（不由包名推导）。Hub 运行期间以**动态注册**的接收器收包变更广播增量更新（[attach]），无轮询。
 *   Hub 清单须声明 `<queries><intent><action android:name="dev.appmcp.TOOLS"/></intent></queries>`（本库清单已带），
 *   不申请 `QUERY_ALL_PACKAGES`。
 * - **拨号**：以 [ServiceChannelDialer.lowImpactBindFlags] `bindService` 显式组件（未运行即由系统激活）→ `open()` 换得 fd →
 *   交给 Rust Hub；Hub 宽限后关闭通道时回调 [release] → `unbindService`，进程回到缓存态由系统冻结 / 回收。
 * - **身份**：同一 appId 由多个包声明时只认第一个（按包名排序；其余记录警告），拨号时要求通道对端 uid = 该包 uid
 *   （spec/naming.md 10.3）。
 *
 * 用法：`val names = AndroidNameService(context); val hub = names.start(HubConfig(...))`；不用时 `names.close()` 再 `hub.close()`。
 */
class AndroidNameService(context: Context) : HubNameService, AutoCloseable {
    private val context = context.applicationContext
    private val pm: PackageManager = this.context.packageManager
    private val dialer = ServiceChannelDialer(this.context)

    /** appId → 登记（包名、Service 组件、uid）。 */
    private val apps = ConcurrentHashMap<String, Registered>()

    /** 拨号持有的绑定：租约号 → 解绑句柄。 */
    private val leases = ConcurrentHashMap<Long, ServiceChannelDialer.Dialed>()
    private val nextLease = AtomicLong(0)
    private val closed = AtomicBoolean(false)

    @Volatile private var sink: NameEventSink? = null

    /** 把 fd 的所有权交出（交给 Rust Hub；测试可替换）。 */
    internal var detach: (ParcelFileDescriptor) -> Int = { it.detachFd() }

    /** 读 Service 指向的清单（测试可替换）。 */
    internal var manifestReader: (ServiceInfo) -> String? = ::readManifest

    @Volatile private var receiver: BroadcastReceiver? = null

    /** 一个已发现的 App。 */
    data class Registered(val appId: String, val component: ComponentName, val uid: Int)

    /** 当前持有的绑定数（诊断 / 测试：宽限后应为 0，spec/naming.md 7.7）。 */
    val boundCount: Int get() = leases.size

    /** 启动以本名字服务按名寻址的 Hub，并开始接收包变更（[attach]）。 */
    fun start(config: HubConfig): Hub = Hub.startWithNameService(config, KIND, this).also(::attach)

    /** 包变更的去向（默认为 Hub：[Hub.nameServiceInstalled] / [Hub.nameServiceRemoved]）。 */
    interface NameEventSink {
        fun installed(app: NamedApp)
        fun removed(appId: String)
    }

    /** 把包变更（安装 / 更新 / 卸载）推送给 [hub]：动态注册接收器，[close] 时注销。 */
    fun attach(hub: Hub) = attach(object : NameEventSink {
        override fun installed(app: NamedApp) = hub.nameServiceInstalled(app)
        override fun removed(appId: String) = hub.nameServiceRemoved(appId)
    })

    /** 同 [attach]，推送给任意去向。 */
    fun attach(sink: NameEventSink) {
        this.sink = sink
        if (receiver != null) return
        val r = object : BroadcastReceiver() {
            override fun onReceive(c: Context, intent: Intent) {
                val pkg = intent.data?.schemeSpecificPart ?: return
                val replacing = intent.getBooleanExtra(Intent.EXTRA_REPLACING, false)
                onPackageChanged(pkg, removed = intent.action == Intent.ACTION_PACKAGE_REMOVED && !replacing ||
                    intent.action == Intent.ACTION_PACKAGE_FULLY_REMOVED)
            }
        }
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_PACKAGE_ADDED)
            addAction(Intent.ACTION_PACKAGE_REPLACED)
            addAction(Intent.ACTION_PACKAGE_CHANGED)
            addAction(Intent.ACTION_PACKAGE_REMOVED)
            addAction(Intent.ACTION_PACKAGE_FULLY_REMOVED)
            addDataScheme("package")
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(r, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(r, filter)
        }
        receiver = r
    }

    /** 包变更：重新读该包的元数据；卸载或不再声明时移除其 App。 */
    internal fun onPackageChanged(pkg: String, removed: Boolean) {
        val target = sink ?: return
        val before = apps.values.filter { it.component.packageName == pkg }.map { it.appId }.toSet()
        val now = if (removed) emptyList() else scan(Intent(ACTION_TOOLS).setPackage(pkg))
        for (app in now) target.installed(app)
        for (appId in before - now.map { it.appId }.toSet()) {
            apps.remove(appId)
            target.removed(appId)
        }
    }

    override fun discover(): List<NamedApp> = scan(Intent(ACTION_TOOLS))

    /** 枚举声明了 `dev.appmcp.TOOLS` 的 Service（只读元数据），登记并返回。 */
    private fun scan(query: Intent): List<NamedApp> {
        val found = queryServices(query)
            .mapNotNull { it.serviceInfo }
            .filter { it.exported && it.applicationInfo.enabled }
            .sortedBy { it.packageName }
        val out = mutableListOf<NamedApp>()
        for (info in found) {
            val manifest = manifestReader(info) ?: continue
            val appId = appIdOf(manifest) ?: run {
                Log.w(TAG, "${info.packageName} 的清单没有合法的 appId，忽略")
                continue
            }
            val component = ComponentName(info.packageName, info.name)
            val existing = apps[appId]
            if (existing != null && existing.component.packageName != info.packageName && isInstalled(existing.component.packageName)) {
                Log.w(TAG, "appId $appId 已由 ${existing.component.packageName} 声明，忽略 ${info.packageName}（spec/naming.md 10.3）")
                continue
            }
            apps[appId] = Registered(appId, component, info.applicationInfo.uid)
            out += NamedApp(appId, true, false, component.flattenToShortString(), manifest)
        }
        return out
    }

    private fun isInstalled(pkg: String): Boolean = runCatching { pm.getPackageInfo(pkg, 0) }.isSuccess

    @Suppress("DEPRECATION")
    private fun queryServices(intent: Intent): List<ResolveInfo> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.queryIntentServices(intent, PackageManager.ResolveInfoFlags.of(PackageManager.GET_META_DATA.toLong()))
        } else {
            pm.queryIntentServices(intent, PackageManager.GET_META_DATA)
        }

    /** 读 `<meta-data android:name="dev.appmcp.manifest">` 指向的资源（不超过 [MAX_MANIFEST_BYTES]）。 */
    private fun readManifest(info: ServiceInfo): String? {
        val resId = info.metaData?.getInt(META_MANIFEST, 0) ?: 0
        if (resId == 0) {
            Log.i(TAG, "${info.packageName}/${info.name} 没有 $META_MANIFEST 元数据，跳过")
            return null
        }
        return runCatching {
            pm.getResourcesForApplication(info.applicationInfo).openRawResource(resId).use { input ->
                val bytes = input.readNBytesCompat(MAX_MANIFEST_BYTES + 1)
                require(bytes.size <= MAX_MANIFEST_BYTES) { "清单超过 $MAX_MANIFEST_BYTES 字节" }
                bytes.decodeToString()
            }
        }.onFailure { Log.w(TAG, "无法读取 ${info.packageName} 的清单：${it.message}") }.getOrNull()
    }

    override fun dial(appId: String, timeoutMs: ULong): DialOutcome {
        val app = apps[appId] ?: return DialOutcome.Failed(NamingCodes.NAME_NOT_FOUND, "没有发现 App「$appId」（未安装或未声明 $ACTION_TOOLS）")
        if (closed.get()) return DialOutcome.Failed(NamingCodes.ACTIVATION_DENIED, "名字服务已关闭")
        val intent = Intent(ACTION_TOOLS).setComponent(app.component)
        return try {
            val dialed = dialer.dial(intent, FdChannel.APP_TOOLS_DESCRIPTOR, timeoutMs.toLong().coerceAtLeast(1))
            val lease = nextLease.incrementAndGet()
            leases[lease] = dialed
            DialOutcome.Channel(detach(dialed.fd), lease.toULong(), app.uid.toUInt())
        } catch (e: ChannelOpenException) {
            Log.w(TAG, "拨号 ${app.component.flattenToShortString()} 失败：$e")
            val blocked = e.blocked
            if (blocked == null) return DialOutcome.Failed(e.code, settingsHint(e))
            notifyBlocked(blocked)
            DialOutcome.Blocked(blocked.packageName, blocked.appLabel, "系统拒绝绑定 ${app.component.flattenToShortString()}")
        }
    }

    /**
     * 系统拦截了对某个 App 的绑定（[NamingCodes.ACTIVATION_BLOCKED]）时回调（在 Hub 的阻塞线程上；如独立 Hub App 据此发通知）。
     * 调用方仍会收到 `USER_ACTION_REQUIRED`；本回调只用于额外的提示，异常被记录后忽略。
     */
    @Volatile
    var onBlocked: ((BlockedTarget) -> Unit)? = null

    private fun notifyBlocked(target: BlockedTarget) {
        val listener = onBlocked ?: return
        runCatching { listener(target) }.onFailure { Log.w(TAG, "onBlocked 回调异常：${it.message}") }
    }

    override fun release(lease: ULong) {
        leases.remove(lease.toLong())?.release()
    }

    /** 注销包变更接收器，解除全部绑定（Hub 退出前调用；系统在进程退出时也会解除）。幂等。 */
    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        receiver?.let { runCatching { context.unregisterReceiver(it) } }
        receiver = null
        sink = null
        for (lease in leases.keys.toList()) leases.remove(lease)?.release()
    }

    companion object {
        private const val TAG = "AppMcpHubNames"
        const val KIND = "android"
        const val ACTION_TOOLS = "dev.appmcp.TOOLS"
        const val META_MANIFEST = "dev.appmcp.manifest"

        /** 清单资源上限（spec/manifest.md 的清单远小于此值；防止异常资源占用内存）。 */
        const val MAX_MANIFEST_BYTES = 512 * 1024

        private val json = Json { ignoreUnknownKeys = true }
        private val APP_ID = Regex("[a-z][a-z0-9-]{0,62}")

        /** 清单中的 appId（不合法时为 null）。 */
        @JvmStatic
        fun appIdOf(manifestJson: String): String? = runCatching {
            json.parseToJsonElement(manifestJson).jsonObject["appId"]?.jsonPrimitive?.content
        }.getOrNull()?.takeIf { APP_ID.matches(it) }

        /**
         * 失败说明附带处理建议。系统拦截（`ACTIVATION_BLOCKED`）不经这里：以 [DialOutcome.Blocked] 交给 Hub，
         * 由 Hub 生成面向用户的 `USER_ACTION_REQUIRED`（spec/protocol.md 第 4 节）。
         */
        @JvmStatic
        fun settingsHint(e: ChannelOpenException): String = when (e.code) {
            NamingCodes.HUB_NOT_TRUSTED -> "${e.detail}。请在该 App 内确认本 Hub，或把本 Hub 的签名证书加入其可信列表。"
            else -> e.detail
        }

        private fun java.io.InputStream.readNBytesCompat(limit: Int): ByteArray {
            val out = java.io.ByteArrayOutputStream()
            val buf = ByteArray(8192)
            while (out.size() < limit) {
                val n = read(buf, 0, minOf(buf.size, limit - out.size()))
                if (n < 0) break
                out.write(buf, 0, n)
            }
            return out.toByteArray()
        }
    }
}
