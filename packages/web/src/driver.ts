/**
 * JS 驱动层：把 sans-IO 核心（{@link CoreClient}）接到浏览器的 WebSocket、定时器、
 * 页面可见性与 JS handler 上。
 *
 * 结构：
 * - 注册操作先进入 {@link AppMcpDriver.ops} 队列，核心加载完成后按顺序执行；
 *   需要异步转换 schema 的操作会阻塞其后的操作，保证顺序与加载前后行为一致。
 * - 每次向核心输入（`handle*` / `complete*` / 注册）后调用 {@link AppMcpDriver.pump}：
 *   取尽事件并处理，然后按 `pollTimeout()` 重设定时器。
 */

import type {
  CancelReason,
  CoreClient,
  CoreConfig,
  CoreEvent,
  CoreLoader,
  CoreOutcome,
  CoreState,
  CoreToolDef,
  CoreToolUpdate,
} from './core'
import { describeParseError, isZodLike, toJsonSchema } from './schema'
import { type BroadcastChannelFactory, InstanceGuard } from './instance-guard'
import { checkHandlerOrLoad, loadHandler } from './lazy'
import { loadInstanceId, loadToken, saveToken } from './storage'
import { attachToolHub, type ToolHub, type ToolHubEvent, type ToolInfo, type ToolView } from './tool-hub'
import { ToolCallError } from './types'
import type {
  AppMcp,
  AppMcpOptions,
  ConnectionState,
  ErrorKind,
  HoldHandle,
  JsonSchema,
  LazyToolDefinition,
  Logger,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ToolDefinition,
  ToolHandle,
  ToolHandler,
  ToolHandlerLoader,
} from './types'
import { type VisibilitySnapshot, type VisibilityWatcher, watchVisibility } from './visibility'

export const SDK_VERSION = '0.1.0'
export const DEFAULT_HOST_URL = 'ws://127.0.0.1:7717'

const APP_ID_RE = /^[a-z][a-z0-9-]{0,62}$/
const NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/
const WS_OPEN = 1
/** 一次输入后连续处理已到期定时器的上限（防止核心时钟异常导致死循环）。 */
const MAX_IMMEDIATE_TIMEOUTS = 16
/** setTimeout 的最大延迟（2^31 - 1）。 */
const MAX_TIMER_DELAY = 0x7fffffff

/** 驱动层需要的 WebSocket 子集（便于测试替换）。 */
export interface WebSocketLike {
  readonly readyState: number
  send(data: string): void
  close(code?: number, reason?: string): void
  onopen: ((ev: unknown) => void) | null
  onmessage: ((ev: { data: unknown }) => void) | null
  onclose: ((ev: unknown) => void) | null
  onerror: ((ev: unknown) => void) | null
}

export type WebSocketFactory = (url: string) => WebSocketLike

export interface DriverDeps {
  loadCore: CoreLoader
  /** 默认使用全局 WebSocket。 */
  createWebSocket?: WebSocketFactory
  /** 核心时钟，默认 `performance.now()`。 */
  now?: () => number
  /** 墙钟，用于把 `retryAt` 转换为 `Date.now()` 刻度，默认 `Date.now()`。 */
  wallNow?: () => number
  window?: Window
  document?: Document
  /**
   * 用于检测复制标签页导致的 instanceId 冲突（见 instance-guard.ts）。
   * 缺省不检测；`createAppMcp` 传入全局 `BroadcastChannel`。
   */
  createBroadcastChannel?: BroadcastChannelFactory
  /** 冲突探测的等待窗口（毫秒），默认 60。 */
  instanceProbeMs?: number
}

const defaultLogger: Logger = {
  debug: () => {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** 转换为可 JSON 序列化的值（语义同 JSON.stringify：Date → 字符串，undefined → null）。 */
function toJsonValue(value: unknown): unknown {
  if (value === undefined) return null
  const text = JSON.stringify(value)
  return text === undefined ? null : JSON.parse(text)
}

function isToolCallErrorLike(e: unknown): e is ToolCallError {
  return (
    e instanceof ToolCallError ||
    (e instanceof Error && e.name === 'ToolCallError' && typeof (e as { kind?: unknown }).kind === 'string')
  )
}

function errorOutcome(kind: ErrorKind, message: string, details?: Record<string, unknown>): CoreOutcome {
  return { error: details === undefined ? { kind, message } : { kind, message, details } }
}

function outcomeFromError(e: unknown): CoreOutcome {
  if (isToolCallErrorLike(e)) {
    let details: Record<string, unknown> | undefined
    try {
      details = e.details === undefined ? undefined : (toJsonValue(e.details) as Record<string, unknown>)
    } catch {
      details = undefined
    }
    return errorOutcome(e.kind, e.message, details ?? undefined)
  }
  return errorOutcome('HANDLER_ERROR', errorMessage(e) || 'handler 出错')
}

/** handler 返回值 → 结果。`{ data, stateHints? }` 形式会被拆开。 */
function outcomeFromResult(result: unknown): CoreOutcome {
  let data: unknown = result
  let stateHints: string[] | undefined
  if (typeof result === 'object' && result !== null && !Array.isArray(result) && 'data' in result) {
    const keys = Object.keys(result)
    const hints = (result as { stateHints?: unknown }).stateHints
    if (keys.every((k) => k === 'data' || k === 'stateHints') && (hints === undefined || Array.isArray(hints))) {
      data = (result as { data: unknown }).data
      stateHints = hints?.map(String)
    }
  }
  try {
    const json = toJsonValue(data)
    return stateHints && stateHints.length > 0 ? { data: json, stateHints } : { data: json }
  } catch (e) {
    return errorOutcome('HANDLER_ERROR', `handler 返回值无法序列化为 JSON：${errorMessage(e)}`)
  }
}

const CANCEL_KIND: Record<CancelReason, [ErrorKind, string]> = {
  requested: ['CANCELLED', '调用已被取消'],
  timeout: ['TIMEOUT', '调用超时'],
  disconnected: ['APP_DISCONNECTED', '与 Host 的连接已断开'],
  stopped: ['CANCELLED', 'SDK 已停止'],
}

// ---------------------------------------------------------------------------
// 内部记录
// ---------------------------------------------------------------------------

interface ScopeRec {
  name: string
  parent: ScopeRec | undefined
  coreId: number | undefined
  disposed: boolean
  children: Set<ScopeRec>
  tools: Set<ToolRec>
  resources: Set<ResourceRec>
}

interface ToolRec {
  name: string
  /** 当前定义（不含 handler / anchor），供内部钩子读取。 */
  info: ToolInfo
  view: ToolView | undefined
  /** 惰性工具在加载完成前为 undefined。 */
  handler: ToolHandler<any, any> | undefined
  /** 惰性加载器（`LazyToolDefinition.load`）。 */
  load: ToolHandlerLoader<any, any> | undefined
  /** 进行中的加载（并发调用共享）。 */
  loading: Promise<ToolHandler<any, any>> | undefined
  parse: ((input: unknown) => unknown) | undefined
  anchor: ToolDefinition['anchor']
  scope: ScopeRec | undefined
  coreId: number | undefined
  disposed: boolean
}

interface ResourceRec {
  name: string
  read: ResourceDefinition<any>['read']
  scope: ScopeRec | undefined
  coreId: number | undefined
  disposed: boolean
}

type Op = (core: CoreClient) => void | Promise<void>

// ---------------------------------------------------------------------------
// 驱动
// ---------------------------------------------------------------------------

export class AppMcpDriver implements AppMcp {
  readonly options: Readonly<AppMcpOptions>

  private readonly log: Logger
  private readonly hostUrl: string
  private readonly deps: DriverDeps
  private readonly now: () => number
  private readonly wallNow: () => number

  private core: CoreClient | undefined
  private disposed = false
  private currentState: ConnectionState = { status: 'idle' }
  private readonly listeners = new Set<(state: ConnectionState) => void>()

  private readonly ops: Op[] = []
  private draining = false
  private pumping = false

  private ws: WebSocketLike | null = null
  private timer: ReturnType<typeof setTimeout> | undefined
  private readonly visibility: VisibilityWatcher

  private readonly toolNames = new Map<string, ToolRec>()
  private readonly resourceNames = new Map<string, ResourceRec>()
  private readonly toolsByCoreId = new Map<number, ToolRec>()
  private readonly resourcesByCoreId = new Map<number, ResourceRec>()
  private readonly rootTools = new Set<ToolRec>()
  private readonly rootResources = new Set<ResourceRec>()
  private readonly rootScopes = new Set<ScopeRec>()
  private readonly calls = new Map<string, AbortController>()
  private readonly hubListeners = new Set<(event: ToolHubEvent) => void>()
  private readonly hub: ToolHub
  private readonly guard: InstanceGuard

  // ---- 生命周期 ----
  private readonly mode: 'persistent' | 'idle' | 'on-demand'
  private readonly win: Window | undefined
  /** 核心加载前调用的 wake / sleep / hold 等，在 start 之后按顺序执行。 */
  private readonly pendingLifecycle: Op[] = []
  /** 因 bfcache / 冻结而休眠：页面恢复时回连。 */
  private sleptForPage = false
  private lastVisibility: VisibilitySnapshot['visibility']
  private readonly pageListeners: Array<() => void> = []
  private parseWake: (args: string) => string | undefined = parseWakeTokenJs

  constructor(options: AppMcpOptions, deps: DriverDeps) {
    if (!APP_ID_RE.test(options.appId)) {
      throw new Error(`无效的 appId ${JSON.stringify(options.appId)}：应匹配 [a-z][a-z0-9-]{0,62}`)
    }
    this.options = Object.freeze({ ...options })
    this.deps = deps
    this.log = options.logger ?? defaultLogger
    this.hostUrl = options.hostUrl ?? DEFAULT_HOST_URL
    this.now = deps.now ?? (() => (typeof performance !== 'undefined' ? performance.now() : Date.now()))
    this.wallNow = deps.wallNow ?? (() => Date.now())
    this.guard = new InstanceGuard({
      appId: options.appId,
      instanceId: loadInstanceId(options.appId),
      createChannel: deps.createBroadcastChannel,
      ...(deps.instanceProbeMs !== undefined && { windowMs: deps.instanceProbeMs }),
      onRegenerated: (prev, next) =>
        this.log.warn(`[app-mcp] 检测到其他标签页使用相同的 instanceId（复制标签页？），已由 ${prev} 改为 ${next}`),
      onLateConflict: (id) =>
        this.log.warn(`[app-mcp] 连接建立后才发现其他标签页使用相同的 instanceId ${id}，刷新本页可解决`),
    })

    const win = deps.window ?? (typeof window === 'undefined' ? undefined : window)
    const doc = deps.document ?? (typeof document === 'undefined' ? undefined : document)
    this.win = win
    this.mode = options.lifecycle?.mode ?? 'persistent'
    this.visibility = watchVisibility((s) => this.onVisibility(s), win, doc)
    this.lastVisibility = this.visibility.current().visibility
    this.watchPage(win, doc)

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

  // ---- AppMcp ---------------------------------------------------------

  get instanceId(): string {
    return this.guard.instanceId
  }

  get state(): ConnectionState {
    return this.currentState
  }

  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle {
    return this.registerTool(name, definition as AnyDef, undefined)
  }

  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return this.registerResource(name, definition, undefined)
  }

  scope(name: string): Scope {
    return this.createScope(name, undefined)
  }

  wake(): void {
    this.lifecycleOp((c) => c.wake(this.now()))
  }

  sleep(): void {
    this.lifecycleOp((c) => c.sleep(this.now()))
  }

  connectNow(): void {
    this.lifecycleOp((c) => c.connectNow(this.now()))
  }

  hold(): HoldHandle {
    return this.acquireHold(undefined)
  }

  dispose(): void {
    if (this.disposed) return
    const core = this.core
    if (core) {
      try {
        core.stop(this.now())
      } catch (e) {
        this.log.error('[app-mcp] 停止核心失败', e)
      }
      this.pump()
    }
    this.disposed = true
    this.ops.length = 0
    for (const c of this.calls.values()) c.abort(new ToolCallError('CANCELLED', 'SDK 已停止'))
    this.calls.clear()
    this.pendingLifecycle.length = 0
    this.closeSocket()
    this.clearTimer()
    this.visibility.dispose()
    for (const off of this.pageListeners.splice(0)) off()
    this.guard.dispose()
    for (const rec of [...this.toolNames.values()]) this.forgetTool(rec)
    this.hubListeners.clear()
    this.hub.yieldable.clear()
    this.toolNames.clear()
    this.resourceNames.clear()
    this.toolsByCoreId.clear()
    this.resourcesByCoreId.clear()
    this.core = undefined
    try {
      core?.free?.()
    } catch {
      // 忽略
    }
    this.setState({ status: 'stopped' })
    this.listeners.clear()
  }

  // ---- 加载 -----------------------------------------------------------

  private async load(): Promise<void> {
    let factory
    // 冲突探测与 WASM 加载并行；核心创建时 instanceId 已确定。
    const probe = this.guard.active ? this.guard.probe() : undefined
    try {
      factory = await this.deps.loadCore(this.options.wasmUrl)
      if (probe) await probe
    } catch (e) {
      if (this.disposed) return
      this.log.error('[app-mcp] WASM 核心加载失败', e)
      this.setState({ status: 'rejected', reason: `WASM 核心加载失败：${errorMessage(e)}` })
      return
    }
    if (this.disposed) return
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
        ...(this.options.overview !== undefined && { overview: this.options.overview }),
        ...tokenField(loadToken(this.options.appId)),
        lifecycle: this.coreLifecycle(),
      })
    } catch (e) {
      this.log.error('[app-mcp] 创建核心失败', e)
      this.setState({ status: 'rejected', reason: `创建核心失败：${errorMessage(e)}` })
      return
    }
    this.core = core
    const vis = this.visibility.current()
    try {
      core.setVisibility(vis.visibility, vis.focused, this.now())
    } catch (e) {
      this.log.error('[app-mcp] 设置可见性失败', e)
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
          this.log.error(`[app-mcp] ${errorMessage(e)}`, e)
        }
      }
    })
    this.drain()
  }

  // ---- 操作队列 -------------------------------------------------------

  private enqueue(op: Op): void {
    if (this.disposed) return
    this.ops.push(op)
    this.drain()
  }

  private drain(): void {
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
          this.log.error(`[app-mcp] ${errorMessage(e)}`, e)
          continue
        }
        if (result && typeof (result as Promise<void>).then === 'function') {
          ;(result as Promise<void>)
            .catch((e: unknown) => this.log.error(`[app-mcp] ${errorMessage(e)}`, e))
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
  private input(fn: (core: CoreClient) => void, quiet = false): void {
    const core = this.core
    if (!core || this.disposed) return
    try {
      fn(core)
    } catch (e) {
      if (quiet) this.log.debug(`[app-mcp] ${errorMessage(e)}`)
      else this.log.error(`[app-mcp] ${errorMessage(e)}`, e)
    }
    this.pump()
  }

  // ---- 事件循环 -------------------------------------------------------

  private pump(): void {
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
          this.log.error(`[app-mcp] ${errorMessage(e)}`, e)
          break
        }
        this.drainEvents(core)
      }
    } finally {
      this.pumping = false
    }
    this.armTimer()
  }

  private drainEvents(core: CoreClient): void {
    for (;;) {
      let ev: CoreEvent | undefined
      try {
        ev = core.pollEvent()
      } catch (e) {
        this.log.error('[app-mcp] pollEvent 失败', e)
        break
      }
      if (ev === undefined) break
      try {
        this.handleEvent(ev)
      } catch (e) {
        this.log.error(`[app-mcp] 处理事件 ${ev.type} 失败`, e)
      }
    }
  }

  private armTimer(): void {
    this.clearTimer()
    const core = this.core
    if (!core || this.disposed) return
    let at: number | undefined
    try {
      at = core.pollTimeout()
    } catch (e) {
      this.log.error('[app-mcp] pollTimeout 失败', e)
      return
    }
    if (at === undefined || at === null) return
    const delay = Math.min(Math.max(0, Math.ceil(at - this.now())), MAX_TIMER_DELAY)
    this.timer = setTimeout(() => {
      this.timer = undefined
      this.input((c) => c.handleTimeout(this.now()))
    }, delay)
  }

  private clearTimer(): void {
    if (this.timer !== undefined) {
      clearTimeout(this.timer)
      this.timer = undefined
    }
  }

  private handleEvent(ev: CoreEvent): void {
    switch (ev.type) {
      case 'connect':
        this.openSocket()
        break
      case 'disconnect':
        this.closeSocket()
        break
      case 'send':
        if (this.ws && this.ws.readyState === WS_OPEN) this.ws.send(ev.text)
        else this.log.warn('[app-mcp] 连接未建立，丢弃消息')
        break
      case 'invokeTool':
        this.invoke(ev.callId, ev.tool, ev.name, ev.arguments)
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
        this.setState(this.mapState(ev.state))
        break
      case 'paired':
        if (!saveToken(this.options.appId, ev.token)) this.log.warn('[app-mcp] 无法持久化配对 token')
        break
      case 'warning':
        this.log.warn(`[app-mcp] ${ev.message}`)
        break
      case 'idleExit':
        // Web 没有进程驻留概念，忽略
        break
      default:
        this.log.debug('[app-mcp] 未知事件', ev)
    }
  }

  private mapState(state: CoreState): ConnectionState {
    if (state.status === 'backoff') {
      return { status: 'backoff', retryAt: Math.round(this.wallNow() + (state.retryAt - this.now())) }
    }
    return state
  }

  private setState(state: ConnectionState): void {
    const prev = this.currentState
    if (JSON.stringify(prev) === JSON.stringify(state)) return
    this.currentState = state
    for (const l of [...this.listeners]) {
      try {
        l(state)
      } catch (e) {
        this.log.error('[app-mcp] onStateChange 监听器出错', e)
      }
    }
  }

  // ---- WebSocket ------------------------------------------------------

  private openSocket(): void {
    this.closeSocket()
    let ws: WebSocketLike
    try {
      const factory =
        this.deps.createWebSocket ?? ((url: string) => new WebSocket(url) as unknown as WebSocketLike)
      ws = factory(this.hostUrl)
    } catch (e) {
      this.log.warn(`[app-mcp] 无法连接 ${this.hostUrl}：${errorMessage(e)}`)
      queueMicrotask(() => this.input((c) => c.handleDisconnected(this.now())))
      return
    }
    this.ws = ws
    ws.onopen = () => {
      if (this.ws === ws) this.input((c) => c.handleConnected(this.now()))
    }
    ws.onmessage = (e) => {
      if (this.ws !== ws) return
      if (typeof e.data === 'string') {
        const text = e.data
        this.input((c) => c.handleMessage(text, this.now()))
      } else this.log.warn('[app-mcp] 忽略非文本消息')
    }
    const lost = (): void => {
      if (this.ws !== ws) return
      this.ws = null
      detach(ws)
      this.input((c) => c.handleDisconnected(this.now()))
    }
    ws.onclose = lost
    ws.onerror = lost
  }

  /** 主动关闭（核心的 Disconnect 事件或 dispose），不通知核心。 */
  private closeSocket(): void {
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

  private onVisibility(s: VisibilitySnapshot): void {
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

  private coreLifecycle(): NonNullable<CoreConfig['lifecycle']> {
    const l = this.options.lifecycle ?? {}
    const lifecycle: NonNullable<CoreConfig['lifecycle']> = { mode: this.mode }
    if (l.idleTimeoutMs !== undefined) lifecycle.idleTimeoutMs = l.idleTimeoutMs
    if (l.hiddenIdleTimeoutMs !== undefined) lifecycle.hiddenIdleTimeoutMs = l.hiddenIdleTimeoutMs
    if (l.graceMs !== undefined) lifecycle.graceMs = l.graceMs
    const href = this.currentHref()
    if (href !== undefined) lifecycle.wake = { kind: 'web-url', target: stripWakeFragment(href) ?? href, background: false }
    return lifecycle
  }

  private currentHref(): string | undefined {
    try {
      const href = this.win?.location?.href
      return typeof href === 'string' && href !== '' ? href : undefined
    } catch {
      return undefined
    }
  }

  /** 地址中有唤醒令牌（`#app-mcp-wake=<token>`）时交给核心，并从地址栏移除该片段（保留其他 hash）。 */
  private consumeWakeUrl(core: CoreClient): void {
    const href = this.currentHref()
    if (href === undefined) return
    let token: string | undefined
    try {
      token = this.parseWake(href) ?? undefined
    } catch {
      token = undefined
    }
    if (token === undefined) return
    const stripped = stripWakeFragment(href)
    if (stripped !== undefined) {
      try {
        this.win?.history?.replaceState(this.win.history.state, '', stripped)
      } catch (e) {
        this.log.debug(`[app-mcp] 无法从地址栏移除唤醒令牌：${errorMessage(e)}`)
      }
    }
    core.handleWake(href, this.now())
  }

  private watchPage(win: Window | undefined, doc: Document | undefined): void {
    const listen = (target: EventTarget | undefined, type: string, fn: (ev: Event) => void): void => {
      if (!target) return
      target.addEventListener(type, fn)
      this.pageListeners.push(() => target.removeEventListener(type, fn))
    }
    // bfcache：进入缓存前立即休眠，恢复时回连（所有模式）
    listen(win, 'pagehide', (ev) => {
      if ((ev as PageTransitionEvent).persisted) this.sleepForPage()
    })
    listen(win, 'pageshow', (ev) => {
      if ((ev as PageTransitionEvent).persisted) this.resumeFromPage()
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

  private sleepForPage(): void {
    if (!this.core) return
    this.input((c) => {
      if (c.sleepWithReason('background', this.now())) this.sleptForPage = true
    })
  }

  private resumeFromPage(): void {
    if (!this.sleptForPage) return
    this.sleptForPage = false
    // 可见性恢复可能已触发回连
    const status = this.currentState.status
    if (status === 'waking' || status === 'connecting' || status === 'handshaking') return
    this.input((c) => c.wakeWithReason('visible', this.now()))
  }

  /** 生命周期操作：核心已加载则立即执行，否则在 start 之后执行。 */
  private lifecycleOp(op: (core: CoreClient) => void): void {
    if (this.disposed) return
    if (this.core) this.input(op)
    else this.pendingLifecycle.push(op)
  }

  /** 获取持有。`callId` 为进行中的 Host 调用时映射为 `holdForCall`，否则为普通持有。 */
  private acquireHold(callId: string | undefined): HoldHandle {
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

  // ---- 调用 -----------------------------------------------------------

  private invoke(callId: string, toolId: number, name: string, args: unknown): void {
    const rec = this.toolsByCoreId.get(toolId)
    if (!rec) {
      this.input(
        (c) => c.completeCall(callId, errorOutcome('TOOL_NOT_FOUND', `工具 ${name} 不存在或已注销`), this.now()),
        true,
      )
      return
    }
    const controller = new AbortController()
    this.calls.get(callId)?.abort(new ToolCallError('CANCELLED', '重复的调用 ID'))
    this.calls.set(callId, controller)
    void this.runTool(rec, args, callId, controller.signal).then((outcome) =>
      this.finishCall(callId, controller, outcome),
    )
  }

  /** 执行 handler（含 zod 校验），结果与异常都转换为 {@link CoreOutcome}。 */
  private async runTool(rec: ToolRec, args: unknown, callId: string, signal: AbortSignal): Promise<CoreOutcome> {
    try {
      await Promise.resolve()
      let input = args
      if (rec.parse) {
        try {
          input = rec.parse(args)
        } catch (e) {
          const { message, details } = describeParseError(e)
          return errorOutcome('INVALID_INPUT', message, details)
        }
      }
      if (signal.aborted) throw signal.reason
      let handler = rec.handler
      if (!handler) {
        try {
          handler = await loadHandler(rec)
        } catch (e) {
          return errorOutcome('HANDLER_ERROR', `加载工具 ${rec.name} 的 handler 失败：${errorMessage(e)}`)
        }
        if (signal.aborted) throw signal.reason
      }
      return outcomeFromResult(await handler(input, { callId, signal, hold: () => this.acquireHold(callId) }))
    } catch (e) {
      return outcomeFromError(e)
    }
  }

  // ---- 内部钩子 -------------------------------------------------------

  private emitHub(event: ToolHubEvent): void {
    for (const l of [...this.hubListeners]) {
      try {
        l(event)
      } catch (e) {
        this.log.error('[app-mcp] 工具注册监听器出错', e)
      }
    }
  }

  private toolView(rec: ToolRec): ToolView {
    if (!rec.view) {
      let seq = 0
      rec.view = {
        name: rec.name,
        get definition() {
          return rec.info
        },
        call: (input, signal) => this.runTool(rec, input, `local-${rec.name}-${++seq}`, signal),
      }
    }
    return rec.view
  }

  private finishCall(callId: string, controller: AbortController, outcome: CoreOutcome): void {
    if (this.calls.get(callId) === controller) this.calls.delete(callId)
    // 已取消 / 超时的调用由核心回复，丢弃结果
    if (controller.signal.aborted || this.disposed) return
    this.input((c) => c.completeCall(callId, outcome, this.now()), true)
  }

  private readResource(readId: number, resourceId: number, name: string): void {
    const rec = this.resourcesByCoreId.get(resourceId)
    if (!rec) {
      this.input((c) => c.completeRead(readId, errorOutcome('RESOURCE_NOT_FOUND', `资源 ${name} 不存在或已注销`)), true)
      return
    }
    const run = async (): Promise<CoreOutcome> => {
      await Promise.resolve()
      return { data: toJsonValue(await rec.read()) }
    }
    run().then(
      (outcome) => this.finishRead(readId, outcome),
      (e: unknown) => this.finishRead(readId, outcomeFromError(e)),
    )
  }

  private finishRead(readId: number, outcome: CoreOutcome): void {
    if (this.disposed) return
    this.input((c) => c.completeRead(readId, outcome), true)
  }

  // ---- 注册 -----------------------------------------------------------

  private assertUsable(kind: string, name: string): boolean {
    if (this.disposed) {
      this.log.warn(`[app-mcp] 实例已 dispose，忽略${kind} ${name} 的注册`)
      return false
    }
    return true
  }

  private registerTool(name: string, def: AnyDef, scope: ScopeRec | undefined): ToolHandle {
    if (!NAME_RE.test(name)) throw new Error(`无效的工具名 ${JSON.stringify(name)}：应匹配 [a-zA-Z0-9_.-]{1,64}`)
    checkHandlerOrLoad(name, def)
    const { handler: _handler, load: _load, anchor: _anchor, ...info } = def
    const rec: ToolRec = {
      name,
      info,
      view: undefined,
      handler: def.handler,
      load: def.load,
      loading: undefined,
      parse: isZodLike(def.input) ? (x) => (def.input as { parse(x: unknown): unknown }).parse(x) : undefined,
      anchor: def.anchor,
      scope,
      coreId: undefined,
      disposed: false,
    }
    const handle = this.toolHandle(rec)
    if (!this.assertUsable('工具', name) || scope?.disposed) {
      rec.disposed = true
      return handle
    }
    if (this.toolNames.has(name)) {
      // 可让位的工具（如经 WebMCP 标准接口注册的）让给 appMcp.tool()
      const evict = this.hub.yieldable.get(name)
      if (evict) {
        this.hub.yieldable.delete(name)
        evict()
      }
    }
    if (this.toolNames.has(name)) throw new Error(`工具 ${JSON.stringify(name)} 已注册`)
    this.toolNames.set(name, rec)
    ;(scope ? scope.tools : this.rootTools).add(rec)
    this.emitHub({ type: 'register', tool: this.toolView(rec) })

    const schema = this.convertSchema(name, def.input)
    const register = (core: CoreClient, inputSchema: JsonSchema): void => {
      if (rec.disposed) return
      const coreDef: CoreToolDef = { name, description: def.description, inputSchema }
      if (def.title !== undefined) coreDef.title = def.title
      if (def.risk !== undefined) coreDef.risk = def.risk
      if (def.activation !== undefined) coreDef.activation = def.activation
      if (def.enabled !== undefined) coreDef.enabled = def.enabled
      if (scope) {
        if (scope.coreId === undefined) throw new Error(`scope ${scope.name} 未创建，无法注册工具 ${name}`)
        coreDef.scope = scope.coreId
      }
      try {
        rec.coreId = core.registerTool(coreDef)
      } catch (e) {
        throw new Error(`注册工具 ${name} 失败：${errorMessage(e)}`)
      }
      this.toolsByCoreId.set(rec.coreId, rec)
    }
    this.enqueue((core) => (isPromise(schema) ? schema.then((s) => register(core, s)) : register(core, schema)))
    return handle
  }

  private convertSchema(name: string, input: unknown): JsonSchema | Promise<JsonSchema> {
    try {
      return toJsonSchema(input as ToolDefinition['input'])
    } catch (e) {
      return Promise.reject(new Error(`工具 ${name} 的输入 schema 无效：${errorMessage(e)}`))
    }
  }

  private toolHandle(rec: ToolRec): ToolHandle {
    return {
      name: rec.name,
      update: (changes) => {
        if (rec.disposed) return
        // 未出现的字段保持不变；显式给出 undefined 的字段恢复默认值（便于框架适配整体同步定义）。
        if ('input' in changes) {
          rec.parse = isZodLike(changes.input)
            ? (x) => (changes.input as { parse(x: unknown): unknown }).parse(x)
            : undefined
        }
        if ('anchor' in changes) rec.anchor = changes.anchor
        const info: Record<string, unknown> = { ...rec.info }
        for (const [k, v] of Object.entries(changes)) {
          if (k === 'anchor' || k === 'handler' || k === 'load') continue
          if (k === 'description' && v === undefined) continue
          if (v === undefined) delete info[k]
          else info[k] = v
        }
        rec.info = info as ToolInfo
        this.emitHub({ type: 'update', tool: this.toolView(rec) })
        const update: CoreToolUpdate = {}
        if (changes.description !== undefined) update.description = changes.description
        if ('risk' in changes) update.risk = changes.risk ?? 'write'
        if ('enabled' in changes) update.enabled = changes.enabled ?? true
        if ('title' in changes) update.title = changes.title ?? null
        if ('activation' in changes) update.activation = changes.activation ?? null
        const schema = 'input' in changes ? this.convertSchema(rec.name, changes.input) : undefined
        if (schema === undefined && Object.keys(update).length === 0) return
        const apply = (core: CoreClient, inputSchema: JsonSchema | undefined): void => {
          if (rec.disposed || rec.coreId === undefined) return
          if (inputSchema !== undefined) update.inputSchema = inputSchema
          try {
            core.updateTool(rec.coreId, update)
          } catch (e) {
            throw new Error(`更新工具 ${rec.name} 失败：${errorMessage(e)}`)
          }
        }
        this.enqueue((core) => (isPromise(schema) ? schema.then((s) => apply(core, s)) : apply(core, schema)))
      },
      setHandler: (handler) => {
        rec.handler = handler
        rec.load = undefined
        rec.loading = undefined
      },
      dispose: () => {
        if (rec.disposed) return
        this.forgetTool(rec)
        this.enqueue((core) => {
          if (rec.coreId === undefined) return
          this.toolsByCoreId.delete(rec.coreId)
          core.unregisterTool(rec.coreId)
        })
      },
    }
  }

  /** JS 侧注销（不操作核心）。 */
  private forgetTool(rec: ToolRec): void {
    rec.disposed = true
    ;(rec.scope ? rec.scope.tools : this.rootTools).delete(rec)
    if (this.toolNames.get(rec.name) === rec) {
      this.toolNames.delete(rec.name)
      this.emitHub({ type: 'unregister', name: rec.name })
    }
  }

  private forgetResource(rec: ResourceRec): void {
    rec.disposed = true
    if (this.resourceNames.get(rec.name) === rec) this.resourceNames.delete(rec.name)
    ;(rec.scope ? rec.scope.resources : this.rootResources).delete(rec)
  }

  private registerResource(name: string, def: ResourceDefinition<any>, scope: ScopeRec | undefined): ResourceHandle {
    if (!NAME_RE.test(name)) throw new Error(`无效的资源名 ${JSON.stringify(name)}：应匹配 [a-zA-Z0-9_.-]{1,64}`)
    const rec: ResourceRec = { name, read: def.read, scope, coreId: undefined, disposed: false }
    const handle: ResourceHandle = {
      name,
      notifyChanged: () => {
        // 核心加载前不可能有订阅，直接忽略
        if (rec.disposed || !this.core) return
        this.enqueue((core) => {
          if (rec.coreId !== undefined && !rec.disposed) core.notifyResourceChanged(rec.coreId, this.now())
        })
      },
      setReader: (read) => {
        rec.read = read
      },
      dispose: () => {
        if (rec.disposed) return
        this.forgetResource(rec)
        this.enqueue((core) => {
          if (rec.coreId === undefined) return
          this.resourcesByCoreId.delete(rec.coreId)
          core.unregisterResource(rec.coreId)
        })
      },
    }
    if (!this.assertUsable('资源', name) || scope?.disposed) {
      rec.disposed = true
      return handle
    }
    if (this.resourceNames.has(name)) throw new Error(`资源 ${JSON.stringify(name)} 已注册`)
    this.resourceNames.set(name, rec)
    ;(scope ? scope.resources : this.rootResources).add(rec)
    this.enqueue((core) => {
      if (rec.disposed) return
      if (scope && scope.coreId === undefined) throw new Error(`scope ${scope.name} 未创建，无法注册资源 ${name}`)
      try {
        rec.coreId = core.registerResource({
          name,
          description: def.description,
          mimeType: def.mimeType ?? 'application/json',
          ...(scope && { scope: scope.coreId }),
        })
      } catch (e) {
        throw new Error(`注册资源 ${name} 失败：${errorMessage(e)}`)
      }
      this.resourcesByCoreId.set(rec.coreId, rec)
    })
    return handle
  }

  private createScope(name: string, parent: ScopeRec | undefined): Scope {
    const rec: ScopeRec = {
      name,
      parent,
      coreId: undefined,
      disposed: false,
      children: new Set(),
      tools: new Set(),
      resources: new Set(),
    }
    const scope: Scope = {
      name,
      tool: (toolName, definition) => this.registerTool(toolName, definition as AnyDef, rec),
      resource: (resName, definition) => this.registerResource(resName, definition, rec),
      scope: (childName) => this.createScope(childName, rec),
      dispose: () => {
        if (rec.disposed) return
        this.forgetScope(rec)
        ;(parent ? parent.children : this.rootScopes).delete(rec)
        this.enqueue((core) => {
          if (rec.coreId === undefined) return
          core.disposeScope(rec.coreId)
        })
      },
    }
    if (this.disposed || parent?.disposed) {
      rec.disposed = true
      return scope
    }
    ;(parent ? parent.children : this.rootScopes).add(rec)
    this.enqueue((core) => {
      if (rec.disposed) return
      if (parent && parent.coreId === undefined) throw new Error(`父 scope ${parent.name} 未创建`)
      rec.coreId = core.createScope(name, parent?.coreId)
    })
    return scope
  }

  /** 递归标记 scope 及其内容为已注销（核心侧由 disposeScope 递归处理）。 */
  private forgetScope(rec: ScopeRec): void {
    rec.disposed = true
    for (const t of [...rec.tools]) {
      this.forgetTool(t)
      if (t.coreId !== undefined) this.toolsByCoreId.delete(t.coreId)
    }
    for (const r of [...rec.resources]) {
      this.forgetResource(r)
      if (r.coreId !== undefined) this.resourcesByCoreId.delete(r.coreId)
    }
    for (const c of [...rec.children]) this.forgetScope(c)
    rec.children.clear()
  }
}

type AnyDef = ToolDefinition<any, any> | LazyToolDefinition<any, any>

function isPromise<T>(v: T | Promise<T> | undefined): v is Promise<T> {
  return typeof v === 'object' && v !== null && typeof (v as Promise<T>).then === 'function'
}

function detach(ws: WebSocketLike): void {
  ws.onopen = null
  ws.onmessage = null
  ws.onclose = null
  ws.onerror = null
}

const WAKE_PARAM = 'app-mcp-wake='

/** JS 版唤醒令牌解析（只识别 URL 片段 `#app-mcp-wake=<token>`）；WASM 核心提供完整实现。 */
export function parseWakeTokenJs(args: string): string | undefined {
  const hash = args.indexOf('#')
  if (hash < 0) return undefined
  for (const part of args.slice(hash + 1).split('&')) {
    if (part.startsWith(WAKE_PARAM)) {
      const token = part.slice(WAKE_PARAM.length)
      if (/^[A-Za-z0-9._~-]{1,512}$/.test(token)) return token
    }
  }
  return undefined
}

/** 从 URL 片段中移除 `app-mcp-wake=…`（保留其他 hash 参数）；没有该片段时返回 undefined。 */
export function stripWakeFragment(href: string): string | undefined {
  const hash = href.indexOf('#')
  if (hash < 0) return undefined
  const parts = href.slice(hash + 1).split('&')
  const kept = parts.filter((p) => !p.startsWith(WAKE_PARAM))
  if (kept.length === parts.length) return undefined
  const base = href.slice(0, hash)
  return kept.length > 0 ? `${base}#${kept.join('&')}` : base
}

function tokenField(token: string | undefined): { token?: string } {
  return token ? { token } : {}
}

function pageInfo(): { origin?: string; instanceTitle?: string; instanceUrl?: string } {
  const info: { origin?: string; instanceTitle?: string; instanceUrl?: string } = {}
  try {
    if (typeof location !== 'undefined') {
      if (location.origin && location.origin !== 'null') info.origin = location.origin
      if (location.href) info.instanceUrl = location.href
    }
    if (typeof document !== 'undefined' && document.title) info.instanceTitle = document.title
  } catch {
    // 忽略
  }
  return info
}

export function createDriver(options: AppMcpOptions, deps: DriverDeps): AppMcp {
  return new AppMcpDriver(options, deps)
}
