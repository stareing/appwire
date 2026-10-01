/**
 * 标签页与连接持有方之间的两种端点：
 *
 * - {@link portEndpoint}：SharedWorker 的 `MessagePort`（首选；Chrome 148 起 Android 也支持）；
 * - {@link LeaderEndpoint}：没有 SharedWorker 时，用 Web Locks 选出一个主标签页持有连接，
 *   其他标签页经 `BroadcastChannel` 与它通信。主标签页在 `pagehide` / `freeze` 时让位
 *   （冻结或进入 bfcache 的页面无法继续服务其他标签页），其他标签页的通道随之断开，由各自核心重连到新的主标签页。
 */

import type { BroadcastChannelFactory, BroadcastChannelLike } from '../instance-guard'
import { randomId } from '../storage'
import type { OwnerEndpoint } from './link'
import { MuxOwner, type TabHandle } from './owner'
import { type ChannelFailure, type OwnerToTab, type TabToOwner, isOwnerToTab, isTabToOwner } from './protocol'

// ---------------------------------------------------------------------------
// SharedWorker
// ---------------------------------------------------------------------------

export interface MessagePortLike {
  postMessage(message: unknown): void
  onmessage: ((ev: { data: unknown }) => void) | null
  start?(): void
  close(): void
}

export interface SharedWorkerLike {
  readonly port: MessagePortLike
  onerror: ((ev: unknown) => void) | null
}

/** SharedWorker 端点。脚本无法加载（`error` 事件）时，通道以 `unsupported` 关闭，驱动层改为直接连接。 */
export function portEndpoint(worker: SharedWorkerLike): OwnerEndpoint {
  const port = worker.port
  let broken: ChannelFailure | undefined
  const ep: OwnerEndpoint = {
    onmessage: null,
    onreset: null,
    post(message) {
      if (broken) {
        if (message.t === 'open') {
          const ch = message.ch
          const failure = broken
          queueMicrotask(() => ep.onmessage?.({ t: 'closed', ch, ...failure }))
        }
        return
      }
      port.postMessage(message)
    },
    close() {
      port.onmessage = null
      worker.onerror = null
      port.close()
    },
  }
  port.onmessage = (ev) => {
    if (isOwnerToTab(ev.data)) ep.onmessage?.(ev.data)
  }
  worker.onerror = () => {
    broken = { unsupported: true, reason: 'SharedWorker 无法启动' }
    ep.onreset?.(broken)
  }
  port.start?.()
  return ep
}

// ---------------------------------------------------------------------------
// Web Locks 选主
// ---------------------------------------------------------------------------

/** `navigator.locks` 的子集。 */
export interface LocksLike {
  request(name: string, options: { ifAvailable: boolean }, callback: (lock: unknown) => unknown): Promise<unknown>
}

type Wire =
  | { k: 'to-leader'; leader: string; from: string; msg: TabToOwner }
  | { k: 'to-tab'; to: string; from: string; msg: OwnerToTab }
  | { k: 'leader'; id: string }
  | { k: 'who' }
  | { k: 'abdicate'; id: string }

function isWire(v: unknown): v is Wire {
  if (typeof v !== 'object' || v === null) return false
  const m = v as Record<string, unknown>
  switch (m.k) {
    case 'to-leader':
      return typeof m.leader === 'string' && typeof m.from === 'string' && isTabToOwner(m.msg)
    case 'to-tab':
      return typeof m.to === 'string' && typeof m.from === 'string' && isOwnerToTab(m.msg)
    case 'leader':
    case 'abdicate':
      return typeof m.id === 'string'
    case 'who':
      return true
    default:
      return false
  }
}

export interface LeaderOptions {
  /** 选主范围（同一 Host 地址的所有标签页）。 */
  name: string
  createChannel: BroadcastChannelFactory
  locks: LocksLike
  /** 成为主标签页时创建持有方。 */
  createOwner: () => MuxOwner
  /** 监听 `pagehide` / `freeze`（让位时机）。 */
  win?: EventTarget | undefined
  doc?: EventTarget | undefined
  /** 没有主标签页应答时重新选主的间隔，默认 500 毫秒。 */
  retryMs?: number
}

export class LeaderEndpoint implements OwnerEndpoint {
  onmessage: ((message: OwnerToTab) => void) | null = null
  onreset: ((failure: ChannelFailure) => void) | null = null

  readonly tabId = randomId()
  private channel: BroadcastChannelLike | undefined
  private leaderId: string | undefined
  /** 本标签页是主标签页时的持有方与释放锁的函数。 */
  private owner: MuxOwner | undefined
  private releaseLock: (() => void) | undefined
  private self: TabHandle | undefined
  private readonly followers = new Map<string, TabHandle>()
  /** 主标签页未知时暂存的消息。 */
  private readonly queue: TabToOwner[] = []
  private electing = false
  /** 从 bfcache 恢复时暂存前的主标签页，用于判断期间是否换过主。 */
  private parkedLeader: string | undefined
  private retry: ReturnType<typeof setTimeout> | undefined
  private closed = false
  private readonly offs: Array<() => void> = []

  constructor(private readonly opts: LeaderOptions) {
    this.openChannel()
    const listen = (target: EventTarget | undefined, type: string): void => {
      if (!target) return
      const fn = (): void => this.abdicate()
      target.addEventListener(type, fn)
      this.offs.push(() => target.removeEventListener(type, fn))
    }
    // 冻结 / 进入 bfcache / 卸载的页面无法继续持有连接
    listen(opts.win, 'pagehide')
    listen(opts.doc, 'freeze')
  }

  get isLeader(): boolean {
    return this.owner !== undefined
  }

  post(message: TabToOwner): void {
    if (this.closed) return
    if (message.t === 'unpark' && !this.channel) {
      // 缓存期间可能错过了让位广播：重新确认主标签页（与暂存前不同则旧通道失效，见 setLeader）
      this.openChannel()
      this.parkedLeader = this.leaderId
      this.leaderId = undefined
    }
    if (this.owner) {
      this.self?.receive(message)
    } else if (this.leaderId !== undefined && this.channel) {
      this.send({ k: 'to-leader', leader: this.leaderId, from: this.tabId, msg: message })
    } else {
      this.queue.push(message)
      this.elect()
    }
    // 进入 bfcache：发出 park 后关闭广播通道（向缓存中的页面投递消息会使其被逐出），恢复时重新打开。
    if (message.t === 'park' && !this.owner) this.closeChannel()
  }

  /** 主标签页对打开通道无应答（可能已崩溃，锁已随之释放）：忘掉它，下次发送时重新选主。 */
  stalled(): void {
    if (!this.owner) this.setLeader(undefined)
  }

  close(): void {
    if (this.closed) return
    this.abdicate()
    this.closed = true
    clearTimeout(this.retry)
    for (const off of this.offs.splice(0)) off()
    this.closeChannel()
  }

  /** 主标签页让位：断开全部通道、释放锁并广播。 */
  abdicate(): void {
    const owner = this.owner
    if (!owner) return
    owner.closeAll()
    this.owner = undefined
    this.self = undefined
    this.followers.clear()
    this.leaderId = undefined
    this.send({ k: 'abdicate', id: this.tabId })
    const release = this.releaseLock
    this.releaseLock = undefined
    release?.()
  }

  // ---- 内部 -----------------------------------------------------------

  private openChannel(): void {
    try {
      this.channel = this.opts.createChannel(this.opts.name)
    } catch {
      this.channel = undefined
      return
    }
    this.channel.onmessage = (ev) => {
      if (isWire(ev.data)) this.onWire(ev.data)
    }
  }

  private closeChannel(): void {
    const ch = this.channel
    this.channel = undefined
    if (!ch) return
    ch.onmessage = null
    try {
      ch.close()
    } catch {
      // 忽略
    }
  }

  private send(m: Wire): void {
    try {
      this.channel?.postMessage(m)
    } catch {
      // 通道已关闭
    }
  }

  private onWire(m: Wire): void {
    switch (m.k) {
      case 'to-leader': {
        if (!this.owner || m.leader !== this.tabId) return
        let handle = this.followers.get(m.from)
        if (!handle) {
          const to = m.from
          handle = this.owner.attach((msg) => this.send({ k: 'to-tab', to, from: this.tabId, msg }))
          this.followers.set(to, handle)
        }
        handle.receive(m.msg)
        if (m.msg.t === 'bye') this.followers.delete(m.from)
        break
      }
      case 'to-tab':
        if (m.to === this.tabId && m.from === this.leaderId) this.onmessage?.(m.msg)
        break
      case 'leader':
        if (this.owner || m.id === this.leaderId) return
        this.setLeader(m.id)
        break
      case 'who':
        if (this.owner) this.send({ k: 'leader', id: this.tabId })
        break
      case 'abdicate':
        if (m.id === this.leaderId) this.setLeader(undefined)
        break
    }
  }

  /** 主标签页变化：经旧主标签页打开的通道已失效。 */
  private setLeader(id: string | undefined): void {
    const previous = this.leaderId ?? this.parkedLeader
    this.parkedLeader = undefined
    this.leaderId = id
    if (previous !== undefined && previous !== id) this.onreset?.({})
    if (id !== undefined) this.flush()
  }

  private flush(): void {
    for (const m of this.queue.splice(0)) this.post(m)
  }

  private elect(): void {
    if (this.electing || this.closed) return
    this.electing = true
    this.opts.locks
      .request(this.opts.name, { ifAvailable: true }, (lock) => {
        this.electing = false
        if (this.closed) return undefined
        if (!lock) {
          this.askLeader()
          return undefined
        }
        this.becomeLeader()
        // 持有锁直到让位
        return new Promise<void>((resolve) => {
          this.releaseLock = resolve
        })
      })
      .catch(() => {
        this.electing = false
        this.askLeader()
      })
  }

  /** 锁已被占用：询问主标签页是谁；仍无应答则稍后重新选主（主标签页可能刚退出）。 */
  private askLeader(): void {
    this.send({ k: 'who' })
    clearTimeout(this.retry)
    this.retry = setTimeout(() => {
      this.retry = undefined
      if (this.queue.length > 0 && this.leaderId === undefined) this.elect()
    }, this.opts.retryMs ?? 500)
  }

  private becomeLeader(): void {
    const owner = this.opts.createOwner()
    this.owner = owner
    this.leaderId = this.tabId
    this.self = owner.attach((msg) => queueMicrotask(() => this.onmessage?.(msg)))
    this.send({ k: 'leader', id: this.tabId })
    this.flush()
  }
}
