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
import { normalizeToolResult, toJsonValue } from './result'
import { describeParseError, isZodLike, toJsonSchema, toOutputSchema } from './schema'
import { type BroadcastChannelFactory, InstanceGuard } from './instance-guard'
import { checkHandlerOrLoad, loadHandler } from './lazy'
import { channelDisconnectIssue, socketDisconnectIssue } from './disconnect'
import { hostTransport } from './host-transport'
import { MuxChannel, type MuxLink } from './mux/link'
import type { ChannelFailure } from './mux/protocol'
import { type ConnectionBlock, NetworkGuard, type PermissionState, type PermissionsLike } from './network-guard'
import { loadInstanceId, loadToken, saveToken } from './storage'
import { attachToolHub, type ToolHub, type ToolHubEvent, type ToolInfo, type ToolView } from './tool-hub'
import { ToolCallError } from './types'
import type {
  AppMcp,
  AppMcpOptions,
  ConnectionBlockCause,
  ConnectionBlockCode,
  ConnectionState,
  ErrorKind,
  HoldHandle,
  JsonSchema,
  LazyToolDefinition,
  Logger,
  OutputDefinition,
  OutputSchema,
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
/** 默认 Host 地址（spec/protocol.md 1.3）：合并端口上的 App 连接路径。 */
export const DEFAULT_HOST_URL = 'ws://127.0.0.1:7717/app'
/**
 * 未指定 `hostUrl` 时依次尝试的地址：Host 的默认端口被占用时按同一顺序改用备选端口（7717 → 7737 → 7757）。
 * 以握手结果核对对端身份（`service: "app-mcp"`），不是 app-mcp 的端口跳过。
 */
export const DEFAULT_HOST_URLS: readonly string[] = [
  DEFAULT_HOST_URL,
  'ws://127.0.0.1:7737/app',
  'ws://127.0.0.1:7757/app',
]

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
  /**
   * 共享连接（见 shared-connection.ts）：首次连接时调用一次，返回 undefined 表示不可用。
   * 缺省不共享（每个实例直接连接）；`createAppMcp` 在 `sharedConnection !== false` 时传入。
   */
  createSharedLink?: (cspBlocked: (url: string) => boolean) => MuxLink | undefined
  /** 本地网络访问权限查询，默认 `navigator.permissions`。 */
  permissions?: PermissionsLike
  /** 默认 `globalThis.isSecureContext`。 */
  isSecureContext?: boolean
  /** 被本地网络访问限制拦截时重新探测的间隔，默认 60000 毫秒。 */
  blockedRetryMs?: number
}

const defaultLogger: Logger = {
  debug: () => {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
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

/** handler 返回值 → 结果，结构化结果（`{ data, stateHints?, status?, … }`）会被拆开（见 result.ts）。 */
function outcomeFromResult(result: unknown): CoreOutcome {
  try {
    return normalizeToolResult(result)
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
  /** 候选地址：显式 `hostUrl` 时只有它，否则为 {@link DEFAULT_HOST_URLS}。 */
  private readonly hostUrls: readonly string[]
  /** 当前尝试的候选。 */
  private hostIndex = 0
  /** 自上次成功连接以来判定为"不是 app-mcp"的候选数（全部不是时停在 `host-mismatch`）。 */
  private mismatched = 0
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

  // ---- 传输 ----
  /** 浏览器拦截诊断（LNA / CSP）。 */
  private readonly net: NetworkGuard
  /** 共享连接：undefined 尚未创建，null 不可用。 */
  private link: MuxLink | null | undefined
  /** 共享连接在本页不可用（Host 不支持多路复用、SharedWorker 无法启动）：改为直接连接。 */
  private sharedUnavailable = false
  /** 连接被浏览器拦截：核心停在 `connecting`（无定时器），等待授权变化或 `wake()` 重试。 */
  private blocked: ConnectionBlock | undefined
  /** 当前连接是否已打开（区分"连接失败"与"连接断开"）。 */
  private wsOpened = false
  /** 拦截状态下的低频重新探测（LNA 判断依据是旁证，不能确定时不永久放弃）。 */
  private blockedTimer: ReturnType<typeof setTimeout> | undefined

  constructor(options: AppMcpOptions, deps: DriverDeps) {
    if (!APP_ID_RE.test(options.appId)) {
      throw new Error(`无效的 appId ${JSON.stringify(options.appId)}：应匹配 [a-z][a-z0-9-]{0,62}`)
    }
    this.options = Object.freeze({ ...options })
    this.deps = deps
    this.log = options.logger ?? defaultLogger
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

    const win = deps.window ?? (typeof window === 'undefined' ? undefined : window)
    const doc = deps.document ?? (typeof document === 'undefined' ? undefined : document)
    this.win = win
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
  private nextCandidate(): void {
    if (this.hostUrls.length > 1) this.hostIndex = (this.hostIndex + 1) % this.hostUrls.length
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
    if (this.retryBlocked()) return
    this.lifecycleOp((c) => c.wake(this.now()))
  }

  sleep(): void {
    this.lifecycleOp((c) => c.sleep(this.now()))
  }

  connectNow(): void {
    if (this.retryBlocked()) return
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
        this.log.error(`${this.tag} 停止核心失败`, e)
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
    this.net.dispose()
    this.blocked = undefined
    clearTimeout(this.blockedTimer)
    this.link?.dispose()
    this.link = null
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
      this.log.error(`${this.tag} WASM 核心加载失败`, e)
      this.setState({ status: 'rejected', reason: `WASM 核心加载失败：${errorMessage(e)}`, code: 'SDK_INIT_FAILED' })
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
        transport: hostTransport(this.hostUrls),
        ...(this.options.heartbeat !== undefined && { heartbeat: { mode: this.options.heartbeat } }),
      })
    } catch (e) {
      this.log.error(`${this.tag} 创建核心失败`, e)
      this.setState({ status: 'rejected', reason: `创建核心失败：${errorMessage(e)}`, code: 'SDK_INIT_FAILED' })
      return
    }
    this.core = core
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
  private get tag(): string {
    const cid = this.connectionId
    return cid ? `[app-mcp] [${cid}]` : '[app-mcp]'
  }

  private input(fn: (core: CoreClient) => void, quiet = false): void {
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

  private drainEvents(core: CoreClient): void {
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

  private armTimer(): void {
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
        else this.log.warn(`${this.tag} 连接未建立，丢弃消息`)
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
        if (ev.state.status === 'host-mismatch' && this.skipMismatchedCandidate(ev.state.reason)) break
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
      default:
        this.log.debug(`${this.tag} 未知事件`, ev)
    }
  }

  private mapState(state: CoreState): ConnectionState {
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

  private setState(state: ConnectionState): void {
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

  // ---- WebSocket ------------------------------------------------------

  private openSocket(): void {
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
  private openTransport(direct = false): void {
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

  private sharedLink(): MuxLink | undefined {
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
  private connectFailed(failure: ChannelFailure | undefined, viaShared: boolean): void {
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
  private skipMismatchedCandidate(reason: string): boolean {
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
  private block(block: ConnectionBlock): void {
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
  private retryBlocked(): boolean {
    if (!this.blocked || this.disposed) return false
    this.blocked = undefined
    clearTimeout(this.blockedTimer)
    this.blockedTimer = undefined
    const core = this.core
    if (core) this.setState(this.mapState(core.state()))
    if (!this.ws) this.openTransport()
    return true
  }

  private onPermissionChange(state: PermissionState): void {
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
    if (l.hostAbsentRetries !== undefined) lifecycle.hostAbsentRetries = l.hostAbsentRetries
    if (l.legacyTimers !== undefined) lifecycle.legacyTimers = l.legacyTimers
    if (l.mergeWindowMs !== undefined) lifecycle.mergeWindowMs = l.mergeWindowMs
    if (l.sleepOnBackground !== undefined) lifecycle.sleepOnBackground = l.sleepOnBackground
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
        this.log.debug(`${this.tag} 无法从地址栏移除唤醒令牌：${errorMessage(e)}`)
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
        this.log.error(`${this.tag} 工具注册监听器出错`, e)
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
      this.log.warn(`${this.tag} 实例已 dispose，忽略${kind} ${name} 的注册`)
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

    const schemas = this.convertSchemas(name, def.input, def.outputSchema)
    const register = (core: CoreClient, { inputSchema, outputSchema }: ToolSchemas): void => {
      if (rec.disposed) return
      const coreDef: CoreToolDef = { name, description: def.description, inputSchema }
      if (def.title !== undefined) coreDef.title = def.title
      if (def.risk !== undefined) coreDef.risk = def.risk
      if (def.annotations !== undefined) coreDef.annotations = def.annotations
      if (outputSchema !== undefined) coreDef.outputSchema = outputSchema
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
    this.enqueue((core) => (isPromise(schemas) ? schemas.then((s) => register(core, s)) : register(core, schemas)))
    return handle
  }

  private convertSchema(name: string, input: unknown): JsonSchema | Promise<JsonSchema> {
    try {
      return toJsonSchema(input as ToolDefinition['input'])
    } catch (e) {
      return Promise.reject(new Error(`工具 ${name} 的输入 schema 无效：${errorMessage(e)}`))
    }
  }

  private convertOutputSchema(name: string, output: OutputDefinition<unknown>): OutputSchema | Promise<OutputSchema> {
    try {
      return toOutputSchema(output)
    } catch (e) {
      return Promise.reject(new Error(`工具 ${name} 的输出 schema 无效：${errorMessage(e)}`))
    }
  }

  /** 输入与输出 schema；都能同步转换时同步返回（注册顺序不受影响），否则返回 Promise。 */
  private convertSchemas(
    name: string,
    input: unknown,
    output: OutputDefinition<unknown> | undefined,
  ): ToolSchemas | Promise<ToolSchemas> {
    const inputSchema = this.convertSchema(name, input)
    const outputSchema = output === undefined ? undefined : this.convertOutputSchema(name, output)
    if (!isPromise(inputSchema) && !isPromise(outputSchema)) return { inputSchema, outputSchema }
    return Promise.all([inputSchema, outputSchema]).then(([i, o]) => ({ inputSchema: i, outputSchema: o }))
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
        if ('annotations' in changes) update.annotations = changes.annotations ?? null
        if ('outputSchema' in changes && changes.outputSchema === undefined) update.outputSchema = null
        const schema = 'input' in changes ? this.convertSchema(rec.name, changes.input) : undefined
        const output =
          changes.outputSchema !== undefined ? this.convertOutputSchema(rec.name, changes.outputSchema) : undefined
        if (schema === undefined && output === undefined && Object.keys(update).length === 0) return
        const apply = (core: CoreClient, inputSchema: JsonSchema | undefined, outputSchema: OutputSchema | undefined): void => {
          if (rec.disposed || rec.coreId === undefined) return
          if (inputSchema !== undefined) update.inputSchema = inputSchema
          if (outputSchema !== undefined) update.outputSchema = outputSchema
          try {
            core.updateTool(rec.coreId, update)
          } catch (e) {
            throw new Error(`更新工具 ${rec.name} 失败：${errorMessage(e)}`)
          }
        }
        this.enqueue((core) =>
          isPromise(schema) || isPromise(output)
            ? Promise.all([schema, output]).then(([i, o]) => apply(core, i, o))
            : apply(core, schema, output),
        )
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
          ...(def.realtime && { realtime: true }),
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

/** 转换后的工具 schema。 */
interface ToolSchemas {
  inputSchema: JsonSchema
  outputSchema: OutputSchema | undefined
}

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

/** 拦截原因 → 错误码（spec/protocol.md 10.1）。 */
const BLOCK_CODE: Record<ConnectionBlockCause, ConnectionBlockCode> = {
  'local-network-access': 'BLOCKED_LOCAL_NETWORK_ACCESS',
  'insecure-context': 'BLOCKED_INSECURE_CONTEXT',
  csp: 'BLOCKED_CSP',
}

function blockedState(block: ConnectionBlock): ConnectionState {
  return { status: 'blocked', cause: block.cause, code: BLOCK_CODE[block.cause], message: block.message }
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
