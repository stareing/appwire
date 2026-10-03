/**
 * 驱动层基类：状态字段、核心加载、操作队列与事件循环（自 driver.ts 拆出）。
 * 传输 / 生命周期见 transport.ts，调用与注册见 registry.ts，对外 API 见 ../driver.ts。
 */

import type { CoreClient, CoreConfig, CoreEvent, CoreState } from '../core'
import { hostTransport } from '../host-transport'
import { InstanceGuard } from '../instance-guard'
import type { MuxLink } from '../mux/link'
import { type ConnectionBlock, NetworkGuard, type PermissionState, type PermissionsLike } from '../network-guard'
import { loadInstanceId, loadToken, saveToken } from '../storage'
import { attachToolHub, type ToolHub, type ToolHubEvent, type ToolView } from '../tool-hub'
import { ToolCallError } from '../types'
import type { AppMcpOptions, ConnectionState, Logger, NavigationHandler, NavigationOptions } from '../types'
import { type VisibilitySnapshot, type VisibilityWatcher, watchVisibility } from '../visibility'
import { WakeHandoff } from '../wake-handoff'
import {
  blockedState,
  CANCEL_KIND,
  APP_ID_RE,
  DEFAULT_HOST_URL,
  DEFAULT_HOST_URLS,
  defaultLogger,
  type DriverDeps,
  errorMessage,
  MAX_IMMEDIATE_TIMEOUTS,
  MAX_TIMER_DELAY,
  type Op,
  pageInfo,
  parseWakeTokenJs,
  type ResourceRec,
  type ScopeRec,
  SDK_VERSION,
  tokenField,
  type ToolRec,
  type WebSocketLike,
  WS_OPEN,
} from './shared'

export abstract class DriverBase {
  readonly options: Readonly<AppMcpOptions>

  protected readonly log: Logger
  /** 候选地址：显式 `hostUrl` 时只有它，否则为 {@link DEFAULT_HOST_URLS}。 */
  protected readonly hostUrls: readonly string[]
  /** 当前尝试的候选。 */
  protected hostIndex = 0
  /** 自上次成功连接以来判定为"不是 app-mcp"的候选数（全部不是时停在 `host-mismatch`）。 */
  protected mismatched = 0
  protected readonly deps: DriverDeps
  protected readonly now: () => number
  protected readonly wallNow: () => number

  protected core: CoreClient | undefined
  protected disposed = false
  protected currentState: ConnectionState = { status: 'idle' }
  protected readonly listeners = new Set<(state: ConnectionState) => void>()

  protected readonly ops: Op[] = []
  protected draining = false
  protected pumping = false

  protected ws: WebSocketLike | null = null
  protected timer: ReturnType<typeof setTimeout> | undefined
  protected readonly visibility: VisibilityWatcher
  /** 页面文档（`view` 工具门控与层栈）；没有时（非浏览器环境）不做门控。 */
  protected readonly doc: Document | undefined
  /** 导航回调（spec/protocol.md 3.4）。 */
  protected navHandler: NavigationHandler | null = null
  protected navOptions: NavigationOptions = {}
  /** 不可见时导航是否仍交给回调（{@link AppMcpOptions.navigateInBackground}）；核心加载时同步，核心缺省 false。 */
  protected navigateInBackground: boolean
  /** 最近一次握手时是否声明了导航能力。 */
  protected navDeclared = false

  protected readonly toolNames = new Map<string, ToolRec>()
  protected readonly resourceNames = new Map<string, ResourceRec>()
  protected readonly toolsByCoreId = new Map<number, ToolRec>()
  protected readonly resourcesByCoreId = new Map<number, ResourceRec>()
  protected readonly rootTools = new Set<ToolRec>()
  protected readonly rootResources = new Set<ResourceRec>()
  protected readonly rootScopes = new Set<ScopeRec>()
  protected readonly calls = new Map<string, AbortController>()
  protected readonly hubListeners = new Set<(event: ToolHubEvent) => void>()
  protected readonly hub: ToolHub
  protected readonly guard: InstanceGuard
  /** 网页唤醒交接（没有 BroadcastChannel 时为 undefined）。 */
  protected readonly handoff: WakeHandoff | undefined

  // ---- 生命周期 ----
  protected readonly mode: 'persistent' | 'idle' | 'on-demand'
  protected readonly win: Window | undefined
  /** 核心加载前调用的 wake / sleep / hold 等，在 start 之后按顺序执行。 */
  protected readonly pendingLifecycle: Op[] = []
  /** 因 bfcache / 冻结而休眠：页面恢复时回连。 */
  protected sleptForPage = false
  protected lastVisibility: VisibilitySnapshot['visibility']
  protected readonly pageListeners: Array<() => void> = []
  protected parseWake: (args: string) => string | undefined = parseWakeTokenJs

  // ---- 传输 ----
  /** 浏览器拦截诊断（LNA / CSP）。 */
  protected readonly net: NetworkGuard
  /** 共享连接：undefined 尚未创建，null 不可用。 */
  protected link: MuxLink | null | undefined
  /** 共享连接在本页不可用（Host 不支持多路复用、SharedWorker 无法启动）：改为直接连接。 */
  protected sharedUnavailable = false
  /** 连接被浏览器拦截：核心停在 `connecting`（无定时器），等待授权变化或 `wake()` 重试。 */
  protected blocked: ConnectionBlock | undefined
  /** 当前连接是否已打开（区分"连接失败"与"连接断开"）。 */
  protected wsOpened = false
  /** 拦截状态下的低频重新探测（LNA 判断依据是旁证，不能确定时不永久放弃）。 */
  protected blockedTimer: ReturnType<typeof setTimeout> | undefined

  constructor(options: AppMcpOptions, deps: DriverDeps) {
    if (!APP_ID_RE.test(options.appId)) {
      throw new Error(`无效的 appId ${JSON.stringify(options.appId)}：应匹配 [a-z][a-z0-9-]{0,62}`)
    }
    this.options = Object.freeze({ ...options })
    this.deps = deps
    this.log = options.logger ?? defaultLogger
    this.navigateInBackground = options.navigateInBackground ?? false
    this.hostUrls = options.hostUrl !== undefined ? [options.hostUrl] : DEFAULT_HOST_URLS
    this.now = deps.now ?? (() => (typeof performance !== 'undefined' ? performance.now() : Date.now()))
    this.wallNow = deps.wallNow ?? (() => Date.now())
    this.guard = new InstanceGuard({
      appId: options.appId,
      instanceId: loadInstanceId(options.appId),
      createChannel: deps.createBroadcastChannel,
      ...(deps.instanceProbeMs !== undefined && { windowMs: deps.instanceProbeMs }),
      onRegenerated: (prev, next) =>
        this.log.warn(`${this.tag} 检测到其他标签页使用相同的 instanceId（复制标签页？），已由 ${prev} 改为 ${next}`),
      onLateConflict: (id) =>
        this.log.warn(`${this.tag} 连接建立后才发现其他标签页使用相同的 instanceId ${id}，刷新本页可解决`),
    })

    this.handoff = deps.createBroadcastChannel
      ? new WakeHandoff({
          appId: options.appId,
          createChannel: deps.createBroadcastChannel,
          canClaim: () => this.canClaimWake(),
          acceptToken: (token) => this.acceptHandedWake(token),
          ...(deps.wakeHandoffMs !== undefined && { timeoutMs: deps.wakeHandoffMs }),
        })
      : undefined

    const win = deps.window ?? (typeof window === 'undefined' ? undefined : window)
    const doc = deps.document ?? (typeof document === 'undefined' ? undefined : document)
    this.win = win
    this.doc = doc
    this.mode = options.lifecycle?.mode ?? 'persistent'
    this.visibility = watchVisibility((s) => this.onVisibility(s), win, doc)
    this.lastVisibility = this.visibility.current().visibility
    this.watchPage(win, doc)
    if (deps.createSharedLink === undefined) this.link = null
    const nav = (globalThis as { navigator?: { permissions?: PermissionsLike } }).navigator
    this.net = new NetworkGuard(
      this.hostUrl,
      {
        pageUrl: this.currentHref(),
        isSecureContext: deps.isSecureContext ?? (globalThis as { isSecureContext?: boolean }).isSecureContext,
        permissions: deps.permissions ?? nav?.permissions,
        doc,
      },
      (state) => this.onPermissionChange(state),
    )

    this.hub = {
      list: () => [...this.toolNames.values()].map((rec) => this.toolView(rec)),
      subscribe: (listener) => {
        this.hubListeners.add(listener)
        return () => {
          this.hubListeners.delete(listener)
        }
      },
      yieldable: new Map(),
    }
    attachToolHub(this, this.hub)

    void this.load()
  }

  /** 当前连接（或正在尝试）的 Host 地址。 */
  get hostUrl(): string {
    return this.hostUrls[this.hostIndex] ?? DEFAULT_HOST_URL
  }

  /** 换到下一个候选地址；只有一个候选时不变。 */
  protected nextCandidate(): void {
    if (this.hostUrls.length > 1) this.hostIndex = (this.hostIndex + 1) % this.hostUrls.length
  }

  get instanceId(): string {
    return this.guard.instanceId
  }

  // ---- 加载 -----------------------------------------------------------

  protected async load(): Promise<void> {
    let factory
    // 冲突探测与 WASM 加载并行；核心创建时 instanceId 已确定。
    const probe = this.guard.active ? this.guard.probe() : undefined
    // 带唤醒令牌打开：先尝试交给已有的休眠标签页（与 WASM 加载并行）。
    const handoff = this.offerWakeHandoff()
    try {
      factory = await this.deps.loadCore(this.options.wasmUrl)
      if (probe) await probe
    } catch (e) {
      if (this.disposed) return
      this.log.error(`${this.tag} WASM 核心加载失败`, e)
      this.setState({ status: 'rejected', reason: `WASM 核心加载失败：${errorMessage(e)}`, code: 'SDK_INIT_FAILED' })
      return
    }
    const handedOff = handoff ? await handoff : false
    if (this.disposed) return
    if (handedOff && this.leaveAfterHandoff()) return
    if (typeof factory.parseWakeToken === 'function') this.parseWake = factory.parseWakeToken
    let core: CoreClient
    try {
      core = factory({
        appId: this.options.appId,
        appName: this.options.appName,
        instanceId: this.instanceId,
        clientKind: 'web',
        sdkVersion: SDK_VERSION,
        ...(this.options.appVersion !== undefined && { appVersion: this.options.appVersion }),
        ...pageInfo(),
        ...(this.options.maxConcurrentCalls !== undefined && { maxConcurrentCalls: this.options.maxConcurrentCalls }),
        ...(this.options.callDedup !== undefined && { callDedup: this.options.callDedup }),
        ...(this.options.overview !== undefined && { overview: this.options.overview }),
        ...tokenField(loadToken(this.options.appId)),
        lifecycle: this.coreLifecycle(),
        transport: hostTransport(this.hostUrls),
        ...(this.options.heartbeat !== undefined && { heartbeat: { mode: this.options.heartbeat } }),
      })
    } catch (e) {
      this.log.error(`${this.tag} 创建核心失败`, e)
      this.setState({ status: 'rejected', reason: `创建核心失败：${errorMessage(e)}`, code: 'SDK_INIT_FAILED' })
      return
    }
    this.core = core
    try {
      // 核心缺省不声明导航能力；加载前设置了导航回调时在首次握手前打开
      if (this.navHandler) core.setNavigation(true)
      if (this.navigateInBackground) core.setNavigateInBackground(true)
    } catch (e) {
      this.log.error(`${this.tag} 设置导航能力失败`, e)
    }
    const vis = this.visibility.current()
    try {
      core.setVisibility(vis.visibility, vis.focused, this.now())
    } catch (e) {
      this.log.error(`${this.tag} 设置可见性失败`, e)
    }
    // 加载前缓存的注册先执行，再启动连接，使首次全量同步包含全部工具。
    // 地址中的唤醒令牌在 start 之前交给核心（核心记录后在 start 时连接）。
    this.ops.push((c) => {
      this.consumeWakeUrl(c)
      c.start(this.now())
      for (const op of this.pendingLifecycle.splice(0)) {
        try {
          op(c)
        } catch (e) {
          this.log.error(`${this.tag} ${errorMessage(e)}`, e)
        }
      }
    })
    this.drain()
  }

  // ---- 操作队列 -------------------------------------------------------

  protected enqueue(op: Op): void {
    if (this.disposed) return
    this.ops.push(op)
    this.drain()
  }

  protected drain(): void {
    if (!this.core || this.draining) return
    this.draining = true
    const loop = (): void => {
      while (this.ops.length > 0) {
        const core = this.core
        if (!core || this.disposed) break
        const op = this.ops.shift() as Op
        let result: void | Promise<void>
        try {
          result = op(core)
        } catch (e) {
          this.log.error(`${this.tag} ${errorMessage(e)}`, e)
          continue
        }
        if (result && typeof (result as Promise<void>).then === 'function') {
          ;(result as Promise<void>)
            .catch((e: unknown) => this.log.error(`${this.tag} ${errorMessage(e)}`, e))
            .then(() => {
              this.pump()
              loop()
            })
          return
        }
      }
      this.draining = false
      this.pump()
    }
    loop()
  }

  /** 向核心输入并处理产生的事件。 */
  /** Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）。 */
  get connectionId(): string | undefined {
    try {
      return this.core?.connectionId() ?? undefined
    } catch {
      return undefined
    }
  }

  /** 日志前缀：连接期间带连接 ID（`[app-mcp] [3f9a1c-12]`），便于与 Host 日志对照。 */
  protected get tag(): string {
    const cid = this.connectionId
    return cid ? `[app-mcp] [${cid}]` : '[app-mcp]'
  }

  protected input(fn: (core: CoreClient) => void, quiet = false): void {
    const core = this.core
    if (!core || this.disposed) return
    try {
      fn(core)
    } catch (e) {
      if (quiet) this.log.debug(`${this.tag} ${errorMessage(e)}`)
      else this.log.error(`${this.tag} ${errorMessage(e)}`, e)
    }
    this.pump()
  }

  // ---- 事件循环 -------------------------------------------------------

  protected pump(): void {
    const core = this.core
    if (!core || this.pumping) return
    this.pumping = true
    try {
      this.drainEvents(core)
      // 已到期的定时器（pollTimeout ≤ now，如休眠被拒后立即重试）直接处理，不经 setTimeout(0)
      for (let i = 0; i < MAX_IMMEDIATE_TIMEOUTS && this.core === core && !this.disposed; i++) {
        let at: number | undefined
        try {
          at = core.pollTimeout()
        } catch {
          break
        }
        if (at === undefined || at === null || at > this.now()) break
        try {
          core.handleTimeout(this.now())
        } catch (e) {
          this.log.error(`${this.tag} ${errorMessage(e)}`, e)
          break
        }
        this.drainEvents(core)
      }
    } finally {
      this.pumping = false
    }
    this.armTimer()
  }

  protected drainEvents(core: CoreClient): void {
    for (;;) {
      let ev: CoreEvent | undefined
      try {
        ev = core.pollEvent()
      } catch (e) {
        this.log.error(`${this.tag} pollEvent 失败`, e)
        break
      }
      if (ev === undefined) break
      try {
        this.handleEvent(ev)
      } catch (e) {
        this.log.error(`${this.tag} 处理事件 ${ev.type} 失败`, e)
      }
    }
  }

  protected armTimer(): void {
    this.clearTimer()
    const core = this.core
    if (!core || this.disposed) return
    let at: number | undefined
    try {
      at = core.pollTimeout()
    } catch (e) {
      this.log.error(`${this.tag} pollTimeout 失败`, e)
      return
    }
    if (at === undefined || at === null) return
    const delay = Math.min(Math.max(0, Math.ceil(at - this.now())), MAX_TIMER_DELAY)
    this.timer = setTimeout(() => {
      this.timer = undefined
      this.input((c) => c.handleTimeout(this.now()))
    }, delay)
  }

  protected clearTimer(): void {
    if (this.timer !== undefined) {
      clearTimeout(this.timer)
      this.timer = undefined
    }
  }

  protected handleEvent(ev: CoreEvent): void {
    switch (ev.type) {
      case 'connect':
        this.openSocket()
        break
      case 'disconnect':
        this.closeSocket()
        break
      case 'send':
        if (this.ws && this.ws.readyState === WS_OPEN) this.ws.send(ev.text)
        else this.log.warn(`${this.tag} 连接未建立，丢弃消息`)
        break
      case 'invokeTool':
        this.invoke(ev.callId, ev.tool, ev.name, ev.arguments, ev.idempotencyKey)
        break
      case 'cancelTool': {
        const controller = this.calls.get(ev.callId)
        if (controller) {
          this.calls.delete(ev.callId)
          const [kind, message] = CANCEL_KIND[ev.reason] ?? CANCEL_KIND.requested
          controller.abort(new ToolCallError(kind, message))
        }
        break
      }
      case 'readResource':
        this.readResource(ev.read, ev.resource, ev.name)
        break
      case 'stateChanged':
        if (ev.state.status === 'host-mismatch' && this.skipMismatchedCandidate(ev.state.reason)) break
        if (ev.state.status === 'handshaking') this.navDeclared = this.navHandler !== null
        if (ev.state.status === 'connected') {
          this.mismatched = 0
          const cid = this.connectionId
          this.log.debug(`${this.tag} 已连接 Host（${cid ? `连接 ID ${cid}` : 'Host 未提供连接 ID'}）`)
        }
        this.setState(this.mapState(ev.state))
        break
      case 'paired':
        if (!saveToken(this.options.appId, ev.token)) this.log.warn(`${this.tag} 无法持久化配对 token`)
        break
      case 'warning':
        this.log.warn(`${this.tag} ${ev.message}`)
        break
      case 'idleExit':
        // Web 没有进程驻留概念，忽略
        break
      case 'navigate':
        this.navigate(ev.navigate, ev.page, ev.params)
        break
      default:
        this.log.debug(`${this.tag} 未知事件`, ev)
    }
  }

  protected mapState(state: CoreState): ConnectionState {
    if (this.blocked) {
      // 被拦截时核心停在 connecting / waking；其他状态（stopped 等）说明已不再连接
      if (state.status === 'connecting' || state.status === 'waking') return blockedState(this.blocked)
      this.blocked = undefined
    }
    if (state.status === 'backoff') {
      return {
        status: 'backoff',
        retryAt: Math.round(this.wallNow() + (state.retryAt - this.now())),
        ...(state.reason !== undefined && { reason: state.reason }),
        ...(state.code !== undefined && { code: state.code }),
      }
    }
    return state
  }

  protected setState(state: ConnectionState): void {
    const prev = this.currentState
    if (JSON.stringify(prev) === JSON.stringify(state)) return
    this.currentState = state
    for (const l of [...this.listeners]) {
      try {
        l(state)
      } catch (e) {
        this.log.error(`${this.tag} onStateChange 监听器出错`, e)
      }
    }
  }

  // ---- 子类实现（transport.ts / registry.ts）----

  protected abstract canClaimWake(): boolean
  protected abstract acceptHandedWake(token: string): boolean
  protected abstract onVisibility(s: VisibilitySnapshot): void
  protected abstract watchPage(win: Window | undefined, doc: Document | undefined): void
  protected abstract currentHref(): string | undefined
  protected abstract onPermissionChange(state: PermissionState): void
  protected abstract toolView(rec: ToolRec): ToolView
  protected abstract offerWakeHandoff(): Promise<boolean> | undefined
  protected abstract leaveAfterHandoff(): boolean
  protected abstract coreLifecycle(): NonNullable<CoreConfig['lifecycle']>
  protected abstract consumeWakeUrl(core: CoreClient): void
  protected abstract openSocket(): void
  protected abstract closeSocket(): void
  protected abstract skipMismatchedCandidate(reason: string): boolean
  protected abstract invoke(callId: string, toolId: number, name: string, args: unknown, idempotencyKey?: string): void
  protected abstract readResource(readId: number, resourceId: number, name: string): void
  protected abstract navigate(id: number, page: string, params: unknown): void
}
