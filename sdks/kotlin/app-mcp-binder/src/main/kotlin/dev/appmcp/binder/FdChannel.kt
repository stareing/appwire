package dev.appmcp.binder

import android.os.Binder
import android.os.IBinder
import android.os.Parcel
import android.os.ParcelFileDescriptor
import android.os.Parcelable
import android.os.RemoteException

/**
 * Binder 上"一次绑定只交换一个 fd"的线协议（spec/naming.md 4.2）：
 *
 * - 事务 [OPEN]（`FIRST_CALL_TRANSACTION`）：请求 `writeInterfaceToken(descriptor)` + `writeString(instance)`（可空）；
 *   成功回复 `writeNoException()` + `writeInt(1)` + `ParcelFileDescriptor`（socketpair 的一端）；
 *   失败回复 `writeException(e)`，说明以错误码开头：`<CODE>：<说明>`（[ChannelOpenException]）。
 * - 之后消息都走 fd，Binder 上不再有任何调用；不传回调 Binder 对象，服务端不保存客户端引用。
 */
object FdChannel {
    /** App 的工具服务（Intent 动作 `dev.appmcp.TOOLS`）：fd 上跑与本地 IPC 相同的 WebSocket 帧（SDK 为客户端）。 */
    const val APP_TOOLS_DESCRIPTOR = "dev.appmcp.IAppTools/1"

    /** 独立 Hub App 的 MCP 服务（Intent 动作 `dev.appmcp.HUB`）：fd 上跑 MCP（每行一条 JSON-RPC）。 */
    const val HUB_DESCRIPTOR = "dev.appmcp.IHub/1"

    /** 唯一的事务。 */
    const val OPEN = IBinder.FIRST_CALL_TRANSACTION
}

/**
 * 打开通道失败。[code] 为 spec/naming.md 第 12 节的错误码（`HUB_NOT_TRUSTED`、`CHANNEL_LIMIT`、`ACTIVATION_DENIED`…）。
 */
class ChannelOpenException(val code: String, message: String, cause: Throwable? = null) :
    Exception("$code：$message", cause) {
    /** 不带错误码前缀的说明。 */
    val detail: String = message

    companion object {
        private val CODED = Regex("^([A-Z][A-Z_]{2,40})：(.*)$", RegexOption.DOT_MATCHES_ALL)

        /** 从对端回传的异常还原：说明以错误码开头时取之，否则按 [fallback]。 */
        @JvmStatic
        fun fromRemote(e: Exception, fallback: String): ChannelOpenException {
            val text = e.message.orEmpty()
            val m = CODED.find(text)
            return if (m != null) ChannelOpenException(m.groupValues[1], m.groupValues[2], e)
            else ChannelOpenException(fallback, text.ifEmpty { e.javaClass.simpleName }, e)
        }
    }
}

/** 调用方（Binder 事务内的 `getCallingUid` / `getCallingPid`）。 */
data class BinderCaller(val uid: Int, val pid: Int)

/**
 * 服务端：`onBind` 返回它。[open] 在 Binder 线程上、调用方的事务内执行（可取 [BinderCaller]），返回交给对端的一端；
 * 抛 [ChannelOpenException] 拒绝（`HUB_NOT_TRUSTED` 以 `SecurityException` 回传，其余以 `IllegalStateException`）。
 *
 * @invariant 不保存调用方的任何引用；返回的 fd 写入回复后由本类关闭本进程的副本。
 */
class FdChannelBinder(
    private val descriptor: String,
    private val open: (caller: BinderCaller, instance: String?) -> ParcelFileDescriptor,
) : Binder() {
    init {
        attachInterface(null, descriptor)
    }

    override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
        if (code == INTERFACE_TRANSACTION) {
            reply?.writeString(descriptor)
            return true
        }
        if (code != FdChannel.OPEN) return super.onTransact(code, data, reply, flags)
        data.enforceInterface(descriptor)
        val instance = data.readString()
        val caller = BinderCaller(getCallingUid(), getCallingPid())
        val out = reply ?: return true
        val fd = try {
            open(caller, instance)
        } catch (e: ChannelOpenException) {
            out.writeException(if (e.code == HUB_NOT_TRUSTED) SecurityException(e.message) else IllegalStateException(e.message))
            return true
        } catch (e: SecurityException) {
            out.writeException(SecurityException("$HUB_NOT_TRUSTED：${e.message}"))
            return true
        } catch (e: RuntimeException) {
            out.writeException(IllegalStateException("$ACTIVATION_DENIED：${e.message ?: e.javaClass.simpleName}"))
            return true
        }
        try {
            out.writeNoException()
            out.writeInt(1)
            fd.writeToParcel(out, Parcelable.PARCELABLE_WRITE_RETURN_VALUE)
        } finally {
            // @why 写入回复时 fd 已被复制进 Parcel；本进程的副本不再需要（PARCELABLE_WRITE_RETURN_VALUE 会关闭它，这里兜底）。
            runCatching { fd.close() }
        }
        return true
    }

    private companion object {
        const val HUB_NOT_TRUSTED = "HUB_NOT_TRUSTED"
        const val ACTIVATION_DENIED = "ACTIVATION_DENIED"
    }
}

/** 客户端：在 `onServiceConnected` 拿到的 [IBinder] 上打开通道（只调用一次，之后不再使用该代理）。 */
object FdChannelClient {
    /**
     * @error 对端拒绝 → [ChannelOpenException]（码取自对端说明；`SecurityException` 缺省为 `HUB_NOT_TRUSTED`）；
     * 对端进程死亡 / 被冻结后被杀（`RemoteException`）→ `ACTIVATION_DENIED`，不重试（spec/naming.md U-02）。
     */
    @JvmStatic
    fun open(binder: IBinder, descriptor: String, instance: String? = null): ParcelFileDescriptor {
        val data = Parcel.obtain()
        val reply = Parcel.obtain()
        try {
            data.writeInterfaceToken(descriptor)
            data.writeString(instance)
            try {
                binder.transact(FdChannel.OPEN, data, reply, 0)
            } catch (e: RemoteException) {
                throw ChannelOpenException("ACTIVATION_DENIED", "Binder 调用失败（对端进程可能已退出）：${e.message}", e)
            }
            try {
                reply.readException()
            } catch (e: SecurityException) {
                throw ChannelOpenException.fromRemote(e, "HUB_NOT_TRUSTED")
            } catch (e: RuntimeException) {
                throw ChannelOpenException.fromRemote(e, "ACTIVATION_DENIED")
            }
            if (reply.readInt() != 1) throw ChannelOpenException("ACTIVATION_DENIED", "对端没有返回通道")
            return ParcelFileDescriptor.CREATOR.createFromParcel(reply)
        } finally {
            data.recycle()
            reply.recycle()
        }
    }
}
