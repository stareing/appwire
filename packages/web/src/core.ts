/**
 * JS 驱动层与 sans-IO 核心之间的接口。
 *
 * 生产环境由 WASM 导出的 `WasmClient`（bindings/wasm）实现；测试可以注入假实现。
 * 方法与 `app_mcp_core::Client` 一一对应，时间参数为 `performance.now()` 毫秒，句柄为数字。
 */

import type { NormalizedResult } from './result'
import type {
  Activation,
  AppOverview,
  CallDedupOptions,
  ContentAnnotations,
  ErrorKind,
  JsonSchema,
  OutputSchema,
  Risk,
  ToolAnnotations,
  ToolSurface,
  Visibility,
} from './types'

export interface CoreConfig {
  appId: string
  appName: string
  instanceId: string
  clientKind?: 'web' | 'native' | 'hybrid'
  sdkVersion?: string
  appVersion?: string
  origin?: string
  instanceTitle?: string
  instanceUrl?: string
  token?: string
  launchToken?: string
  reconnect?: { initialDelayMs?: number; maxDelayMs?: number; multiplier?: number }
  heartbeat?: { mode?: 'auto' | 'always' | 'off'; intervalMs?: number; timeoutMs?: number; hiddenTimeoutMs?: number }
  maxConcurrentCalls?: number
  resourceUpdateThrottleMs?: number
  /** App 总览，随 `app/hello` 发送。 */
  overview?: AppOverview
  /** 握手超时（毫秒），缺省 10000；0 表示不限。 */
  handshakeTimeoutMs?: number
  /** 生命周期策略，缺省 `persistent`（spec/lifecycle.md 第 3 节）。 */
  lifecycle?: CoreLifecycle
  /** 到 Host 的传输类别（spec/lifecycle.md 第 11 节），缺省未知（按远程处理）。 */
  transport?: 'ipc' | 'loopback' | 'remote'
  /** 调用去重（spec/protocol.md 3.3），缺省保留 5 分钟、最多 64 条；任一为 0 关闭。 */
  callDedup?: CallDedupOptions
}

export interface CoreWakeDescriptor {
  kind: 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'
  target?: string
  background?: boolean
}

export interface CoreLifecycle {
  mode?: 'persistent' | 'idle' | 'on-demand'
  idleTimeoutMs?: number
  hiddenIdleTimeoutMs?: number
  graceMs?: number
  residency?: 'keep' | 'exit-when-idle' | 'exit-always'
  wake?: CoreWakeDescriptor
  hostAbsentRetries?: number
  legacyTimers?: boolean
  mergeWindowMs?: number
  sleepOnBackground?: boolean
}

export type WakeReason = 'os-activation' | 'app' | 'visible' | 'cold-start'
export type SleepReason = 'idle' | 'grace' | 'background' | 'app'

export interface CoreToolDef {
  name: string
  description: string
  inputSchema: JsonSchema
  risk?: Risk
  activation?: Activation
  title?: string
  annotations?: ToolAnnotations
  outputSchema?: OutputSchema
  enabled?: boolean
  scope?: number
  /** 缺省 `app`（spec/protocol.md 3.4）。 */
  surface?: ToolSurface
  page?: string
}

/** 部分更新；缺省字段不变，`activation` / `title` / `annotations` / `outputSchema` / `page` 为 null 表示清除。 */
export interface CoreToolUpdate {
  description?: string
  inputSchema?: JsonSchema
  risk?: Risk
  activation?: Activation | null
  title?: string | null
  enabled?: boolean
  annotations?: ToolAnnotations | null
  outputSchema?: OutputSchema | null
  surface?: ToolSurface
  page?: string | null
}

export interface CoreResourceDef {
  name: string
  description: string
  mimeType?: string
  scope?: number
  realtime?: boolean
  annotations?: ContentAnnotations
}

export interface CoreToolError {
  kind: ErrorKind
  message: string
  details?: Record<string, unknown>
}

/** handler / 资源读取结果（成功结果的可选字段见 bindings/wasm/src/convert.rs `JsCallOutcome`）。 */
export type CoreOutcome = NormalizedResult | { error: CoreToolError }

/** 导航结果：`{}` 完成，或 `NAVIGATION_FAILED` / `NAVIGATION_DENIED` 错误（spec/protocol.md 3.4）。 */
export type CoreNavigateOutcome = Record<string, never> | { error: CoreToolError }

/** 核心连接状态（`retryAt` 为核心时钟，即 `performance.now()` 毫秒）。 */
export type CoreState =
  | { status: 'idle' }
  | { status: 'connecting' }
  | { status: 'handshaking' }
  | { status: 'pending-pairing' }
  | { status: 'connected' }
  | { status: 'backoff'; retryAt: number; reason?: string; code?: string }
  | { status: 'rejected'; reason: string; code: string }
  | { status: 'stopped' }
  | { status: 'dormant' }
  | { status: 'waking' }
  | { status: 'host-mismatch'; reason: string; code: string }

export type CancelReason = 'requested' | 'timeout' | 'disconnected' | 'stopped'

export type CoreEvent =
  | { type: 'connect' }
  | { type: 'disconnect' }
  | { type: 'send'; text: string }
  | { type: 'invokeTool'; callId: string; tool: number; name: string; arguments: unknown }
  | { type: 'cancelTool'; callId: string; reason: CancelReason }
  | { type: 'readResource'; read: number; resource: number; name: string }
  | { type: 'stateChanged'; state: CoreState }
  | { type: 'paired'; token: string }
  | { type: 'warning'; message: string }
  /** residency 允许时休眠完成后产生；Web 忽略。 */
  | { type: 'idleExit' }
  /** Host 请求导航（`app/navigate`，仅在 `setNavigation(true)` 时产生）；`params` 缺省为 null。 */
  | { type: 'navigate'; navigate: number; page: string; params: unknown }

export interface CoreClient {
  state(): CoreState
  token(): string | undefined
  start(now: number): void
  stop(now: number): void
  createScope(name: string, parent?: number): number
  disposeScope(scope: number): void
  registerTool(def: CoreToolDef): number
  updateTool(tool: number, update: CoreToolUpdate): void
  unregisterTool(tool: number): void
  registerResource(def: CoreResourceDef): number
  notifyResourceChanged(resource: number, now: number): void
  unregisterResource(resource: number): void
  setVisibility(visibility: Visibility, focused: boolean, now: number): void
  handleConnected(now: number): void
  handleDisconnected(now: number): void
  /** 建立连接失败（带错误码，spec/protocol.md 10.1），`backoff` 状态带 `reason` / `code`。不认识的码抛错。 */
  handleConnectFailed(code: string, message: string, now: number): void
  /** 已建立的连接断开（带错误码 `CONNECTION_CLOSED` / `CONNECTION_LOST`，spec/protocol.md 10.1），`backoff` 状态带 `reason` / `code`。 */
  handleDisconnectedWith(code: string, message: string, now: number): void
  handleMessage(text: string, now: number): void
  handleTimeout(now: number): void
  completeCall(callId: string, outcome: CoreOutcome, now: number): void
  completeRead(read: number, outcome: CoreOutcome): void
  /** 导航完成；连接已断开（请求已丢弃）时抛错。 */
  completeNavigate(navigate: number, outcome: CoreNavigateOutcome): void
  /** 是否处理 `app/navigate`（握手时声明 `capabilities.navigate`；连接后修改在下次连接生效）。 */
  setNavigation(enabled: boolean): void
  pollEvent(): CoreEvent | undefined
  pollTimeout(): number | undefined
  // ---- 生命周期（spec/lifecycle.md）----
  /** 处理唤醒参数（URL / `#app-mcp-wake=<token>`），不是本 SDK 的唤醒返回 false。 */
  handleWake(args: string, now: number): boolean
  wake(now: number): boolean
  wakeWithReason(reason: WakeReason, now: number): boolean
  connectNow(now: number): boolean
  sleep(now: number): boolean
  sleepWithReason(reason: SleepReason, now: number): boolean
  hold(now: number): number
  /** 调用不存在时抛错。 */
  holdForCall(callId: string, now: number): number
  /** 报告进行中调用的进度（spec/protocol.md 3.3）。调用不在执行中时抛错；未连接时丢弃。 */
  reportProgress(callId: string, progress: number, total: number | undefined, message: string | undefined, now: number): void
  releaseHold(hold: number, now: number): boolean
  toolsHash(): string
  resumeToken(): string | undefined
  // ---- 诊断（spec/protocol.md 第 10 节）----
  /** Host 为当前连接分配的连接 ID。 */
  connectionId(): string | undefined
  /** 记录一次连接问题，下次握手成功后以 `app/diagnostic` 上报给 Host。 */
  reportIssue(code: string, message: string): void
  /** 释放 WASM 内存（可选）。 */
  free?(): void
}

export type CoreFactory = ((config: CoreConfig) => CoreClient) & {
  /** 从 URL / 激活参数中提取唤醒令牌（不改变客户端状态）。WASM 加载器提供；缺省时驱动层用 JS 实现。 */
  parseWakeToken?: (args: string) => string | undefined
}

/** 异步加载核心，返回构造函数。 */
export type CoreLoader = (wasmUrl?: string | URL) => Promise<CoreFactory>
