/**
 * 标签页侧：经连接持有方（SharedWorker 或主标签页）打开多路复用通道。
 *
 * 每个通道对驱动层表现为一个 WebSocket（{@link MuxChannel} 与 `WebSocketLike` 同形），
 * 核心照常在其上握手、心跳、休眠，不感知多路复用。
 */

import type { SocketLike } from './owner'
import { type ChannelFailure, type OwnerToTab, type TabToOwner } from './protocol'

/** 与持有方之间的消息端点（MessagePort、BroadcastChannel 选主等）。 */
export interface OwnerEndpoint {
  post(message: TabToOwner): void
  /** 持有方发来的消息。 */
  onmessage: ((message: OwnerToTab) => void) | null
  /**
   * 持有方已不可用（如主标签页让位）：经它打开的通道全部视为断开。
   * `failure.unsupported` 表示共享连接在本页不可用（如 SharedWorker 脚本无法加载），驱动层改为直接连接。
   */
  onreset: ((failure: ChannelFailure) => void) | null
  /** 持有方对打开通道无应答（可选）：端点可据此重新寻找持有方。 */
  stalled?(): void
  close(): void
}

/** 通道打开的等待上限：持有方无响应（如 SharedWorker 崩溃）时按连接失败处理。 */
export const DEFAULT_OPEN_TIMEOUT_MS = 20_000

const CONNECTING = 0
const OPEN = 1
const CLOSED = 3

export class MuxChannel implements SocketLike {
  readyState = CONNECTING
  /** 关闭原因（持有方报告的），供驱动层诊断。 */
  failure: ChannelFailure | undefined
  onopen: ((ev: unknown) => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: unknown) => void) | null = null
  onerror: ((ev: unknown) => void) | null = null
  timer: ReturnType<typeof setTimeout> | undefined

  constructor(
    readonly ch: number,
    private readonly link: MuxLink,
  ) {}

  send(data: string): void {
    if (this.readyState === OPEN) this.link.post({ t: 'send', ch: this.ch, text: data })
  }

  close(): void {
    if (this.readyState === CLOSED) return
    this.readyState = CLOSED
    this.link.forget(this, true)
  }

  /** 由持有方关闭（或打开失败）。 */
  closedByOwner(failure: ChannelFailure): void {
    if (this.readyState === CLOSED) return
    this.readyState = CLOSED
    this.failure = failure
    this.link.forget(this, false)
    this.onclose?.({})
  }

  opened(): void {
    if (this.readyState !== CONNECTING) return
    this.readyState = OPEN
    clearTimeout(this.timer)
    this.timer = undefined
    this.onopen?.({})
  }
}

export class MuxLink {
  private readonly channels = new Map<number, MuxChannel>()
  private nextCh = 1
  private disposed = false

  constructor(
    private readonly endpoint: OwnerEndpoint,
    private readonly openTimeoutMs = DEFAULT_OPEN_TIMEOUT_MS,
  ) {
    endpoint.onmessage = (m) => this.onOwner(m)
    endpoint.onreset = (failure) => {
      for (const c of [...this.channels.values()]) c.closedByOwner(failure)
    }
  }

  /** 打开通道。返回的对象与 WebSocket 同形；打开结果经 `onopen` / `onclose` 报告。 */
  open(url: string): MuxChannel {
    const chan = new MuxChannel(this.nextCh++, this)
    if (this.disposed) {
      queueMicrotask(() => chan.closedByOwner({}))
      return chan
    }
    this.channels.set(chan.ch, chan)
    chan.timer = setTimeout(() => {
      chan.timer = undefined
      if (chan.readyState !== CONNECTING) return
      this.post({ t: 'close', ch: chan.ch })
      chan.closedByOwner({ reason: '共享连接无响应' })
      this.endpoint.stalled?.()
    }, this.openTimeoutMs)
    this.post({ t: 'open', ch: chan.ch, url })
    return chan
  }

  /** 页面进入 bfcache：请持有方暂存消息。 */
  park(): void {
    if (!this.disposed) this.post({ t: 'park' })
  }

  /** 页面从 bfcache 恢复。 */
  unpark(): void {
    if (!this.disposed) this.post({ t: 'unpark' })
  }

  /** 页面卸载：关闭全部通道并断开端点。 */
  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    for (const c of this.channels.values()) clearTimeout(c.timer)
    this.channels.clear()
    this.post({ t: 'bye' })
    this.endpoint.onmessage = null
    this.endpoint.onreset = null
    try {
      this.endpoint.close()
    } catch {
      // 忽略
    }
  }

  /** @internal */
  post(message: TabToOwner): void {
    try {
      this.endpoint.post(message)
    } catch {
      // 端点已关闭
    }
  }

  /** @internal 通道结束；`notifyOwner` 时通知持有方关闭。 */
  forget(chan: MuxChannel, notifyOwner: boolean): void {
    clearTimeout(chan.timer)
    chan.timer = undefined
    if (this.channels.get(chan.ch) !== chan) return
    this.channels.delete(chan.ch)
    if (notifyOwner && !this.disposed) this.post({ t: 'close', ch: chan.ch })
  }

  private onOwner(m: OwnerToTab): void {
    const chan = this.channels.get(m.ch)
    if (!chan) return
    switch (m.t) {
      case 'opened':
        chan.opened()
        break
      case 'message':
        if (chan.readyState === OPEN) chan.onmessage?.({ data: m.text })
        break
      case 'closed': {
        const { t: _t, ch: _ch, ...failure } = m
        chan.closedByOwner(failure)
        break
      }
    }
  }
}
