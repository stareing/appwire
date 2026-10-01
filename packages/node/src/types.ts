/**
 * @app-mcp/node 公开 API 类型。
 *
 * 与 `@app-mcp/web`（packages/web/src/types.ts）保持同形：`tool` / `resource` / `scope` /
 * `onStateChange` / `dispose` 与 `ToolCallError` 的签名一致，同一套注册代码可在网页与 Node 中使用。
 * 本包不依赖 @app-mcp/web，因此这里按结构重复声明；差异只在创建选项（见 {@link NodeAppMcpOptions}）
 * 与 Node 独有的扩展（`setVisibility`、`token`、`start`，以及生命周期 `handleWake` / `wake` / `sleep` / `hold` 等）。
 */

import type { NativeBinding } from './native.js'

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
  /** Host 侧限流，调用未转发（Host 产生，App 一般不抛）。 */
  | 'RATE_LIMITED'
  /** 调用参数 / 结果 / 资源内容超过 Host 的大小上限（Host 产生，App 一般不抛）。 */
  | 'PAYLOAD_TOO_LARGE'

/** JSON Schema 对象（只要求顶层 type 为 object）。 */
export type JsonSchema = { type: 'object'; [key: string]: unknown }

/**
 * 标准 MCP 工具注解（spec/protocol.md 第 3 节），原样转发给 Agent，由 Agent 决定是否确认 / 放行。
 * 与旧写法 `risk` 同时给出时，声明的字段逐个优先，缺少的按 `risk` 推导。
 */
export interface ToolAnnotations {
  /** 给人看的工具标题。 */
  title?: string
  /** 不修改任何状态。 */
  readOnlyHint?: boolean
  /** 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。 */
  destructiveHint?: boolean
  /** 以相同参数重复调用没有额外效果（只在非只读时有意义）。 */
  idempotentHint?: boolean
  /** 会与外部世界交互（网络、第三方、其他用户可见）。 */
  openWorldHint?: boolean
}

/** 内容的接收方（MCP `Role`）。 */
export type Audience = 'user' | 'assistant'

/** 标准 MCP 内容注解：对结果内容的标注，Hub 原样转发。 */
export interface ContentAnnotations {
  /** 内容面向谁。 */
  audience?: Audience[]
  /** 重要程度，0（可选）到 1（必需）。 */
  priority?: number
  /** 最后修改时刻（ISO 8601）。 */
  lastModified?: string
}

/**
 * 调用结果的业务状态（spec/protocol.md 3.2）：`done`（缺省）已完成；`pending` 已受理、待用户在 App 内确认或异步完成
 * （后续状态见 `stateResource`）；`partial` 只完成了一部分（说明见 `summary`）；`noop` 没有做任何改动。
 */
export type ResultStatus = 'done' | 'pending' | 'partial' | 'noop'

/** 任意 JSON Schema 对象（输出 schema 的根类型不限于 object；非 object 时 Hub 包装为 `{ result: <schema> }`）。 */
export type OutputSchema = { [key: string]: unknown }

/**
 * 工具结果的 schema（MCP `outputSchema`），形式同 {@link InputDefinition}：JSON Schema 对象、
 * zod v4 schema（按输出形态转换，不做校验）或带 `toJSONSchema()` 方法的对象。
 */
export type OutputDefinition<O> = OutputSchema | { toJSONSchema(): OutputSchema } | ZodLike<O>

/**
 * 工具输入定义。支持三种形式：
 * - JSON Schema 对象；
 * - zod v4 schema（可选 peer 依赖，调用前用 `parse` 校验）；
 * - 任何带 `toJSONSchema()` 方法的对象。
 */
export type InputDefinition<I> = JsonSchema | { toJSONSchema(): JsonSchema } | ZodLike<I>

/** zod v4 schema 的最小结构（避免强依赖 zod 类型）。 */
export interface ZodLike<I> {
  readonly _zod: unknown
  parse(input: unknown): I
}

export type ConnectionState =
  | { status: 'disabled' }
  | { status: 'idle' }
  | { status: 'connecting' }
  | { status: 'handshaking' }
  | { status: 'pending-pairing' }
  | { status: 'connected' }
  /**
   * `reason` / `code`：进入退避的原因与错误码（spec/protocol.md 10.1）：连接建立失败（`HOST_NOT_RUNNING`、
   * `CONNECT_TIMEOUT` 等）、已建立的连接断开（`CONNECTION_CLOSED` / `CONNECTION_LOST`）、心跳 / 握手超时
   * （`HEARTBEAT_TIMEOUT` / `HANDSHAKE_TIMEOUT`）。原生驱动层的每次退避都带这两项；类型保留可选以兼容旧原生模块。
   */
  | { status: 'backoff'; retryAt: number; reason?: string; code?: string }
  /** `code`：错误码（spec/protocol.md 10.1，如 `ORIGIN_NOT_ALLOWED`、`PAIRING_REJECTED`）。 */
  | { status: 'rejected'; reason: string; code: string }
  | { status: 'stopped' }
  /** 已与 Host 完成 `app/sleep` 握手后断开（或 `on-demand` 启动后尚未连接）；注册表保留，等待唤醒。 */
  | { status: 'dormant' }
  /** 收到唤醒后正在回连，之后进入 `handshaking`。 */
  | { status: 'waking' }
  /**
   * 对端不是期望的 Host（spec/protocol.md 1.6）：不是 app-mcp，或属于本机其他用户。不再自动重连，
   * `wake()` / `connectNow()` 时再试一次。`reason` 为中文说明。
   */
  | { status: 'host-mismatch'; reason: string; code: string }

// ---------------------------------------------------------------------------
// 生命周期（spec/lifecycle.md）
// ---------------------------------------------------------------------------

/** `persistent`：不休眠（默认）；`idle`：空闲后休眠；`on-demand`：启动不连接，被唤醒时连接。 */
export type LifecycleMode = 'persistent' | 'idle' | 'on-demand'
/** 休眠后的进程驻留策略。 */
export type Residency = 'keep' | 'exit-when-idle' | 'exit-always'
export type WakeKind = 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'
/** 回连原因（随 `app/hello` 发送）。 */
export type WakeReason = 'os-activation' | 'app' | 'visible' | 'cold-start'
export type SleepReason = 'idle' | 'grace' | 'background' | 'app'

/** 本实例的唤醒描述（spec/lifecycle.md 第 5 节），随 `app/sleep` 上报给 Host。 */
export interface WakeDescriptor {
  kind: WakeKind
  /** 各 kind 的定位信息（如 `myapp://app-mcp/wake`）。 */
  target?: string
  /** 能否不把窗口带到前台就唤醒，缺省 false。 */
  background?: boolean
}

/** 心跳策略（spec/lifecycle.md 第 11 节）。 */
export type HeartbeatMode = 'auto' | 'always' | 'off'

export interface LifecycleOptions {
  /** 缺省 `'persistent'`。 */
  mode?: LifecycleMode
  /** `idle` 模式下空闲多久进入休眠，默认 60000。 */
  idleTimeoutMs?: number
  /** 可见性为 hidden / frozen 时使用的空闲时间（与模式超时取较小值），默认 15000。 */
  hiddenIdleTimeoutMs?: number
  /** `on-demand` 模式下连上后一直没有调用时保留连接的时间，默认 10000（处理过调用后用 `mergeWindowMs`）。 */
  graceMs?: number
  /**
   * 进程驻留，缺省 `'keep'`。`'exit-when-idle'`：仅当本进程由唤醒冷启动时，休眠后触发 `onIdleExit`；
   * `'exit-always'`：每次休眠后都触发。SDK 本身从不退出进程。
   */
  residency?: Residency
  /** 本实例的唤醒描述；缺省时 Host 回退到清单 `launch`。 */
  wake?: WakeDescriptor
  /**
   * `idle` / `on-demand` 下连续多少次以"Host 不在"（`HOST_NOT_RUNNING`）连接失败后停止重连、进入 `dormant`，
   * 等可见、`wake()` 或 Host 唤醒再连接（spec/lifecycle.md 第 11 节）。默认 3；0 = 一直重连。
   */
  hostAbsentRetries?: number
  /**
   * 回退到 4e 之前的定时器行为（租约到期后才计空闲、Host 不在时一直重连、双向心跳、调用后按空闲时长、
   * 任何资源订阅都阻止休眠）。默认 false。
   */
  legacyTimers?: boolean
  /**
   * 调用 / 资源读取后的合并窗口（spec/lifecycle.md 第 13 节 B1）：本连接处理过调用后，空闲时长取
   * min(本值, 按模式与可见性的空闲时长)，之后是否在线只由 Host 租约决定。默认 2000。
   */
  mergeWindowMs?: number
  /** `idle` / `on-demand` 下从可见变为隐藏 / 冻结且空闲时立即休眠，不等租约（B4）。默认 false。 */
  sleepOnBackground?: boolean
}

/** 阻止自动休眠的持有（{@link AppMcp.hold}、{@link ToolContext.hold}）。 */
export interface HoldHandle {
  /** 释放持有。重复调用无效果。 */
  release(): void
}

export interface AppOverview {
  /** 一句话简介，≤ 100 字符。 */
  summary: string
  /** 总览正文（Markdown），≤ 2000 字符。 */
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
// 创建
// ---------------------------------------------------------------------------

/** 与 @app-mcp/web 的 `AppMcpOptions` 同名字段含义相同，另加 Node 专用字段。 */
export interface NodeAppMcpOptions {
  /** `[a-z][a-z0-9-]{0,62}` */
  appId: string
  appName: string
  appVersion?: string
  /** 为 false 时不加载原生模块、不连接，所有注册调用为空操作。默认 true。 */
  enabled?: boolean
  /**
   * Host 端点：`unix:<绝对路径>`、`pipe:\\.\pipe\<名称>`、`ws://…` 或 `wss://…`（spec/protocol.md 第 1 节）。
   * 缺省：环境变量 `APP_MCP_ENDPOINT` → 登记文件 `~/.app-mcp/run/endpoints.json`（运行中的 Host 写下的实际端点）→
   * 平台默认本地 IPC 端点（Unix 域套接字 / Windows 命名管道）→ `ws://127.0.0.1:7717/app`。
   */
  hostUrl?: string
  /** 同时执行的调用上限，默认 1。 */
  maxConcurrentCalls?: number
  /**
   * App 总览：握手时发给 Host，模型在会话中首次接触本 App 时由 Host 附带（spec/protocol.md 第 7 节）。
   * 静态总览、不含动态状态（当前页面、登录状态等请用资源提供）。
   */
  overview?: AppOverview
  /** 日志输出，默认 console（仅 warn / error）。 */
  logger?: Logger

  // ---- Node 专用 ----------------------------------------------------------

  /** 客户端类型：普通 Node 程序用 `'native'`（默认），Electron 主进程用 `'hybrid'`。 */
  clientKind?: 'native' | 'hybrid'
  /** 实例 ID，缺省由原生运行时生成（每个进程一个）。 */
  instanceId?: string
  /** 实例标题（如窗口标题），显示在 Host 的实例列表中。 */
  instanceTitle?: string
  /** 之前配对得到的 token（由 {@link onPaired} 持久化）。 */
  token?: string
  /** Host 唤醒 App 时提供的一次性 token；缺省读取环境变量 `APP_MCP_LAUNCH_TOKEN`。 */
  launchToken?: string
  /** 配对成功并获得新 token 时调用；App 应持久化，下次通过 {@link token} 传入。 */
  onPaired?: (token: string) => void
  /** 创建后立即连接 Host。默认 true；为 false 时需手动调用 `start()`。 */
  autoStart?: boolean
  /**
   * 连接期间保持 Node 进程存活（事件循环上保留一个 ref 的定时器）。默认 true。
   * 原生回调本身不会阻止进程退出；`dispose()` 后进程可正常退出。
   */
  keepAlive?: boolean
  /** 生命周期策略（spec/lifecycle.md）。缺省 `{ mode: 'persistent' }`：保持现有行为，不休眠。 */
  lifecycle?: LifecycleOptions
  /** 建立 WebSocket 连接的超时（毫秒），默认 5000。 */
  connectTimeoutMs?: number
  /**
   * 心跳策略（spec/lifecycle.md 第 11 节）。默认 `'auto'`：本地 IPC 与本机回环不发心跳（靠连接断开感知），
   * 远程端点发；`'always'` / `'off'` 强制。
   */
  heartbeat?: HeartbeatMode
  /**
   * 已休眠且 `lifecycle.residency` 允许退出进程时调用。SDK **不会**自行退出进程：
   * App 在这里自行 `process.exit()`（Electron 为 `app.quit()`），或什么都不做。
   * 也可以用 {@link AppMcp.onIdleExit} 订阅。
   */
  onIdleExit?: () => void
  /** 高级：注入原生模块（测试或自定义加载路径）。缺省按平台加载包内的 `.node` 文件。 */
  binding?: NativeBinding
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
   * 必须在调用结束前调用；调用已结束时抛出（`code` 为 `ALREADY_COMPLETED`）。未启用生命周期时无副作用。
   */
  hold(): HoldHandle
  /**
   * 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。`progress` 应递增
   * （不递增的值被 Host 丢弃），`total` 未知时省略。调用已结束、未连接或原生模块不支持时无副作用（不抛出）。
   */
  progress(progress: number, total?: number, message?: string): void
}

/**
 * 结构化调用结果。handler 返回的对象**含 `data` 键**、其余键都属于本接口且取值合法时按此解释，
 * 否则整个返回值作为 `data`（避免把普通返回值误判为信封）。
 */
export interface ToolResultEnvelope<O> {
  /** 返回值；`undefined` / `null` 表示无返回值（Hub 对模型输出"已完成"）。 */
  data: O
  /** 调用后内容可能已变化的资源名，提示模型重新读取。 */
  stateHints?: string[]
  /** 业务状态，缺省 `done`。 */
  status?: ResultStatus
  /** `pending` 时可读取后续状态的资源名（局部名）。 */
  stateResource?: string
  /** 一句面向模型 / 用户的结论（`partial` 时说明完成了哪部分）。 */
  summary?: string
  /** 结果内容的标注。 */
  annotations?: ContentAnnotations
}

/**
 * handler 可以直接返回数据，也可以返回结构化结果（{@link ToolResultEnvelope}）。
 *
 * @compat 保留旧成员 `{ data: O; stateHints?: string[] }`：否则形如 `{ data, status: 'success' }` 的普通返回值
 * 在类型上会被当作信封检查而报错（运行时它整体作为 `data`，见 {@link ToolResultEnvelope}）。
 */
export type ToolResult<O> = O | { data: O; stateHints?: string[] } | ToolResultEnvelope<O>

/** 工具 handler。 */
export type ToolHandler<I = unknown, O = unknown> = (
  input: I,
  context: ToolContext,
) => ToolResult<O> | Promise<ToolResult<O>>

/**
 * 惰性 handler 加载器（spec/lifecycle.md 第 10.2 节第 3 条）：首次调用时执行一次并缓存结果；
 * 可以返回 handler 本身，或 `{ default: handler }`（即 `() => import('./checkout.js')`）。
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
  /** 旧写法：缺省 'write'。新代码优先用 `annotations`；两者同时给出时 `annotations` 声明的字段优先。 */
  risk?: Risk
  /** 标准 MCP 工具注解（只读、破坏性、幂等、开放世界）。 */
  annotations?: ToolAnnotations
  /** 结果的 schema（MCP `outputSchema`）；缺省不声明。 */
  outputSchema?: OutputDefinition<O>
  activation?: Activation
  /** 缺省 true。为 false 时工具不对 Host 可见。 */
  enabled?: boolean
  /** 网页中用于高亮的元素；Node 中忽略（保留字段以便与 @app-mcp/web 共用定义）。 */
  anchor?: unknown
  handler: (input: I, context: ToolContext) => ToolResult<O> | Promise<ToolResult<O>>
  /** 与 `handler` 二选一：只声明元数据、首次调用时加载 handler，见 {@link LazyToolDefinition}。 */
  load?: undefined
}

/**
 * 惰性工具定义：元数据立即注册，handler 在首次调用时由 `load` 加载（冷启动只初始化被调用的模块）。
 *
 * ```ts
 * appMcp.tool('cart.checkout', { description: '结算', risk: 'payment', load: () => import('./checkout.js') })
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
  /**
   * 更新描述、schema、风险、注解或启用状态；未提供的字段保持不变，显式给出 `undefined` 的字段恢复默认
   * （`annotations` / `outputSchema` 为清除声明；旧版原生模块不支持清除，保持原声明）。
   */
  update(changes: Partial<Omit<ToolDefinition<any, any>, 'handler'>>): void
  /** 替换 handler（不产生协议消息）。 */
  setHandler(handler: ToolDefinition<any, any>['handler']): void
  dispose(): void
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

export interface ResourceDefinition<T = unknown> {
  description: string
  /** 缺省 'application/json'。 */
  mimeType?: string
  /**
   * 需实时推送（spec/lifecycle.md 第 13 节 B3）：被 Host 订阅时保持连接、休眠中变化时回连推送。
   * 默认 false：订阅不阻止休眠，变化在下次连接时补发。
   */
  realtime?: boolean
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
  readonly options: Readonly<NodeAppMcpOptions>
  readonly instanceId: string
  readonly state: ConnectionState
  /** Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 `cid` 对应；未连接时为 `undefined`。 */
  readonly connectionId?: string
  /** 订阅连接状态变化，返回取消订阅函数。 */
  onStateChange(listener: (state: ConnectionState) => void): () => void
  /** 断开连接并注销全部工具与资源。 */
  dispose(): void

  // ---- Node 专用 ----------------------------------------------------------

  /** 当前 token（配置带入的或配对后获得的）；未启用时为 null。 */
  readonly token: string | null
  /** 开始连接 Host（`autoStart: false` 时使用；重复调用无效果）。 */
  start(): void
  /** 报告实例可见性（如 Electron 窗口最小化 / 失焦）。`focused` 默认 true。 */
  setVisibility(visibility: Visibility, focused?: boolean): void

  // ---- 生命周期（spec/lifecycle.md 第 8 节）-------------------------------

  /**
   * 处理操作系统激活参数：命令行参数数组（如 `process.argv`、Electron `second-instance` 的 argv）
   * 或单个字符串 / URL（macOS `open-url`）。识别 `app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、
   * `#app-mcp-wake=<token>`；识别到时回连（尚未 `start()` 时记录，`start()` 时连接），返回 true。
   * 不是本 SDK 的唤醒返回 false。
   */
  handleWake(args: string | readonly string[]): boolean
  /** App 主动回连（如用户打开了相关界面）。`reason` 缺省 `'app'`。返回是否因此发起了回连。 */
  wake(reason?: 'app' | 'visible'): boolean
  /** `on-demand` 模式下主动连接；尚未 `start()` 时等同于 `start()`。 */
  connectNow(): boolean
  /** 主动请求休眠（原因 `app`，不看空闲条件与持有）。返回是否有效果。 */
  sleep(): boolean
  /** 临时阻止自动休眠，直到返回的句柄被释放。 */
  hold(): HoldHandle
  /** 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）；未启用时为空字符串。 */
  toolsHash(): string
  /** 订阅 idle-exit（见 {@link NodeAppMcpOptions.onIdleExit}），返回取消订阅函数。 */
  onIdleExit(listener: () => void): () => void
}
