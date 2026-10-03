/**
 * 驱动层共用部分：常量、依赖注入接口、内部记录类型与无状态辅助函数（自 driver.ts 拆出）。
 */

import type { CancelReason, CoreClient, CoreLoader, CoreOutcome } from '../core'
import { normalizeToolResult, toJsonValue } from '../result'
import type { MuxLink } from '../mux/link'
import type { BroadcastChannelFactory } from '../instance-guard'
import type { ConnectionBlock, PermissionsLike } from '../network-guard'
import type { ToolInfo, ToolView } from '../tool-hub'
import { ToolCallError } from '../types'
import type {
  ConnectionBlockCause,
  ConnectionBlockCode,
  ConnectionState,
  ErrorKind,
  JsonSchema,
  LazyToolDefinition,
  Logger,
  OutputSchema,
  ResourceDefinition,
  ScopeOptions,
  ToolDefinition,
  ToolHandler,
  ToolHandlerLoader,
} from '../types'
import type { ScopeChain, ViewDeclaration } from '../view'

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

export const APP_ID_RE = /^[a-z][a-z0-9-]{0,62}$/
export const NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/
export const WS_OPEN = 1
/** 一次输入后连续处理已到期定时器的上限（防止核心时钟异常导致死循环）。 */
export const MAX_IMMEDIATE_TIMEOUTS = 16
/** setTimeout 的最大延迟（2^31 - 1）。 */
export const MAX_TIMER_DELAY = 0x7fffffff

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
  /** 带唤醒令牌打开时把令牌交给已有休眠标签页的等待上限（毫秒），默认 1000（见 wake-handoff.ts）。 */
  wakeHandoffMs?: number
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

export const defaultLogger: Logger = {
  debug: () => {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

function isToolCallErrorLike(e: unknown): e is ToolCallError {
  return (
    e instanceof ToolCallError ||
    (e instanceof Error && e.name === 'ToolCallError' && typeof (e as { kind?: unknown }).kind === 'string')
  )
}

export function errorOutcome(kind: ErrorKind, message: string, details?: Record<string, unknown>): CoreOutcome {
  return { error: details === undefined ? { kind, message } : { kind, message, details } }
}

export function outcomeFromError(e: unknown): CoreOutcome {
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
export function outcomeFromResult(result: unknown): CoreOutcome {
  try {
    return normalizeToolResult(result)
  } catch (e) {
    return errorOutcome('HANDLER_ERROR', `handler 返回值无法序列化为 JSON：${errorMessage(e)}`)
  }
}

export const CANCEL_KIND: Record<CancelReason, [ErrorKind, string]> = {
  requested: ['CANCELLED', '调用已被取消'],
  timeout: ['TIMEOUT', '调用超时'],
  disconnected: ['APP_DISCONNECTED', '与 Host 的连接已断开'],
  stopped: ['CANCELLED', 'SDK 已停止'],
}


// ---------------------------------------------------------------------------
// 内部记录
// ---------------------------------------------------------------------------

export interface ScopeRec {
  name: string
  /** 其下工具的缺省界面声明（{@link ScopeOptions}）。 */
  options: ScopeOptions | undefined
  parent: ScopeRec | undefined
  coreId: number | undefined
  disposed: boolean
  children: Set<ScopeRec>
  tools: Set<ToolRec>
  resources: Set<ResourceRec>
}

export interface ToolRec {
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
  /** 界面声明与可见性门控（注册时创建；注册被忽略的记录为 undefined）。 */
  decl: ViewDeclaration | undefined
  /** 最近一次告诉核心的启用状态。 */
  coreEnabled: boolean
}

export interface ResourceRec {
  name: string
  read: ResourceDefinition<any>['read']
  scope: ScopeRec | undefined
  coreId: number | undefined
  disposed: boolean
}

export type Op = (core: CoreClient) => void | Promise<void>

export type AnyDef = ToolDefinition<any, any> | LazyToolDefinition<any, any>

/** 转换后的工具 schema。 */
export interface ToolSchemas {
  inputSchema: JsonSchema
  outputSchema: OutputSchema | undefined
}

/** scope 链（自近到远）上的界面声明。 */
export function scopeChain(scope: ScopeRec | undefined): ScopeChain {
  return function* () {
    for (let s = scope; s; s = s.parent) yield s.options
  }
}

/** 定义快照中的 surface / page 取生效值（含继承）。 */
export function withView(info: ToolInfo, rec: Pick<ViewDeclaration, 'surface' | 'page'>): ToolInfo {
  const next: ToolInfo = { ...info, surface: rec.surface }
  if (rec.page !== undefined) next.page = rec.page
  else delete next.page
  return next
}

export function isPromise<T>(v: T | Promise<T> | undefined): v is Promise<T> {
  return typeof v === 'object' && v !== null && typeof (v as Promise<T>).then === 'function'
}

export function detach(ws: WebSocketLike): void {
  ws.onopen = null
  ws.onmessage = null
  ws.onclose = null
  ws.onerror = null
}

export const WAKE_PARAM = 'app-mcp-wake='
export const HANDOFF_NOTICE = '已交给原来的标签页处理，可以关闭此标签页。'

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
export const BLOCK_CODE: Record<ConnectionBlockCause, ConnectionBlockCode> = {
  'local-network-access': 'BLOCKED_LOCAL_NETWORK_ACCESS',
  'insecure-context': 'BLOCKED_INSECURE_CONTEXT',
  csp: 'BLOCKED_CSP',
}

export function blockedState(block: ConnectionBlock): ConnectionState {
  return { status: 'blocked', cause: block.cause, code: BLOCK_CODE[block.cause], message: block.message }
}

export function tokenField(token: string | undefined): { token?: string } {
  return token ? { token } : {}
}

export function pageInfo(): { origin?: string; instanceTitle?: string; instanceUrl?: string } {
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
