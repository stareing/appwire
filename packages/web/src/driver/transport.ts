/**
 * 驱动层传输与生命周期：WebSocket / 共享连接、浏览器拦截、可见性、页面生命周期、
 * 网页唤醒交接、持有与导航（自 driver.ts 拆出）。
 */

import type { CoreClient, CoreConfig, CoreNavigateOutcome } from '../core'
import { channelDisconnectIssue, socketDisconnectIssue } from '../disconnect'
import { MuxChannel, type MuxLink } from '../mux/link'
import type { ChannelFailure } from '../mux/protocol'
import { navigationParams, runNavigation } from '../navigation'
import type { ConnectionBlock, PermissionState } from '../network-guard'
import type { HoldHandle } from '../types'
import type { VisibilitySnapshot } from '../visibility'
import { DriverBase } from './base'
import {
  BLOCK_CODE,
  blockedState,
  detach,
  errorMessage,
  HANDOFF_NOTICE,
  parseWakeTokenJs,
  stripWakeFragment,
  WAKE_PARAM,
  type WebSocketLike,
} from './shared'

export abstract class DriverTransport extends DriverBase {
  // ---- WebSocket ------------------------------------------------------

  protected openSocket(): void {
    this.closeSocket()
    if (this.blocked) return
    // 已观察到 CSP 拦截：不再尝试
    const pre = this.net.cspBlocks(this.hostUrl) ? this.net.diagnose() : undefined
    if (pre) {
      this.block(pre)
      return
    }
    this.openTransport()
  }

  /**
   * 选择传输并打开：共享连接可用时经它打开一个通道，否则（或 `direct`）直接连接。
   */
  protected openTransport(direct = false): void {
    let ws: WebSocketLike | undefined
    if (!direct && !this.sharedUnavailable) {
      const link = this.sharedLink()
      if (link) ws = link.open(this.hostUrl)
    }
    if (!ws) {
      try {
        const factory =
          this.deps.createWebSocket ?? ((url: string) => new WebSocket(url) as unknown as WebSocketLike)
        ws = factory(this.hostUrl)
      } catch (e) {
        this.log.warn(`${this.tag} 无法连接 ${this.hostUrl}：${errorMessage(e)}`)
        queueMicrotask(() => {
          if (!this.ws && !this.disposed) this.connectFailed(undefined, false)
        })
        return
      }
    }
    this.ws = ws
    this.wsOpened = false
    const socket = ws
    socket.onopen = () => {
      if (this.ws !== socket) return
      this.wsOpened = true
      // 重新探测成功：解除拦截（状态随核心的 connected 更新）
      this.blocked = undefined
      clearTimeout(this.blockedTimer)
      this.blockedTimer = undefined
      this.input((c) => c.handleConnected(this.now()))
    }
    socket.onmessage = (e) => {
      if (this.ws !== socket) return
      if (typeof e.data === 'string') {
        const text = e.data
        this.input((c) => c.handleMessage(text, this.now()))
      } else this.log.warn(`${this.tag} 忽略非文本消息`)
    }
    const lost = (ev: unknown): void => {
      if (this.ws !== socket) return
      this.ws = null
      detach(socket)
      if (this.wsOpened) {
        // 已建立的连接断开：带错误码进入 backoff（spec/protocol.md 10.1，与原生驱动层一致）
        const issue = socket instanceof MuxChannel ? channelDisconnectIssue(socket.failure) : socketDisconnectIssue(ev)
        this.log.debug(`${this.tag} [${issue.code}] ${issue.message}`)
        this.input((c) => c.handleDisconnectedWith(issue.code, issue.message, this.now()))
      } else if (socket instanceof MuxChannel) this.connectFailed(socket.failure, true)
      else this.connectFailed(undefined, false)
    }
    socket.onclose = lost
    socket.onerror = lost
  }

  protected sharedLink(): MuxLink | undefined {
    if (this.link === undefined) {
      try {
        // 主标签页模式下持有方运行在页面里，用页面的 CSP 违规记录判断拦截
        this.link = this.deps.createSharedLink?.((url) => this.net.cspBlocks(url)) ?? null
      } catch (e) {
        this.log.debug(`${this.tag} 共享连接不可用：${errorMessage(e)}`)
        this.link = null
      }
    }
    return this.link ?? undefined
  }

  /**
   * 连接在打开之前失败：判断是否被浏览器拦截，否则交给核心按断开处理（退避重连）。
   * `viaShared` 为经共享连接打开的通道。
   */
  protected connectFailed(failure: ChannelFailure | undefined, viaShared: boolean): void {
    if (this.disposed) return
    if (failure?.unsupported) {
      // 能力协商结果（旧 Host 不支持多路复用 / SharedWorker 无法启动）：本页改为直接连接，核心仍在连接中
      this.sharedUnavailable = true
      this.log.debug(`${this.tag} 共享连接不可用（${failure.reason ?? 'Host 不支持多路复用'}），改为直接连接`)
      this.openTransport()
      return
    }
    if (viaShared && this.net.lnaApplies && this.net.lnaPermission === 'prompt') {
      // 本地网络访问尚未授权：worker 中的请求不会弹出授权提示，本次改由页面直接连接（可弹出提示）
      this.openTransport(true)
      return
    }
    const block = this.net.diagnose(failure?.csp === true)
    if (block) {
      this.block(block)
      return
    }
    if (this.blocked) {
      // 重新探测时不再满足拦截条件（如授权已变化）：回到普通的退避重连
      this.blocked = undefined
    }
    // 连接没能建立：下次重连尝试下一个候选端口（未指定 hostUrl 时）。浏览器不给出失败原因；浏览器拦截
    // （LNA / CSP / 非安全上下文）已在上面排除，余下按"Host 不在"归为 HOST_NOT_RUNNING（spec/protocol.md 10.1），
    // idle / on-demand 下连续多次即停止重连（spec/lifecycle.md 第 11 节）。
    const url = this.hostUrl
    this.nextCandidate()
    this.input((c) =>
      c.handleConnectFailed('HOST_NOT_RUNNING', `无法连接 ${url}（Host 未运行，或连接被拒绝）`, this.now()),
    )
  }

  /**
   * 核心判定对端不是 app-mcp Host：还有没试过的候选端口时换下一个并立即连接（不对外报告该状态），
   * 返回 `true`；候选都不是 app-mcp（或显式指定了 hostUrl）时返回 `false`，状态停在 `host-mismatch`。
   */
  protected skipMismatchedCandidate(reason: string): boolean {
    this.mismatched += 1
    if (this.mismatched >= this.hostUrls.length) {
      this.log.warn(`${this.tag} ${reason}`)
      this.mismatched = 0
      return false
    }
    this.log.debug(`${this.tag} ${this.hostUrl} 不是 app-mcp Host（${reason}），尝试下一个候选端口`)
    this.nextCandidate()
    const core = this.core
    if (core) {
      try {
        core.connectNow(this.now())
      } catch (e) {
        this.log.error(`${this.tag} ${errorMessage(e)}`, e)
      }
    }
    return true
  }

  /**
   * 进入拦截状态：不通知核心（核心停在 connecting，没有退避定时器）。
   * CSP 拦截确定无疑，只在 `wake()` / `connectNow()` 时重试；本地网络访问的判断依据是授权状态这一旁证
   * （浏览器版本不同，WebSocket 不一定受限，失败也可能只是 Host 未运行），因此除授权变化时立即重连外，
   * 还按 `blockedRetryMs`（默认 60 秒）低频重新探测，状态保持 `blocked` 直到连接成功。
   */
  protected block(block: ConnectionBlock): void {
    const changed = this.blocked?.cause !== block.cause || this.blocked.message !== block.message
    this.blocked = block
    if (changed) {
      this.log.warn(`${this.tag} 连接被浏览器拦截：${block.message}`)
      // 拦截期间 Host 无从得知；记下来，连接恢复后经 app/diagnostic 上报（spec/protocol.md 10.2）
      this.input((c) => c.reportIssue(BLOCK_CODE[block.cause], block.message), true)
    }
    this.setState(blockedState(block))
    clearTimeout(this.blockedTimer)
    this.blockedTimer = undefined
    if (block.cause === 'csp') return
    this.blockedTimer = setTimeout(() => {
      this.blockedTimer = undefined
      if (this.blocked && !this.disposed && !this.ws) this.openTransport()
    }, this.deps.blockedRetryMs ?? 60_000)
  }

  /** 被拦截时立即重试一次；返回是否处于拦截状态。 */
  protected retryBlocked(): boolean {
    if (!this.blocked || this.disposed) return false
    this.blocked = undefined
    clearTimeout(this.blockedTimer)
    this.blockedTimer = undefined
    const core = this.core
    if (core) this.setState(this.mapState(core.state()))
    if (!this.ws) this.openTransport()
    return true
  }

  protected onPermissionChange(state: PermissionState): void {
    if (this.disposed) return
    if (this.blocked?.cause === 'local-network-access' || this.blocked?.cause === 'insecure-context') {
      const block = this.net.diagnose()
      if (block && state !== 'granted') this.block(block)
      else {
        this.log.debug(`${this.tag} 本地网络访问授权变为 ${state}，重新连接`)
        this.retryBlocked()
      }
      return
    }
    // 退避中获得授权：立即重连
    if (state === 'granted' && this.currentState.status === 'backoff') this.input((c) => c.wake(this.now()))
  }

  /** 主动关闭（核心的 Disconnect 事件或 dispose），不通知核心。 */
  protected closeSocket(): void {
    const ws = this.ws
    if (!ws) return
    this.ws = null
    detach(ws)
    try {
      ws.close()
    } catch {
      // 忽略
    }
  }

  // ---- 可见性 ---------------------------------------------------------

  protected onVisibility(s: VisibilitySnapshot): void {
    const prev = this.lastVisibility
    this.lastVisibility = s.visibility
    this.input((c) => c.setVisibility(s.visibility, s.focused, this.now()))
    if (s.visibility === 'visible' && prev !== 'visible' && this.mode !== 'persistent') {
      // 休眠中（或休眠握手进行中，对外仍为 connected）重新可见：回连
      const status = this.currentState.status
      if (status === 'dormant' || status === 'connected') this.input((c) => c.wakeWithReason('visible', this.now()))
    }
  }

  // ---- 生命周期 -------------------------------------------------------

  protected coreLifecycle(): NonNullable<CoreConfig['lifecycle']> {
    const l = this.options.lifecycle ?? {}
    const lifecycle: NonNullable<CoreConfig['lifecycle']> = { mode: this.mode }
    if (l.idleTimeoutMs !== undefined) lifecycle.idleTimeoutMs = l.idleTimeoutMs
    if (l.hiddenIdleTimeoutMs !== undefined) lifecycle.hiddenIdleTimeoutMs = l.hiddenIdleTimeoutMs
    if (l.graceMs !== undefined) lifecycle.graceMs = l.graceMs
    if (l.hostAbsentRetries !== undefined) lifecycle.hostAbsentRetries = l.hostAbsentRetries
    if (l.legacyTimers !== undefined) lifecycle.legacyTimers = l.legacyTimers
    if (l.mergeWindowMs !== undefined) lifecycle.mergeWindowMs = l.mergeWindowMs
    if (l.sleepOnBackground !== undefined) lifecycle.sleepOnBackground = l.sleepOnBackground
    const href = this.currentHref()
    if (href !== undefined) lifecycle.wake = { kind: 'web-url', target: stripWakeFragment(href) ?? href, background: false }
    return lifecycle
  }

  protected currentHref(): string | undefined {
    try {
      const href = this.win?.location?.href
      return typeof href === 'string' && href !== '' ? href : undefined
    } catch {
      return undefined
    }
  }

  /** 地址中有唤醒令牌（`#app-mcp-wake=<token>`）时交给核心，并从地址栏移除该片段（保留其他 hash）。 */
  protected consumeWakeUrl(core: CoreClient): void {
    const href = this.currentHref()
    if (href === undefined) return
    let token: string | undefined
    try {
      token = this.parseWake(href) ?? undefined
    } catch {
      token = undefined
    }
    if (token === undefined) return
    this.stripWakeFromAddressBar(href)
    core.handleWake(href, this.now())
  }

  protected stripWakeFromAddressBar(href: string): void {
    const stripped = stripWakeFragment(href)
    if (stripped === undefined) return
    try {
      this.win?.history?.replaceState(this.win.history.state, '', stripped)
    } catch (e) {
      this.log.debug(`${this.tag} 无法从地址栏移除唤醒令牌：${errorMessage(e)}`)
    }
  }

  // ---- 网页唤醒交接（wake-handoff.ts） ----

  /** 地址中有唤醒令牌时发出交接；没有令牌或没有通道时返回 undefined。 */
  protected offerWakeHandoff(): Promise<boolean> | undefined {
    const href = this.currentHref()
    if (!this.handoff || href === undefined) return undefined
    const token = parseWakeTokenJs(href)
    return token === undefined ? undefined : this.handoff.offer(token)
  }

  /**
   * 令牌已由原标签页接手：移除地址栏中的令牌并尝试关闭本标签页。
   * 返回 true 表示本标签页正在关闭（不再创建核心）；关不掉时显示提示，之后按普通标签页继续。
   */
  protected leaveAfterHandoff(): boolean {
    const href = this.currentHref()
    if (href !== undefined) this.stripWakeFromAddressBar(href)
    this.log.debug(`${this.tag} 唤醒令牌已交给原标签页`)
    const win = this.win
    try {
      win?.close()
      // @why 规范中 close() 被允许时立即置 closed（is closing），不需要定时器确认
      if (win?.closed === true) return true
    } catch (e) {
      this.log.debug(`${this.tag} 无法关闭本标签页：${errorMessage(e)}`)
    }
    this.showHandoffNotice()
    return false
  }

  protected showHandoffNotice(): void {
    const doc = this.doc
    if (!doc?.body || typeof doc.createElement !== 'function') return
    const note = doc.createElement('div')
    note.setAttribute('role', 'status')
    note.setAttribute('data-app-mcp-handoff', '')
    note.textContent = HANDOFF_NOTICE
    note.style.cssText =
      'position:fixed;left:50%;bottom:16px;transform:translateX(-50%);z-index:2147483647;max-width:calc(100% - 32px);' +
      'padding:8px 14px;border-radius:8px;background:rgba(32,33,36,.92);color:#fff;font:14px/1.4 system-ui,sans-serif;cursor:pointer'
    note.addEventListener('click', () => note.remove())
    doc.body.appendChild(note)
  }

  /** @invariant 只有休眠（或等待重连）中的实例认领；连接中 / 已连接的实例不需要令牌。 */
  protected canClaimWake(): boolean {
    if (!this.core || this.disposed) return false
    const status = this.currentState.status
    return status === 'dormant' || status === 'backoff'
  }

  protected acceptHandedWake(token: string): boolean {
    if (!this.core || this.disposed) return false
    let accepted = false
    this.input((c) => {
      accepted = c.handleWake(`#${WAKE_PARAM}${token}`, this.now())
    })
    if (accepted) this.log.debug(`${this.tag} 接手新标签页交来的唤醒令牌`)
    return accepted
  }

  protected watchPage(win: Window | undefined, doc: Document | undefined): void {
    const listen = (target: EventTarget | undefined, type: string, fn: (ev: Event) => void): void => {
      if (!target) return
      target.addEventListener(type, fn)
      this.pageListeners.push(() => target.removeEventListener(type, fn))
    }
    // bfcache：进入缓存前立即休眠，恢复时回连（所有模式）
    // 共享连接：进入 bfcache 时请持有方暂存消息（向缓存中的页面投递消息会使其被逐出），恢复时取回；
    // 页面卸载时关闭本页的全部通道（不等 Host 心跳超时）。
    listen(win, 'pagehide', (ev) => {
      if ((ev as PageTransitionEvent).persisted) {
        this.sleepForPage()
        this.link?.park()
      } else {
        this.link?.dispose()
        this.link = null
      }
    })
    listen(win, 'pageshow', (ev) => {
      if (!(ev as PageTransitionEvent).persisted) return
      this.link?.unpark()
      this.resumeFromPage()
    })
    // Page Lifecycle 冻结：idle / on-demand 模式下休眠，恢复时回连
    listen(doc, 'freeze', () => {
      if (this.mode !== 'persistent') this.sleepForPage()
    })
    listen(doc, 'resume', () => this.resumeFromPage())
    // 页面已打开时 Host 聚焦同一地址（只改变 hash）
    listen(win, 'hashchange', () => {
      if (this.core) this.input((c) => this.consumeWakeUrl(c))
    })
  }

  protected sleepForPage(): void {
    if (!this.core) return
    this.input((c) => {
      if (c.sleepWithReason('background', this.now())) this.sleptForPage = true
    })
  }

  protected resumeFromPage(): void {
    if (!this.sleptForPage) return
    this.sleptForPage = false
    // 可见性恢复可能已触发回连
    const status = this.currentState.status
    if (status === 'waking' || status === 'connecting' || status === 'handshaking') return
    this.input((c) => c.wakeWithReason('visible', this.now()))
  }

  /** 生命周期操作：核心已加载则立即执行，否则在 start 之后执行。 */
  protected lifecycleOp(op: (core: CoreClient) => void): void {
    if (this.disposed) return
    if (this.core) this.input(op)
    else this.pendingLifecycle.push(op)
  }

  /** 获取持有。`callId` 为进行中的 Host 调用时映射为 `holdForCall`，否则为普通持有。 */
  protected acquireHold(callId: string | undefined): HoldHandle {
    let id: number | undefined
    let released = false
    this.lifecycleOp((core) => {
      if (released) return
      const now = this.now()
      if (callId !== undefined) {
        try {
          id = core.holdForCall(callId, now)
          return
        } catch {
          // 调用已结束或为本地调用（WebMCP），退回普通持有
        }
      }
      id = core.hold(now)
    })
    return {
      release: () => {
        if (released) return
        released = true
        const held = id
        if (held !== undefined) this.input((c) => c.releaseHold(held, this.now()), true)
      },
    }
  }

  /** handler 的 `context.progress()`：调用已结束 / 取消、核心未加载时丢弃（进度只是提示）。 */
  protected reportProgress(callId: string, signal: AbortSignal, progress: number, total?: number, message?: string): void {
    if (signal.aborted) return
    // @why 调用刚结束（结果已提交）时核心报未知调用：进度不影响结果，只记 debug。
    this.input((core) => core.reportProgress(callId, progress, total, message, this.now()), true)
  }

  // ---- 导航 -----------------------------------------------------------

  /** Host 的 `app/navigate`（spec/protocol.md 3.4）：按 {@link runNavigation} 执行，结果交给核心回复。 */
  protected navigate(id: number, page: string, params: unknown): void {
    const request = { page, params: navigationParams(params) }
    void runNavigation(this.navHandler, request, this.navOptions, { doc: this.doc, win: this.win }).then((result) => {
      if (this.disposed) return
      const outcome: CoreNavigateOutcome = result.ok
        ? {}
        : { error: { kind: result.kind, message: result.message, details: result.details } }
      this.input((c) => c.completeNavigate(id, outcome), true)
    })
  }
}
