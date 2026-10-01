/**
 * 连接持有方：每个 Host 地址一条多路复用 WebSocket（spec/protocol.md 第 9 节），承载所有标签页的通道。
 *
 * 运行在 SharedWorker 中（`mux-worker.ts`），或在没有 SharedWorker 的浏览器中运行在选出的主标签页里
 * （`leader.ts`）。持有方不理解 v1 消息内容，只做：协商、分配通道号、装帧 / 拆帧、通道开关、
 * bfcache 暂存。每个通道上的握手、心跳、休眠全部由该标签页自己的核心完成。
 */

import {
  type ChannelFailure,
  MUX_REQUEST,
  type OwnerToTab,
  type TabToOwner,
  decodeFrame,
  encodeClose,
  encodeMsg,
  encodeOpen,
  isTabToOwner,
  parseMuxReply,
} from './protocol'

/** 持有方需要的 WebSocket 子集。 */
export interface SocketLike {
  readonly readyState: number
  send(data: string): void
  close(code?: number, reason?: string): void
  onopen: ((ev: unknown) => void) | null
  onmessage: ((ev: { data: unknown }) => void) | null
  onclose: ((ev: unknown) => void) | null
  onerror: ((ev: unknown) => void) | null
}

export interface OwnerOptions {
  createWebSocket: (url: string) => SocketLike
  /** 所在环境的 CSP 是否拦截了到 `url` 的连接（由宿主监听 `securitypolicyviolation` 提供）。 */
  cspBlocked?: (url: string) => boolean
  /** `app/mux` 协商超时，默认 10000 毫秒。 */
  negotiateTimeoutMs?: number
  /** 判定 Host 不支持多路复用后，在这段时间内直接以 `unsupported` 拒绝新通道，默认 60000 毫秒。 */
  unsupportedTtlMs?: number
  /** 暂存（bfcache）期间每个标签页最多缓存的消息数，默认 1024；超出时关闭该标签页的全部通道。 */
  parkLimit?: number
  now?: () => number
}

/** 一个已接入的标签页。 */
export interface TabHandle {
  /** 标签页发来的消息（结构不对的忽略）。 */
  receive(message: unknown): void
  /** 标签页已不可达：关闭它的全部通道。 */
  detach(): void
}

interface Tab {
  post: (message: OwnerToTab) => void
  parked: boolean
  buffer: OwnerToTab[]
  /** 局部通道号 → 通道。 */
  channels: Map<number, Chan>
  detached: boolean
}

interface Chan {
  tab: Tab
  local: number
  host: HostConn
  /** 连接上的通道号。 */
  id: number
  /** 已向 Host 发出 `open`。 */
  opened: boolean
}

interface HostConn {
  url: string
  ws: SocketLike
  state: 'connecting' | 'negotiating' | 'ready' | 'closed'
  nextId: number
  chans: Map<number, Chan>
  timer: ReturnType<typeof setTimeout> | undefined
}

const WS_OPEN = 1

export class MuxOwner {
  private readonly opts: OwnerOptions
  private readonly hosts = new Map<string, HostConn>()
  /** Host 地址 → 判定不支持多路复用的时刻。 */
  private readonly unsupported = new Map<string, number>()
  private readonly now: () => number

  constructor(options: OwnerOptions) {
    this.opts = options
    this.now = options.now ?? (() => Date.now())
  }

  /** 接入一个标签页；`post` 把消息发给该标签页。 */
  attach(post: (message: OwnerToTab) => void): TabHandle {
    const tab: Tab = { post, parked: false, buffer: [], channels: new Map(), detached: false }
    return {
      receive: (message) => {
        if (!tab.detached && isTabToOwner(message)) this.onTab(tab, message)
      },
      detach: () => this.detach(tab),
    }
  }

  /** 关闭全部连接（主标签页让位时）：所有通道以断开通知各标签页。 */
  closeAll(): void {
    for (const host of [...this.hosts.values()]) this.failHost(host, {})
  }

  /** 当前连接数（测试用）。 */
  get connectionCount(): number {
    return this.hosts.size
  }

  // ---- 标签页消息 -------------------------------------------------------

  private onTab(tab: Tab, m: TabToOwner): void {
    switch (m.t) {
      case 'open':
        this.open(tab, m.ch, m.url)
        break
      case 'send': {
        const chan = tab.channels.get(m.ch)
        if (chan?.opened && chan.host.state === 'ready' && chan.host.ws.readyState === WS_OPEN) {
          chan.host.ws.send(encodeMsg(chan.id, m.text))
        }
        break
      }
      case 'close': {
        const chan = tab.channels.get(m.ch)
        if (chan) this.removeChan(chan, true)
        break
      }
      case 'park':
        tab.parked = true
        break
      case 'unpark': {
        tab.parked = false
        for (const msg of tab.buffer.splice(0)) this.postTo(tab, msg)
        break
      }
      case 'bye':
        this.detach(tab)
        break
    }
  }

  private open(tab: Tab, local: number, url: string): void {
    const old = tab.channels.get(local)
    if (old) this.removeChan(old, true)
    const since = this.unsupported.get(url)
    if (since !== undefined) {
      if (this.now() - since < (this.opts.unsupportedTtlMs ?? 60_000)) {
        this.deliver(tab, { t: 'closed', ch: local, unsupported: true })
        return
      }
      this.unsupported.delete(url)
    }
    let host = this.hosts.get(url)
    if (!host) {
      const created = this.connect(url)
      if (!created) {
        this.deliver(tab, { t: 'closed', ch: local, ...this.failure(url) })
        return
      }
      host = created
    }
    const chan: Chan = { tab, local, host, id: host.nextId++, opened: false }
    tab.channels.set(local, chan)
    host.chans.set(chan.id, chan)
    if (host.state === 'ready') this.openChan(chan)
  }

  private openChan(chan: Chan): void {
    chan.opened = true
    chan.host.ws.send(encodeOpen(chan.id))
    this.deliver(chan.tab, { t: 'opened', ch: chan.local })
  }

  /** 移除通道；`toHost` 时向 Host 发送 `close`。连接上没有通道时关闭连接。 */
  private removeChan(chan: Chan, toHost: boolean): void {
    const host = chan.host
    if (chan.tab.channels.get(chan.local) === chan) chan.tab.channels.delete(chan.local)
    if (host.chans.get(chan.id) !== chan) return
    host.chans.delete(chan.id)
    if (toHost && chan.opened && host.state === 'ready' && host.ws.readyState === WS_OPEN) {
      host.ws.send(encodeClose(chan.id))
    }
    if (host.chans.size === 0) this.closeHost(host)
  }

  private detach(tab: Tab): void {
    if (tab.detached) return
    for (const chan of [...tab.channels.values()]) this.removeChan(chan, true)
    tab.detached = true
    tab.buffer.length = 0
  }

  // ---- 投递 -----------------------------------------------------------

  private deliver(tab: Tab, message: OwnerToTab): void {
    if (tab.detached) return
    if (!tab.parked) {
      this.postTo(tab, message)
      return
    }
    tab.buffer.push(message)
    if (tab.buffer.length <= (this.opts.parkLimit ?? 1024)) return
    // 暂存溢出：关闭该标签页的全部通道，恢复时只通知断开。
    const closed: OwnerToTab[] = []
    for (const chan of [...tab.channels.values()]) {
      this.removeChan(chan, true)
      closed.push({ t: 'closed', ch: chan.local, reason: 'bfcache 期间积压的消息过多' })
    }
    tab.buffer = closed
  }

  private postTo(tab: Tab, message: OwnerToTab): void {
    try {
      tab.post(message)
    } catch {
      // 端口已关闭：视为标签页离开
      this.detach(tab)
    }
  }

  // ---- 连接 -----------------------------------------------------------

  private connect(url: string): HostConn | undefined {
    let ws: SocketLike
    try {
      ws = this.opts.createWebSocket(url)
    } catch {
      return undefined
    }
    const host: HostConn = { url, ws, state: 'connecting', nextId: 1, chans: new Map(), timer: undefined }
    this.hosts.set(url, host)
    ws.onopen = () => {
      if (host.state !== 'connecting') return
      host.state = 'negotiating'
      ws.send(MUX_REQUEST)
      host.timer = setTimeout(() => this.failHost(host, { reason: 'app/mux 协商超时' }), this.opts.negotiateTimeoutMs ?? 10_000)
    }
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') this.onHostText(host, ev.data)
    }
    const lost = (): void => {
      if (host.state === 'closed') return
      const beforeOpen = host.state === 'connecting'
      host.state = 'closed'
      // CSP 违规事件与 WebSocket 失败事件的先后不确定：下一轮任务再判断原因。
      if (beforeOpen) setTimeout(() => this.failHost(host, this.failure(url)), 0)
      else this.failHost(host, {})
    }
    ws.onclose = lost
    ws.onerror = lost
    return host
  }

  private failure(url: string): ChannelFailure {
    return this.opts.cspBlocked?.(url) ? { csp: true } : {}
  }

  private onHostText(host: HostConn, text: string): void {
    if (host.state === 'negotiating') {
      const ok = parseMuxReply(text)
      if (ok === undefined) return
      clearTimeout(host.timer)
      host.timer = undefined
      if (!ok) {
        this.unsupported.set(host.url, this.now())
        this.failHost(host, { unsupported: true })
        return
      }
      host.state = 'ready'
      for (const chan of [...host.chans.values()]) this.openChan(chan)
      return
    }
    if (host.state !== 'ready') return
    const frame = decodeFrame(text)
    if (!frame) return
    const chan = host.chans.get(frame.ch)
    if (!chan) return
    if (frame.type === 'msg') {
      this.deliver(chan.tab, { t: 'message', ch: chan.local, text: frame.text })
    } else if (frame.type === 'close') {
      this.removeChan(chan, false)
      this.deliver(chan.tab, frame.reason ? { t: 'closed', ch: chan.local, reason: frame.reason } : { t: 'closed', ch: chan.local })
    }
  }

  /** 连接失败或断开：以 `failure` 关闭其上全部通道。 */
  private failHost(host: HostConn, failure: ChannelFailure): void {
    const chans = [...host.chans.values()]
    host.chans.clear()
    this.closeHost(host)
    for (const chan of chans) {
      if (chan.tab.channels.get(chan.local) === chan) chan.tab.channels.delete(chan.local)
      this.deliver(chan.tab, { t: 'closed', ch: chan.local, ...failure })
    }
  }

  private closeHost(host: HostConn): void {
    if (this.hosts.get(host.url) === host) this.hosts.delete(host.url)
    clearTimeout(host.timer)
    host.timer = undefined
    const ws = host.ws
    host.state = 'closed'
    ws.onopen = null
    ws.onmessage = null
    ws.onclose = null
    ws.onerror = null
    try {
      ws.close()
    } catch {
      // 忽略
    }
  }
}
