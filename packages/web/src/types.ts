/**
 * @app-mcp/web 公开 API 类型。
 *
 * 这是 @app-mcp/web 与上层包（@app-mcp/react、@app-mcp/vue、示例）之间的契约，
 * 修改前需同步更新依赖它的包。协议层类型见 spec/protocol.md。
 */

// ---------------------------------------------------------------------------
// 基础类型（与协议一致）
// ---------------------------------------------------------------------------

export type Risk = 'read' | 'write' | 'destructive' | 'payment' | 'os-sensitive'
export type Activation = 'headless' | 'background' | 'foreground'
export type Visibility = 'visible' | 'hidden' | 'frozen'

/** 协议错误类别，见 spec/protocol.md 第 4 节。 */
export type ErrorKind =
  | 'TOOL_NOT_FOUND'
  | 'TOOL_DISABLED'
  | 'INVALID_INPUT'
  | 'USER_REJECTED'
  | 'TIMEOUT'
  | 'HANDLER_ERROR'
  | 'CANCELLED'
  | 'APP_DISCONNECTED'
  | 'APP_NOT_INSTALLED'
  | 'LAUNCH_FAILED'
  | 'APP_NOT_RESPONDING'
  | 'INSTANCE_FROZEN'
  | 'RESOURCE_NOT_FOUND'
  | 'UNAUTHORIZED'
  | 'UNSUPPORTED_PROTOCOL'

/** JSON Schema 对象（只要求顶层 type 为 object）。 */
export type JsonSchema = { type: 'object'; [key: string]: unknown }

/**
 * 工具输入定义。支持三种形式：
 * - JSON Schema 对象；
 * - zod v4 schema（需要安装 zod，内部用 `z.toJSONSchema` 转换，并在调用前用 `parse` 校验）；
 * - 任何带 `toJSONSchema()` 方法的对象。
 */
export type InputDefinition<I> =
  | JsonSchema
  | { toJSONSchema(): JsonSchema }
  | ZodLike<I>

/** zod v4 schema 的最小结构（避免强依赖 zod 类型）。 */
export interface ZodLike<I> {
  readonly _zod: unknown
  parse(input: unknown): I
}

/**
 * 连接被浏览器拦截的原因（`blocked` 状态，见 {@link ConnectionState}）：
 * - `local-network-access`：Chrome 本地网络访问（LNA）权限未授予——公网 / 局域网页面连接本机 Host 需要用户允许
 *   「本机上的应用」（`loopback-network`）；授权后自动重连；
 * - `insecure-context`：非 HTTPS 的公网页面，Chrome 禁止其连接本机且不询问用户（需改用 HTTPS 或在 localhost 打开）；
 * - `csp`：内容安全策略 `connect-src` 不允许连接 Host 地址（修改策略后刷新页面）。
 */
export type ConnectionBlockCause = 'local-network-access' | 'insecure-context' | 'csp'

export type ConnectionState =
  | { status: 'disabled' }
  | { status: 'idle' }
  | { status: 'connecting' }
  | { status: 'handshaking' }
  | { status: 'pending-pairing' }
  | { status: 'connected' }
  | { status: 'backoff'; retryAt: number }
  | { status: 'rejected'; reason: string }
  | { status: 'stopped' }
  /** 已与 Host 完成 `app/sleep` 握手后断开（或 `on-demand` 启动后尚未连接）；注册表保留，等待唤醒（spec/lifecycle.md 第 2 节）。 */
  | { status: 'dormant' }
  /** 收到唤醒（页面重新可见、URL 唤醒令牌、`wake()`）后正在回连，之后进入 `handshaking`。 */
  | { status: 'waking' }
  /**
   * 连接被浏览器拦截（不是 Host 未运行）：不再定时重试。`message` 是给用户 / 开发者的中文说明（可直接展示）。
   * 本地网络访问授权变为允许时自动重连；`wake()` / `connectNow()` 立即重试一次（可重新弹出授权提示）。
   */
  | { status: 'blocked'; cause: ConnectionBlockCause; message: string }
  /**
   * 对端不是 app-mcp Host（spec/protocol.md 1.6）：握手结果的 `service` 不是 `app-mcp`、不认识 `app/hello`
   * 或结果无法解析。使用默认地址时 SDK 先依次尝试候选端口（7717 → 7737 → 7757），都不是 app-mcp 时停在此状态；
   * 不再定时重试，`wake()` / `connectNow()` 时再试一次。`reason` 为中文说明。
   */
  | { status: 'host-mismatch'; reason: string }

/**
 * 生命周期策略（spec/lifecycle.md 第 3 节）。Web 没有进程驻留（`residency`）概念；
 * 唤醒描述由 SDK 自动填为 `{ kind: 'web-url', target: 当前页面地址 }`。
 */
export interface LifecycleOptions {
  /**
   * - `persistent`（默认）：启动即连接、一直在线；
   * - `idle`：启动时连接，空闲 `idleTimeoutMs` 后休眠，页面重新可见或被唤醒时回连；
   * - `on-demand`：启动时不连接，被唤醒或调用 `connectNow()` 时连接，任务完成后经过 `graceMs` 休眠。
   */
  mode?: 'persistent' | 'idle' | 'on-demand'
  /** 空闲多久进入休眠（`idle` 模式），默认 60000。 */
  idleTimeoutMs?: number
  /** 页面隐藏 / 冻结时使用的更短空闲时间，默认 15000。 */
  hiddenIdleTimeoutMs?: number
  /** `on-demand` 模式下任务完成后保留连接的时间，默认 10000。 */
  graceMs?: number
}

/**
 * 持有句柄（{@link AppMcp.hold}、{@link ToolContext.hold}）：调用 `release()` 释放，重复调用无效果。
 * 与 @app-mcp/node 的 `HoldHandle` 同形。
 */
export interface HoldHandle {
  release(): void
}

// ---------------------------------------------------------------------------
// 创建
// ---------------------------------------------------------------------------

export interface AppMcpOptions {
  /** `[a-z][a-z0-9-]{0,62}` */
  appId: string
  appName: string
  appVersion?: string
  /** 为 false 时不加载 WASM、不连接，所有注册调用为空操作。默认 true。 */
  enabled?: boolean
  /**
   * Host 地址。缺省时依次尝试 `ws://127.0.0.1:7717/app`、`ws://127.0.0.1:7737/app`、`ws://127.0.0.1:7757/app`
   * （Host 的默认端口被占用时按同一顺序改用备选端口），以握手结果核对对端是 app-mcp；显式指定时只连该地址。
   */
  hostUrl?: string
  /**
   * 同一来源的多个标签页共用一条到 Host 的连接（SharedWorker；没有时选一个主标签页持有），默认 true。
   * 每个标签页仍是独立实例（各自 instanceId 与工具，随标签页存亡）。Host 不支持多路复用、或共享连接
   * 在本页不可用时自动改为每个标签页直接连接。为 false 时始终直接连接。
   */
  sharedConnection?: boolean
  /** WASM 文件地址，默认使用包内置的文件。 */
  wasmUrl?: string | URL
  /** 调用时高亮关联元素并显示提示条（M2 实现，M1 忽略）。 */
  highlight?: boolean
  /** 同时执行的调用上限，默认 1。 */
  maxConcurrentCalls?: number
  /**
   * App 总览：握手时发给 Host，模型在会话中首次接触本 App 时由 Host 附带（spec/protocol.md 第 7 节）。
   * 静态总览、不含动态状态（当前页面、登录状态等请用资源提供）。
   */
  overview?: AppOverview
  /** 日志输出，默认 console（仅 warn / error）。 */
  logger?: Logger
  /** 生命周期策略（休眠 / 唤醒），缺省 `persistent`。见 {@link LifecycleOptions}。 */
  lifecycle?: LifecycleOptions
}

export interface AppOverview {
  /** 一句话简介，≤ 100 字符。 */
  summary: string
  /** 总览正文（Markdown），≤ 2000 字符。建议小节：适用场景、能力范围、典型流程、前置条件、不支持的操作、风险说明。 */
  body?: string
  /** 如 'zh-CN'。 */
  locale?: string
}

export interface Logger {
  debug(message: string, ...args: unknown[]): void
  warn(message: string, ...args: unknown[]): void
  error(message: string, ...args: unknown[]): void
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

export interface ToolContext {
  /** Host 生成的调用 ID。 */
  callId: string
  /** 调用被取消、超时或连接断开时触发。 */
  signal: AbortSignal
  /**
   * 延长持有：handler 返回后仍阻止自动休眠（如 handler 发起了后台长任务），直到返回的句柄被释放。
   * 调用进行中本身已视为非空闲，无需手动持有。未启用生命周期时无副作用。
   */
  hold?(): HoldHandle
}

/** handler 可以直接返回数据，也可以返回带 stateHints 的结果。 */
export type ToolResult<O> = O | { data: O; stateHints?: string[] }

/** 工具 handler。 */
export type ToolHandler<I = unknown, O = unknown> = (
  input: I,
  context: ToolContext,
) => ToolResult<O> | Promise<ToolResult<O>>

/**
 * 惰性 handler 加载器（spec/lifecycle.md 第 10.2 节第 3 条）：首次调用时执行一次并缓存结果；
 * 可以返回 handler 本身，或 `{ default: handler }`（即 `() => import('./checkout')`）。
 * 加载失败时本次调用返回 `HANDLER_ERROR`，下次调用重试。
 */
export type ToolHandlerLoader<I = unknown, O = unknown> = () => Promise<
  { default: ToolHandler<I, O> } | ToolHandler<I, O>
>

export interface ToolDefinition<I = unknown, O = unknown> {
  description: string
  title?: string
  /** 缺省为无参数（`{ type: 'object', properties: {} }`）。 */
  input?: InputDefinition<I>
  /** 缺省 'write'。 */
  risk?: Risk
  activation?: Activation
  /** 缺省 true。为 false 时工具不对 Host 可见。 */
  enabled?: boolean
  /** 可选：关联的 DOM 元素，用于高亮（M2）。 */
  anchor?: Element | (() => Element | null)
  handler: (input: I, context: ToolContext) => ToolResult<O> | Promise<ToolResult<O>>
  /** 与 `handler` 二选一：只声明元数据、首次调用时加载 handler，见 {@link LazyToolDefinition}。 */
  load?: undefined
}

/**
 * 惰性工具定义：元数据立即注册，handler 在首次调用时由 `load` 加载（冷启动只初始化被调用的模块）。
 *
 * ```ts
 * appMcp.tool('cart.checkout', { description: '结算', risk: 'payment', load: () => import('./checkout') })
 * ```
 *
 * 与 {@link ToolDefinition} 的 `handler` 二选一；两者都给出或都缺少时注册抛错。
 * 注册后仍可用 `setHandler` 直接替换 handler（之后不再调用 `load`）。
 */
export interface LazyToolDefinition<I = unknown, O = unknown> extends Omit<ToolDefinition<I, O>, 'handler' | 'load'> {
  load: ToolHandlerLoader<I, O>
  handler?: undefined
}

/** `Registrar.tool` 接受的定义：普通或惰性。 */
export type AnyToolDefinition<I = unknown, O = unknown> = ToolDefinition<I, O> | LazyToolDefinition<I, O>

export interface ToolHandle {
  readonly name: string
  /** 更新描述、schema、风险或启用状态；未提供的字段保持不变。 */
  update(changes: Partial<Omit<ToolDefinition<any, any>, 'handler'>>): void
  /** 替换 handler（不产生协议消息，供框架适配在每次渲染时刷新闭包）。 */
  setHandler(handler: ToolDefinition<any, any>['handler']): void
  dispose(): void
}

/** handler 可以抛出此错误以指定错误类别；其他异常归为 HANDLER_ERROR。 */
export class ToolCallError extends Error {
  readonly kind: ErrorKind
  readonly details?: Record<string, unknown>
  constructor(kind: ErrorKind, message: string, details?: Record<string, unknown>) {
    super(message)
    this.name = 'ToolCallError'
    this.kind = kind
    this.details = details
  }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

export interface ResourceDefinition<T = unknown> {
  description: string
  /** 缺省 'application/json'。 */
  mimeType?: string
  read: () => T | Promise<T>
}

export interface ResourceHandle {
  readonly name: string
  /** 内容已变化；只有 Host 订阅时才会发送通知，且会节流。 */
  notifyChanged(): void
  /** 替换 read 函数（不产生协议消息）。 */
  setReader(read: ResourceDefinition<any>['read']): void
  dispose(): void
}

// ---------------------------------------------------------------------------
// Scope 与实例
// ---------------------------------------------------------------------------

export interface Registrar {
  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle
  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle
  /** 创建子 scope。 */
  scope(name: string): Scope
}

export interface Scope extends Registrar {
  readonly name: string
  /** 注销该 scope 下的全部工具、资源与子 scope。 */
  dispose(): void
}

export interface AppMcp extends Registrar {
  readonly options: Readonly<AppMcpOptions>
  readonly instanceId: string
  readonly state: ConnectionState
  /** 订阅连接状态变化，返回取消订阅函数。 */
  onStateChange(listener: (state: ConnectionState) => void): () => void
  /** 断开连接并注销全部工具与资源。 */
  dispose(): void

  // ---- 生命周期（spec/lifecycle.md 第 8 节）-------------------------------

  /** App 主动回连（如用户打开了相关界面）；休眠（`dormant`）或退避（`backoff`）时立即连接。 */
  wake(): void
  /** 主动请求休眠（原因 `app`，不看空闲条件与持有）。 */
  sleep(): void
  /** 临时阻止自动休眠，直到返回的句柄被释放。 */
  hold(): HoldHandle
  /** `on-demand` 模式下主动连接（其他模式等同于 `wake()`）。 */
  connectNow(): void
}
