/**
 * 多路复用（spec/protocol.md 第 9 节）的两层消息：
 *
 * 1. **Host 帧**：多路复用连接上的 WebSocket 文本帧 `{"type":"open"|"msg"|"close","ch":N,…}`；
 * 2. **标签页 ↔ 连接持有方**：标签页（每个 `AppMcp` 一个核心）与持有 WebSocket 的一方（SharedWorker，
 *    或没有 SharedWorker 时选出的主标签页）之间经 `MessagePort` / `BroadcastChannel` 传递的结构化消息。
 *
 * 通道号分两种：标签页内的局部号（`ch`，由标签页分配）与连接上的通道号（由持有方分配，不出持有方）。
 */

/** `app/mux` 请求的 JSON-RPC id（连接上的第一条消息）。 */
export const MUX_REQUEST_ID = 'mux'
export const MUX_VERSION = 1

export const MUX_REQUEST = JSON.stringify({
  jsonrpc: '2.0',
  id: MUX_REQUEST_ID,
  method: 'app/mux',
  params: { version: MUX_VERSION },
})

// ---------------------------------------------------------------------------
// Host 帧
// ---------------------------------------------------------------------------

export type HostFrame =
  | { type: 'open'; ch: number }
  | { type: 'msg'; ch: number; text: string }
  | { type: 'close'; ch: number; reason?: string }

export function encodeOpen(ch: number): string {
  return `{"type":"open","ch":${ch}}`
}

/** `text` 是核心产生的 JSON-RPC 文本（合法 JSON），原样嵌入。 */
export function encodeMsg(ch: number, text: string): string {
  return `{"type":"msg","ch":${ch},"msg":${text}}`
}

export function encodeClose(ch: number): string {
  return `{"type":"close","ch":${ch}}`
}

/** 解析 Host 帧；格式不对时返回 undefined。 */
export function decodeFrame(text: string): HostFrame | undefined {
  let v: unknown
  try {
    v = JSON.parse(text)
  } catch {
    return undefined
  }
  if (typeof v !== 'object' || v === null) return undefined
  const f = v as { type?: unknown; ch?: unknown; msg?: unknown; reason?: unknown }
  if (typeof f.ch !== 'number' || !Number.isInteger(f.ch) || f.ch < 1) return undefined
  switch (f.type) {
    case 'open':
      return { type: 'open', ch: f.ch }
    case 'msg':
      return typeof f.msg === 'object' && f.msg !== null ? { type: 'msg', ch: f.ch, text: JSON.stringify(f.msg) } : undefined
    case 'close':
      return typeof f.reason === 'string' ? { type: 'close', ch: f.ch, reason: f.reason } : { type: 'close', ch: f.ch }
    default:
      return undefined
  }
}

/** `app/mux` 的响应：`true` 协商成功，`false` Host 不支持（旧 Host 返回错误），undefined 不是该响应。 */
export function parseMuxReply(text: string): boolean | undefined {
  let v: unknown
  try {
    v = JSON.parse(text)
  } catch {
    return undefined
  }
  if (typeof v !== 'object' || v === null) return undefined
  const r = v as { id?: unknown; result?: unknown; error?: unknown }
  if (r.id !== MUX_REQUEST_ID) return undefined
  if (r.error !== undefined) return false
  const version = (r.result as { version?: unknown } | undefined)?.version
  return typeof version === 'number' && version >= 1 && version <= MUX_VERSION
}

// ---------------------------------------------------------------------------
// 标签页 ↔ 持有方
// ---------------------------------------------------------------------------

/** 通道失败的原因（随 `closed` 发给标签页）。 */
export interface ChannelFailure {
  /** Host 不支持多路复用（旧 Host）：标签页改为自己直接连接。 */
  unsupported?: boolean
  /** 持有方所在环境的 CSP `connect-src` 拦截了连接。 */
  csp?: boolean
  /** 诊断说明（Host 关闭通道的原因等）。 */
  reason?: string
}

export type TabToOwner =
  /** 打开通道（`url` 为 Host 地址）。 */
  | { t: 'open'; ch: number; url: string }
  | { t: 'send'; ch: number; text: string }
  | { t: 'close'; ch: number }
  /** 页面进入 bfcache：暂存发给本标签页的消息（向缓存中的页面投递消息会使其被逐出）。 */
  | { t: 'park' }
  /** 页面从 bfcache 恢复：投递暂存的消息。 */
  | { t: 'unpark' }
  /** 页面卸载：关闭本标签页的全部通道。 */
  | { t: 'bye' }

export type OwnerToTab =
  | { t: 'opened'; ch: number }
  | { t: 'message'; ch: number; text: string }
  | ({ t: 'closed'; ch: number } & ChannelFailure)

export function isTabToOwner(v: unknown): v is TabToOwner {
  if (typeof v !== 'object' || v === null) return false
  const m = v as { t?: unknown; ch?: unknown; url?: unknown; text?: unknown }
  switch (m.t) {
    case 'open':
      return typeof m.ch === 'number' && typeof m.url === 'string'
    case 'send':
      return typeof m.ch === 'number' && typeof m.text === 'string'
    case 'close':
      return typeof m.ch === 'number'
    case 'park':
    case 'unpark':
    case 'bye':
      return true
    default:
      return false
  }
}

export function isOwnerToTab(v: unknown): v is OwnerToTab {
  if (typeof v !== 'object' || v === null) return false
  const m = v as { t?: unknown; ch?: unknown; text?: unknown }
  if (typeof m.ch !== 'number') return false
  return m.t === 'opened' || m.t === 'closed' || (m.t === 'message' && typeof m.text === 'string')
}
