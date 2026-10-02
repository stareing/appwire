package dev.appmcp.android

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.IBinder
import android.os.ParcelFileDescriptor
import android.util.Log
import dev.appmcp.AppMcp
import dev.appmcp.ChannelOffer
import dev.appmcp.StateStatus
import dev.appmcp.binder.BinderCaller
import dev.appmcp.binder.ChannelOpenException
import dev.appmcp.binder.FdChannel
import dev.appmcp.binder.FdChannelBinder
import dev.appmcp.binder.PackageIdentity
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull

/**
 * 按名寻址的 App 端（spec/naming.md 4.2）：导出的绑定式 Service，Intent 动作 `dev.appmcp.TOOLS`。Hub 以低优先级标志
 * `bindService` 即激活本进程（冷启动时 `Application.onCreate` 照常创建客户端），在 `onServiceConnected` 后调用一次 `open()`：
 *
 * 1. 核对调用方（`Binder.getCallingUid()`，[HubTrust]），不可信 → `HUB_NOT_TRUSTED`；
 * 2. `ParcelFileDescriptor.createSocketPair()`，一端交给本进程客户端（[AppMcp.acceptChannelFd]），另一端返回 Hub；
 * 3. 之后消息都走 fd（与本地 IPC 相同的 WebSocket 帧，SDK 先发 `app/hello`）；Hub 宽限后解绑 / 进程死亡 → fd EOF → 客户端转休眠。
 *
 * `onBind` 只返回 Binder（轻量）；不保存调用方引用、不传回调 Binder、不启动前台服务、不持 WakeLock。
 * App 在自己的清单中为本 Service 加 `<meta-data android:name="dev.appmcp.manifest" android:resource="@raw/…"/>`
 * 指向静态清单，Hub 据此发现（不启动本进程）。不需要按名寻址时用 `tools:node="remove"` 去掉本 Service。
 */
class ToolsService : Service() {
    override fun onBind(intent: Intent?): IBinder? {
        if (intent?.action != AppMcpAndroid.ACTION_TOOLS) return null
        return FdChannelBinder(FdChannel.APP_TOOLS_DESCRIPTOR) { caller, instance -> open(caller, instance) }
    }

    /** 在 Binder 线程上、调用方事务内执行。 */
    internal fun open(caller: BinderCaller, instance: String?): ParcelFileDescriptor {
        if (instance != null) {
            throw ChannelOpenException("NAME_NOT_FOUND", "本 App 只提供默认名字，没有登记实例 $instance")
        }
        if (!HubTrust.isTrusted(this, caller.uid)) {
            Log.w(TAG, "拒绝 uid ${caller.uid} 的拨号：不是可信的 Hub")
            throw ChannelOpenException(
                "HUB_NOT_TRUSTED",
                "uid ${caller.uid} 不在本 App 的可信 Hub 列表中（spec/naming.md 10.2）",
            )
        }
        val target = AppMcpAndroid.resolveChannelTarget(applicationContext)
            ?: throw ChannelOpenException("ACTIVATION_DENIED", "App 没有可用的 AppMcp 客户端（请在 Application 中创建并实现 AppMcpProvider）")
        return ChannelHandoff.offer(target, AppMcpAndroid.socketPairFactory)
    }

    private companion object {
        const val TAG = "AppMcpTools"
    }
}

/** 接受 Hub 通道的一方（默认为包装 [AppMcp] 的 [AppMcpChannelTarget]；测试或自定义宿主可替换）。 */
interface ChannelTarget {
    /**
     * 接受通道的 App 一端。返回非 null 时 [appEnd] 已被接管（无论是否接受）；返回 null 表示客户端已关闭、
     * [appEnd] 未被接管（由调用方关闭）。
     */
    fun acceptChannel(appEnd: ParcelFileDescriptor): ChannelOffer?

    /** 等到连接不处于"正在建立"（连接中 / 握手中 / 回连中）或超时；返回是否已稳定。 */
    fun awaitSettled(timeoutMillis: Long): Boolean = true
}

/** 把 [AppMcp] 适配为 [ChannelTarget]。 */
class AppMcpChannelTarget(val client: AppMcp) : ChannelTarget {
    override fun acceptChannel(appEnd: ParcelFileDescriptor): ChannelOffer? =
        if (client.isClosed) null else client.acceptChannelFd(appEnd.detachFd())

    override fun awaitSettled(timeoutMillis: Long): Boolean = runBlocking {
        withTimeoutOrNull(timeoutMillis) { client.state.first { it.status !in TRANSIENT } } != null
    }

    private companion object {
        val TRANSIENT = setOf(StateStatus.CONNECTING, StateStatus.HANDSHAKING, StateStatus.WAKING)
    }
}

/** socketpair 的两端：`first` 交给本进程客户端，`second` 返回给 Hub。 */
fun interface SocketPairFactory {
    fun create(): Pair<ParcelFileDescriptor, ParcelFileDescriptor>
}

/** 把 socketpair 的一端交给客户端、另一端交给 Hub。 */
internal object ChannelHandoff {
    /** 客户端正在拨出连接（如 on-demand 进入前台后连 Host）时等它结束的上限。 */
    private const val SETTLE_TIMEOUT_MS = 3_000L

    /**
     * @error 客户端已有连接 → `CHANNEL_LIMIT`；已停止 / 关闭 → `ACTIVATION_DENIED`。失败时两端都已关闭。
     */
    fun offer(target: ChannelTarget, pairs: SocketPairFactory): ParcelFileDescriptor {
        var outcome = handOnce(target, pairs)
        // @why 客户端刚好在拨出（连接 Host 失败前的短暂 CONNECTING）时按忙拒绝会让 Hub 的这次调用失败：等它落定后再试一次
        // （事件驱动等待状态变化，不轮询；不在 Hub 一侧重试，spec/naming.md U-02）。
        if (outcome is Attempt.Busy && target.awaitSettled(SETTLE_TIMEOUT_MS)) outcome = handOnce(target, pairs)
        return when (outcome) {
            is Attempt.Done -> outcome.hubEnd
            is Attempt.Busy -> throw ChannelOpenException("CHANNEL_LIMIT", outcome.detail)
            is Attempt.Refused -> throw ChannelOpenException("ACTIVATION_DENIED", outcome.detail)
        }
    }

    private sealed class Attempt {
        class Done(val hubEnd: ParcelFileDescriptor) : Attempt()
        class Busy(val detail: String) : Attempt()
        class Refused(val detail: String) : Attempt()
    }

    private fun handOnce(target: ChannelTarget, pairs: SocketPairFactory): Attempt {
        val (appEnd, hubEnd) = pairs.create()
        val offer = try {
            target.acceptChannel(appEnd)
        } catch (e: RuntimeException) {
            runCatching { appEnd.close() }
            hubEnd.close()
            return Attempt.Refused("客户端接受通道失败：${e.message}")
        }
        if (offer is dev.appmcp.ffi.ChannelOffer.Accepted) return Attempt.Done(hubEnd)
        hubEnd.close()
        return when (offer) {
            is dev.appmcp.ffi.ChannelOffer.Busy -> Attempt.Busy(offer.detail)
            is dev.appmcp.ffi.ChannelOffer.Stopped -> Attempt.Refused(offer.detail)
            is dev.appmcp.ffi.ChannelOffer.Invalid -> Attempt.Refused(offer.detail)
            else -> {
                // 客户端已关闭（null）：App 一端未被接管，由这里关闭。
                runCatching { appEnd.close() }
                Attempt.Refused("客户端已关闭")
            }
        }
    }
}

/**
 * Hub 信任（spec/naming.md 10.2）：`open()` 内按调用方 uid 判断，满足任一即放行：
 * ① 与本 App 同一 uid（进程内 Hub）；② 与本 App 同一签名证书（同一开发者）；③ 调用方任一签名证书摘要在可信列表中——
 * 构建期写入的 `<meta-data android:name="dev.appmcp.trustedHubs" android:value="sha256:…,…"/>`（Service 上）、
 * [AppMcpAndroid.trustedHubCertificates]，或用户在 App 内确认过的（[AppMcpAndroid.confirmHub]）。
 * 不在冷启动绑定中弹出界面。
 */
object HubTrust {
    private const val PREFS = "app_mcp_hub_trust"
    private const val KEY_CONFIRMED = "confirmed"

    /** 纯判定（便于测试）。 */
    @JvmStatic
    fun decide(callerUid: Int, myUid: Int, sameSignature: Boolean, callerCerts: Set<String>, trusted: Set<String>): Boolean =
        callerUid == myUid || sameSignature || callerCerts.any { it in trusted }

    @JvmStatic
    fun isTrusted(context: Context, callerUid: Int): Boolean {
        val pm = context.packageManager
        val myUid = android.os.Process.myUid()
        if (callerUid == myUid) return true
        // 系统查不到调用方的包：不可信（不依赖 checkSignatures 对未知 uid 的返回值）。
        if (pm.getPackagesForUid(callerUid).isNullOrEmpty()) return false
        val same = pm.checkSignatures(myUid, callerUid) == PackageManager.SIGNATURE_MATCH
        if (same) return true
        val callerCerts = PackageIdentity.of(pm, callerUid).certificates
        return decide(callerUid, myUid, false, callerCerts, trustedCertificates(context))
    }

    /** 全部可信证书摘要：清单 meta-data + 代码配置 + 用户确认。 */
    @JvmStatic
    fun trustedCertificates(context: Context): Set<String> {
        val fromManifest = runCatching {
            val info = context.packageManager.getServiceInfo(
                android.content.ComponentName(context, ToolsService::class.java), PackageManager.GET_META_DATA,
            )
            info.metaData?.getString(AppMcpAndroid.META_TRUSTED_HUBS).orEmpty()
        }.getOrDefault("")
        return (parseList(fromManifest) + AppMcpAndroid.trustedHubCertificates.mapNotNull(PackageIdentity::normalizeDigest) +
            confirmed(context)).toSet()
    }

    /** 逗号 / 空白分隔的摘要列表；不合法的项被忽略。 */
    @JvmStatic
    fun parseList(text: String): List<String> =
        text.split(',', ';', '\n').mapNotNull { it.takeIf(String::isNotBlank)?.let(PackageIdentity::normalizeDigest) }

    internal fun confirmed(context: Context): Set<String> =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getStringSet(KEY_CONFIRMED, emptySet()).orEmpty()

    internal fun setConfirmed(context: Context, digest: String, trusted: Boolean): Boolean {
        val normalized = PackageIdentity.normalizeDigest(digest) ?: return false
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val next = confirmed(context).toMutableSet().apply { if (trusted) add(normalized) else remove(normalized) }
        prefs.edit().putStringSet(KEY_CONFIRMED, next).apply()
        return true
    }
}
