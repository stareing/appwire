/**
 * 网页唤醒交接（spec/lifecycle.md 第 5 节「Web 唤醒交接」）。
 *
 * Host 以 `web-url` 唤醒时浏览器会新开一个带 `#app-mcp-wake=<token>` 的标签页，而原来休眠中的标签页仍在。
 * 新标签页先在同源、同 appId 的 `BroadcastChannel` 上发出 `offer`；休眠中的标签页回 `claim`，
 * 新标签页只把令牌 `grant` 给第一个认领者，认领者交给核心回连后回 `taken`。
 * 超时（默认 1 秒）内没有完成时 `offer` 返回 false，由新标签页自己用令牌回连，保证不丢唤醒。
 *
 * @invariant 令牌只在 `grant` 中出现一次、只发给一个认领者；通道名含 appId 且 BroadcastChannel 只在同源内投递。
 * @invariant 只有 `offer` 期间有一个定时器；空闲时只保留一个通道监听，无轮询。
 */

import type { BroadcastChannelFactory, BroadcastChannelLike } from './instance-guard'
import { randomId } from './storage'

/** 新标签页等待交接完成的上限。 */
export const DEFAULT_WAKE_HANDOFF_MS = 1000

export const wakeChannelName = (appId: string): string => `app-mcp:${appId}:wake`

type Message =
  | { t: 'offer'; app: string; id: string }
  | { t: 'claim'; app: string; id: string; from: string }
  | { t: 'grant'; app: string; id: string; to: string; token: string }
  | { t: 'taken'; app: string; id: string; from: string }

function isMessage(data: unknown): data is Message {
  if (typeof data !== 'object' || data === null) return false
  const m = data as Record<string, unknown>
  if (typeof m.app !== 'string' || typeof m.id !== 'string') return false
  switch (m.t) {
    case 'offer':
      return true
    case 'claim':
    case 'taken':
      return typeof m.from === 'string'
    case 'grant':
      return typeof m.to === 'string' && typeof m.token === 'string'
    default:
      return false
  }
}

export interface WakeHandoffOptions {
  appId: string
  createChannel: BroadcastChannelFactory
  /** 本标签页此刻能否认领唤醒（休眠中）。 */
  canClaim: () => boolean
  /** 收到交给本标签页的令牌：交给核心回连，返回核心是否接受。 */
  acceptToken: (token: string) => boolean
  timeoutMs?: number
}

interface PendingOffer {
  id: string
  token: string
  grantedTo: string | undefined
  timer: ReturnType<typeof setTimeout>
  resolve: (handedOff: boolean) => void
}

export class WakeHandoff {
  private readonly nonce = randomId()
  private channel: BroadcastChannelLike | undefined
  /** 本标签页已回 `claim`、等待 `grant` 的 offer。 */
  private claiming: string | undefined
  private pending: PendingOffer | undefined
  private readonly options: WakeHandoffOptions

  constructor(options: WakeHandoffOptions) {
    this.options = options
    try {
      this.channel = options.createChannel(wakeChannelName(options.appId))
    } catch {
      this.channel = undefined
      return
    }
    this.channel.onmessage = (event) => this.onMessage(event.data)
  }

  /**
   * 把令牌交给同源同 appId 的休眠标签页。返回 true 表示对方已接手（本标签页不得再用该令牌）；
   * 没有通道、无人认领或超时返回 false。
   */
  offer(token: string): Promise<boolean> {
    const channel = this.channel
    if (!channel || this.pending) return Promise.resolve(false)
    const id = randomId()
    return new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => this.settle(id, false), this.options.timeoutMs ?? DEFAULT_WAKE_HANDOFF_MS)
      this.pending = { id, token, grantedTo: undefined, timer, resolve }
      this.post({ t: 'offer', app: this.options.appId, id })
    })
  }

  dispose(): void {
    const pending = this.pending
    if (pending) this.settle(pending.id, false)
    const channel = this.channel
    this.channel = undefined
    if (!channel) return
    channel.onmessage = null
    try {
      channel.close()
    } catch {
      // 忽略
    }
  }

  private settle(id: string, handedOff: boolean): void {
    const pending = this.pending
    if (!pending || pending.id !== id) return
    this.pending = undefined
    clearTimeout(pending.timer)
    pending.resolve(handedOff)
  }

  private post(message: Message): void {
    try {
      this.channel?.postMessage(message)
    } catch {
      // 忽略（通道已关闭等）
    }
  }

  private onMessage(data: unknown): void {
    if (!this.channel || !isMessage(data) || data.app !== this.options.appId) return
    const app = this.options.appId
    const pending = this.pending
    switch (data.t) {
      case 'offer':
        if (pending?.id === data.id || !this.options.canClaim()) return
        this.claiming = data.id
        this.post({ t: 'claim', app, id: data.id, from: this.nonce })
        return
      case 'claim':
        if (!pending || pending.id !== data.id || pending.grantedTo !== undefined) return
        pending.grantedTo = data.from
        this.post({ t: 'grant', app, id: data.id, to: data.from, token: pending.token })
        return
      case 'grant':
        if (data.to !== this.nonce || data.id !== this.claiming) return
        this.claiming = undefined
        if (this.options.acceptToken(data.token)) this.post({ t: 'taken', app, id: data.id, from: this.nonce })
        return
      case 'taken':
        if (pending?.id === data.id && pending.grantedTo === data.from) this.settle(data.id, true)
        return
    }
  }
}
