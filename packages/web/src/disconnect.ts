/**
 * 已建立连接断开的归类（spec/protocol.md 10.1，类别 `disconnect`），与原生驱动层（crates/native）对齐：
 * 收到关闭（Close 帧 / 连接结束）→ `CONNECTION_CLOSED`，I/O 错误 → `CONNECTION_LOST`。
 *
 * @why 浏览器只给出 `close` / `error` 两种事件：`close`（CloseEvent，带关闭码）归为 CLOSED，
 * 先到的 `error`（连接失败、被重置）归为 LOST；`close` 未经关闭握手（`wasClean === false`，如 1006）时在说明中注明。
 */

import type { ChannelFailure } from './mux/protocol'

/** 断线错误码（心跳超时 `HEARTBEAT_TIMEOUT` 由核心自己给出，不经驱动层）。 */
export type DisconnectCode = 'CONNECTION_CLOSED' | 'CONNECTION_LOST'

export interface DisconnectIssue {
  code: DisconnectCode
  /** 中文说明。 */
  message: string
}

const DISCONNECT_CODES: ReadonlySet<string> = new Set<DisconnectCode>(['CONNECTION_CLOSED', 'CONNECTION_LOST'])

/** CloseEvent 的形状（测试中的假 WebSocket 也可给出）。 */
interface CloseEventLike {
  code: number
  reason?: unknown
  wasClean?: unknown
}

function isCloseEvent(ev: unknown): ev is CloseEventLike {
  return typeof ev === 'object' && ev !== null && typeof (ev as { code?: unknown }).code === 'number'
}

/**
 * WebSocket 的 `close` / `error` 事件 → 断线原因。
 * @input `ev` 为先到达的事件（`error` 后紧跟的 `close` 由调用方忽略）。
 */
export function socketDisconnectIssue(ev: unknown): DisconnectIssue {
  if (!isCloseEvent(ev)) return { code: 'CONNECTION_LOST', message: '与 Host 的连接中断（WebSocket 错误）' }
  const reason = typeof ev.reason === 'string' && ev.reason !== '' ? `，原因：${ev.reason}` : ''
  const unclean = ev.wasClean === false ? '，未经关闭握手' : ''
  return { code: 'CONNECTION_CLOSED', message: `Host 关闭了连接（关闭码 ${ev.code}${reason}${unclean}）` }
}

/**
 * 已打开的多路复用通道被持有方关闭 → 断线原因。持有方给出 `code` 时原样使用（`reason` 为说明）；
 * 未给出（持有方让位、无响应、暂存溢出等本地原因）时归为 `CONNECTION_LOST`。
 */
export function channelDisconnectIssue(failure: ChannelFailure | undefined): DisconnectIssue {
  const code = failure?.code
  if (code !== undefined && DISCONNECT_CODES.has(code)) {
    return { code, message: failure?.reason ?? (code === 'CONNECTION_CLOSED' ? 'Host 关闭了连接' : '与 Host 的连接中断') }
  }
  const reason = failure?.reason ? `：${failure.reason}` : ''
  return { code: 'CONNECTION_LOST', message: `共享连接中断${reason}` }
}
