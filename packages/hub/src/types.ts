/**
 * @app-mcp/hub 的公开类型：与 `crates/hub` 的 serde（camelCase）JSON 形态一一对应（spec/hub-api.md 3.1、3.4）。
 */

// ---------------------------------------------------------------------------
// 基础枚举
// ---------------------------------------------------------------------------

/** 风险等级，顺序：read < write < destructive < payment < os-sensitive。 */
export type Risk = 'read' | 'write' | 'destructive' | 'payment' | 'os-sensitive'
export type Activation = 'headless' | 'background' | 'foreground'
export type Visibility = 'visible' | 'hidden' | 'frozen'

/** 协议错误类别（spec/protocol.md §4）。 */
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

/**
 * 绑定层自身的错误代码（非协议错误）。`UNSUPPORTED`：原生模块未包含所需能力（cargo feature，spec/hub-api.md 3.10，
 * 如精简构建上的 `mcpHttp` / `upstreams` / `serveHttp`），说明含缺少的 feature 名；换完整构建或关闭该配置，重试无效。
 */
export type BindingErrorCode = 'INVALID_ARG' | 'SHUTDOWN' | 'START_FAILED' | 'UNSUPPORTED' | 'INTERNAL'

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/** 上游 MCP 服务器（Hub 以子进程启动并聚合其工具）。 */
export interface UpstreamConfig {
  command: string
  args?: string[]
  env?: Record<string, string>
}

export interface ApprovalPolicy {
  /** 风险不低于此等级的调用前询问审批回调；缺省 = 不审批。 */
  requireAtOrAbove?: Risk | null
  /** 等待审批的上限（毫秒），缺省为 `responseTimeoutMs`。超时视为拒绝。 */
  timeout?: number | null
}

/**
 * 唤醒器（spec/hub-api.md 3.5）：`system`（默认，按平台执行系统激活）/ `none`（不唤醒，返回 APP_DISCONNECTED）/
 * `{ exec: [程序, ...参数] }`（不经 shell 执行，唤醒请求以一行 JSON 写入 stdin）。`setWaker` 设置的回调优先。
 */
export type WakerConfig = 'system' | 'none' | { exec: string[] }

/**
 * 工具暴露方式（spec/hub-api.md 3.7）：`auto`（默认，App 与上游工具总数超过阈值时渐进）/
 * `progressive`（工具列表只含 `apps.*` 与本会话展开过、调用过或选定了实例的 App）/ `all`。
 */
export type ToolExposure = 'auto' | 'progressive' | 'all'

/** `Hub.start` 的配置。时长均为毫秒。 */
export interface HubConfig {
  /**
   * HTTP 监听地址：`/app`（App 的 WebSocket 连接）、`/healthz`，`mcpHttp` 时另有 `/mcp`（spec/protocol.md 1.3）。
   * 缺省 `127.0.0.1:7717`（被占用时依次尝试 7737、7757）；显式给出时只绑定该地址；端口 0 = 随机；`null` = 不开。
   */
  listen?: string | null
  /** 是否在 `listen` 上提供 MCP Streamable HTTP（`/mcp`），默认 false。 */
  mcpHttp?: boolean
  /** 单实例锁与登记文件目录（`<runDir>/hub.lock`、`endpoints.json`，spec/protocol.md 1.5、1.7）；缺省不参与。 */
  runDir?: string
  /**
   * 本地 IPC 端点（原生 App 默认连接这里，spec/protocol.md 1.2）：`unix:<绝对路径>` 或 `pipe:\\.\pipe\<名称>`（JS 字符串中反斜杠需转义）；
   * 缺省为平台默认端点（Linux `$XDG_RUNTIME_DIR/app-mcp/hub.sock` 等）；`null` = 不开。
   */
  ipcEndpoint?: string | null
  /** 静态清单文件（app-mcp.json）。 */
  manifestFiles?: string[]
  /** 静态清单目录（按文件名排序加载其中的 *.json）。 */
  manifestDir?: string
  /** 额外允许的 Origin 模式（默认已允许 localhost / 127.0.0.1 任意端口）。 */
  allowOrigins?: string[]
  pingIntervalMs?: number
  idleTimeoutMs?: number
  hiddenIdleTimeoutMs?: number
  /** 发给 SDK 的 `timeoutMs`（SDK 侧超时），缺省 30000。 */
  invokeTimeoutMs?: number
  /** Hub 等待 SDK 响应的时间，缺省 35000。 */
  responseTimeoutMs?: number
  /** 列表变化通知的合并窗口，缺省 50。 */
  listChangedDebounceMs?: number
  /** 等待配对回调的上限，缺省 120000。 */
  pairingTimeoutMs?: number
  // ---- 生命周期（spec/hub-api.md 3.5）----
  /** 调用实例完成后发送的租约时长，缺省 60000；0 关闭租约。 */
  leaseTtlMs?: number
  /** 唤醒后等待 App 回连的上限，缺省 15000（超时 → APP_NOT_RESPONDING）。 */
  wakeTimeoutMs?: number
  /** 唤醒令牌有效期，缺省 60000。 */
  wakeTokenTtlMs?: number
  /** 休眠记录保留时长，缺省 86400000（24 小时）。 */
  dormantTtlMs?: number
  /** 同一 appId 以新实例 ID 连接时移除其休眠记录，缺省 true。 */
  dormantReplacedByNewInstance?: boolean
  /** App 未运行且清单无显式 wake 时，是否由清单 launch 推导唤醒方式并冷启动，缺省 false。 */
  wakeFromLaunch?: boolean
  /** 每 App 每分钟最多唤醒次数（spec/lifecycle.md 第 12 节），缺省 6；0 不限。超出时调用以 `LAUNCH_FAILED`（`data.code = 'WAKE_RATE_LIMITED'`）结束。 */
  wakeRateLimit?: number
  /** 回退到旧心跳：对所有 App 连接发 ping 并按无消息断开（spec/lifecycle.md 第 11 节），缺省 false。 */
  legacyHeartbeat?: boolean
  /** 唤醒器，缺省 `system`。 */
  waker?: WakerConfig
  // ---- 渐进暴露（spec/hub-api.md 3.7）----
  /** 工具暴露方式，缺省 `auto`。 */
  toolExposure?: ToolExposure
  /** `auto` 的阈值，缺省 40。 */
  toolExposureThreshold?: number
  /** 上游 MCP 服务器：名称（appId 规则）→ 启动方式。 */
  upstreams?: Record<string, UpstreamConfig>
  approval?: ApprovalPolicy
}

// ---------------------------------------------------------------------------
// App 与实例
// ---------------------------------------------------------------------------

export interface InstanceInfo {
  instanceId: string
  /** `web` / `native` / `hybrid`。 */
  clientKind: string
  visibility: Visibility
  focused: boolean
  /** 最近活跃时间（Unix 毫秒）。 */
  lastActiveMs: number
  title: string | null
  /** 实例进程号：经本地 IPC 连接时由操作系统提供；否则缺省。 */
  pid?: number
  /** Hub 分配的连接 ID（`<标记>-<序号>`，与 Hub / SDK 日志的 `cid` 相同）；休眠实例缺省。 */
  connectionId?: string
}

export interface AppInfo {
  appId: string
  name: string
  kind: 'app' | 'upstream'
  summary: string | null
  connected: boolean
  instances: InstanceInfo[]
  selectedInstance: string | null
  /** 休眠中的实例（按休眠时间排列）：调用其工具时 Hub 先唤醒。`connected` 只看已连接实例。 */
  dormantInstances: InstanceInfo[]
}

// ---------------------------------------------------------------------------
// 工具与资源
// ---------------------------------------------------------------------------

/**
 * - `available`：至少一个已连接实例注册了该工具
 * - `disconnected`：App 未连接，工具来自静态清单
 * - `notRegistered`：App 已连接，但没有实例注册该静态工具
 * - `dormant`：只由休眠实例提供；调用时 Hub 先唤醒再派发
 *
 * 未来可能新增取值，调用方应把不认识的值当作“不可直接调用”。
 */
export type Availability = 'available' | 'disconnected' | 'notRegistered' | 'dormant'

/** JSON Schema（原样透传）。 */
export type JsonSchema = Record<string, unknown>

export interface HubTool {
  /** 全名 `<appId>.<tool>`。 */
  name: string
  appId: string
  tool: string
  title: string | null
  description: string
  inputSchema: JsonSchema
  risk: Risk
  activation: Activation
  availability: Availability
}

export interface ToolFilter {
  /** 只要这些 App 的工具；缺省 = 全部。 */
  apps?: string[] | null
  /** 只要风险不高于此等级的工具。 */
  maxRisk?: Risk | null
  /** 只列出当前可调用（available）的工具。默认 false。 */
  onlyAvailable?: boolean
  /** 是否包含内置工具 `apps.list` / `apps.select` / `apps.overview`（渐进暴露生效时另有 `apps.tools`）。默认 true。 */
  includeBuiltin?: boolean
  /**
   * 厂商会话 ID（与 `dispatch` / `callTool` 的会话相同；缺省 = 默认会话）。渐进暴露生效且未给 `apps` 时，
   * 只保留该会话已展开 / 调用过 / 选定了实例的 App 的工具。
   */
  session?: string | null
}

export interface HubResource {
  /** `app-mcp://<appId>/<name>`。 */
  uri: string
  /** `<appId>.<name>`。 */
  name: string
  appId: string
  description: string
  mimeType: string | null
  available: boolean
}

export interface ResourceContent {
  uri: string
  mimeType: string | null
  /** 文本内容（JSON 资源为 JSON 文本）。 */
  text: string | null
  /** 二进制内容（base64）。 */
  blob: string | null
}

export interface AppOverviewInfo {
  appId: string
  name: string
  summary: string
  body: string | null
  locale: string | null
  version: string
  /** `runtime` / `manifest` / `upstream`。 */
  source: string
  /** 注入给模型的文本（含 `<app-overview>` 包裹）。 */
  text: string
}

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

export interface CallRequest {
  /** 全名 `<appId>.<tool>`（内置工具为 `apps.list` 等）。 */
  name: string
  /** 参数对象；缺省为 `{}`。 */
  arguments?: unknown
  /** 严格指定实例（不存在或未注册该工具 → TOOL_NOT_FOUND）。 */
  instanceId?: string | null
  /** 本次调用的等待上限（毫秒）。 */
  timeout?: number | null
  /** 供 `cancelCall` 使用；缺省自动生成。 */
  callId?: string | null
  /** 厂商会话 ID：总览首次附带、`apps.select` 按会话计算。 */
  session?: string | null
}

export interface ToolErrorInfo {
  kind: ErrorKind
  message: string
  details?: unknown
}

/** 调用结果：`{ ok }` 或 `{ error }`。 */
export type CallResult = { ok: unknown; error?: undefined } | { error: ToolErrorInfo; ok?: undefined }

export interface CallOutcome {
  callId: string
  result: CallResult
  /** App 声明可能已变化的资源名（不含 appId 前缀）。 */
  stateHints: string[]
  instanceId: string | null
  /** 该会话首次接触此 App 时附带：把 `overview.text` 放进模型上下文。 */
  overview: AppOverviewInfo | null
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

export type HubEvent =
  | { type: 'appConnected'; appId: string; instanceId: string }
  | { type: 'appDisconnected'; appId: string; instanceId: string }
  /** 已合并（listChangedDebounceMs）；收到后重新 exportTools。 */
  | { type: 'toolsChanged' }
  | { type: 'resourcesChanged' }
  | { type: 'resourceUpdated'; uri: string }
  | { type: 'visibilityChanged'; appId: string; instanceId: string; visibility: Visibility }
  | { type: 'upstreamState'; name: string; connected: boolean; error: string | null }
  /** 实例进入休眠：工具仍列出（availability = dormant），不另发 toolsChanged。 */
  | { type: 'appDormant'; appId: string; instanceId: string }
  /** Hub 正在唤醒 App；`instanceId` 为 null 表示 App 未运行、按清单冷启动。 */
  | { type: 'appWaking'; appId: string; instanceId: string | null }
  /** SDK 上报了此前遇到的连接问题（`app/diagnostic`，spec/protocol.md 10.2）；`code` 可能是本 Hub 不认识的新码。 */
  | { type: 'appDiagnostic'; appId: string; instanceId: string; code: string; message: string; count: number }

// ---------------------------------------------------------------------------
// 运行状态（spec/hub-api.md 3.9；与 `GET /status` 相同）
// ---------------------------------------------------------------------------

/** 主 HTTP 服务的令牌策略。 */
export interface AuthStatus {
  /** 是否配置了访问令牌。 */
  tokenConfigured: boolean
  /** 不带 `Origin` 的本地客户端是否也必须携带令牌（`--auth all`）。 */
  tokenRequiredWithoutOrigin: boolean
}

/** App 整体状态。 */
export type AppState = 'connected' | 'waking' | 'dormant' | 'disconnected'

/** 实例状态。 */
export type InstanceState = 'connected' | 'dormant' | 'waking'

export interface InstanceStatus extends InstanceInfo {
  state: InstanceState
  /** 功耗观测（spec/lifecycle.md 第 12 节）；Hub 尚无该实例的计数时缺省。 */
  power?: InstancePower
}

/** 已连接实例当前不能休眠的原因（Hub 可见部分；App 的 hold() 只有 SDK 知道）。 */
export type AwakeReason = 'persistent' | 'call' | 'lease' | 'subscription' | 'wake-pending'

/** 每实例功耗观测（跨重连与休眠保留，Hub 重启清零）。 */
export interface InstancePower {
  /** 回连次数：Hub 启动以来完成握手的次数减 1。 */
  reconnects: number
  /** 以该休眠实例为目标实际发出的唤醒激活次数。 */
  wakes: number
  /** 累计在线秒数（含当前连接）。 */
  onlineSecs: number
  /** Hub 发出的 ping 与收到 SDK 的 ping 之和。 */
  heartbeats: number
  /** SDK 声明的心跳间隔：0 = 不发心跳（本地传输）；缺省 = 旧 SDK。 */
  heartbeatMs?: number
  /** SDK 声明的生命周期模式；缺省 = 未声明。 */
  lifecycleMode?: 'persistent' | 'idle' | 'on-demand'
  /** 当前不能休眠的原因；休眠实例与可以休眠时缺省。 */
  awakeReasons?: AwakeReason[]
}

/** 最近一次错误（握手被拒、配对被拒、唤醒失败 / 超时；上游为进程错误）。 */
export interface LastError {
  /** 连接级错误码（spec/protocol.md 10.1）或工具错误类别；未知时缺省。 */
  code?: string
  message: string
  /** 发生时刻（Unix 毫秒）；上游错误为 0。 */
  atMs: number
}

export interface AppStatus {
  appId: string
  name: string
  kind: 'app' | 'upstream'
  state: AppState
  /** 在线实例在前，其后为休眠实例；上游为空。 */
  instances: InstanceStatus[]
  lastError?: LastError
  /** Hub 启动以来为该 App 实际发出的唤醒激活次数（含冷启动；上游为 0）。旧 Hub 缺省。 */
  wakes?: number
}

/** 一条 SDK 诊断上报（`app/diagnostic`）。 */
export interface DiagnosticReport {
  appId: string
  instanceId: string
  connectionId: string
  code: string
  message: string
  count: number
  /** Hub 收到的时刻（Unix 毫秒）。 */
  receivedAtMs: number
}

export interface HubStatus {
  /** 固定为 `app-mcp`。 */
  service: string
  version: string
  /** 进程的操作系统用户；取不到时缺省。 */
  user?: string
  pid: number
  /** HTTP 服务实际监听地址；未开启时缺省。 */
  listen?: string
  /** 本地 IPC 端点；未开启时缺省。 */
  ipcEndpoint?: string
  /** 启动时刻（Unix 毫秒）。 */
  startedAtMs: number
  /** 是否提供 MCP Streamable HTTP（`/mcp`）。 */
  mcpHttp: boolean
  auth: AuthStatus
  /** 已初始化的 MCP 会话数。 */
  mcpSessions: number
  /** App（含上游），按 appId 排序。 */
  apps: AppStatus[]
  /** 最近的 SDK 诊断上报，旧的在前（最多 32 条）。 */
  reports: DiagnosticReport[]
}

// ---------------------------------------------------------------------------
// 策略回调
// ---------------------------------------------------------------------------

export interface ApprovalRequest {
  callId: string
  appId: string
  appName: string
  tool: string
  title: string | null
  description: string
  risk: Risk
  arguments: unknown
  session: string | null
}

export interface PairingRequest {
  appId: string
  appName: string
  origin: string | null
  clientKind: string
  instanceId: string
}

/** 唤醒方式（spec/lifecycle.md 第 5 节）。 */
export type WakeKind = 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'

export interface WakeDescriptor {
  kind: WakeKind
  /** scheme、AUMID、bundle id、D-Bus 名称、组件名或 URL。 */
  target?: string | null
  /** 能否不把窗口带到前台就唤醒。 */
  background: boolean
}

/** 交给 {@link Waker} 的唤醒请求（spec/hub-api.md 3.5）。 */
export interface WakeRequest {
  appId: string
  /** 被唤醒的休眠实例；null = App 未运行，按清单冷启动。 */
  instanceId: string | null
  descriptor: WakeDescriptor
  /** 一次性唤醒令牌（32 位十六进制）。 */
  token: string
  /** 通用激活参数 `app-mcp-wake:<token>`，App 端 SDK 的 `handleWake` 可识别。 */
  activationArg: string
}

/**
 * 自定义唤醒（如 Android 发送显式广播）：resolve = 已发出激活，Hub 随后等待 App 回连（`wakeTimeoutMs`）。
 * 抛错 / reject → 调用以 `LAUNCH_FAILED` 结束；抛出 `kind` 为协议错误类别的 {@link HubError}（如 `APP_NOT_INSTALLED`）则用该类别。
 */
export type Waker = (req: WakeRequest) => void | Promise<void>

/** 返回 true 同意；返回 false、抛错、reject、超时均视为拒绝。 */
export type ApprovalHandler = (req: ApprovalRequest) => boolean | Promise<boolean>
/** 返回 true 同意配对；返回 false、抛错、reject、超时均视为拒绝。 */
export type PairingHandler = (req: PairingRequest) => boolean | Promise<boolean>

// ---------------------------------------------------------------------------
// 工具格式（spec/hub-api.md 第 5 节）
// ---------------------------------------------------------------------------

export type ToolFormat = 'mcp' | 'openai-chat' | 'openai-responses' | 'anthropic' | 'gemini'

export interface McpToolDef {
  name: string
  title?: string
  description: string
  inputSchema: JsonSchema
  annotations?: Record<string, unknown>
}
export interface McpToolCall {
  name: string
  arguments?: unknown
}
export interface McpCallToolResult {
  content: Array<{ type: string; text?: string; [k: string]: unknown }>
  isError?: boolean
  structuredContent?: unknown
  [k: string]: unknown
}

export interface OpenAiChatToolDef {
  type: 'function'
  function: { name: string; description: string; parameters: JsonSchema }
}
export interface OpenAiChatToolCall {
  id: string
  type?: 'function'
  function: { name: string; arguments: string }
}
export interface OpenAiChatToolMessage {
  role: 'tool'
  tool_call_id: string
  content: string
}

export interface OpenAiResponsesToolDef {
  type: 'function'
  name: string
  description: string
  parameters: JsonSchema
}
export interface OpenAiResponsesFunctionCall {
  type: 'function_call'
  call_id: string
  name: string
  arguments: string
  id?: string
}
export interface OpenAiResponsesFunctionCallOutput {
  type: 'function_call_output'
  call_id: string
  output: string
}

export interface AnthropicToolDef {
  name: string
  description: string
  input_schema: JsonSchema
}
export interface AnthropicToolUse {
  type: 'tool_use'
  id: string
  name: string
  input: unknown
}
export interface AnthropicToolResult {
  type: 'tool_result'
  tool_use_id: string
  content: string
  is_error?: boolean
}

export interface GeminiTools {
  functionDeclarations: Array<{ name: string; description: string; parameters?: JsonSchema }>
}
export interface GeminiFunctionCall {
  name: string
  args?: unknown
  id?: string
}
export interface GeminiFunctionResponsePart {
  functionResponse: { name: string; id?: string; response: { output: string } | { error: string } }
}

/** `exportTools(format)` 的返回类型。 */
export interface ExportedTools {
  mcp: McpToolDef[]
  'openai-chat': OpenAiChatToolDef[]
  'openai-responses': OpenAiResponsesToolDef[]
  anthropic: AnthropicToolDef[]
  gemini: GeminiTools
}

/** `dispatch(format, call)` 接受的工具调用。 */
export interface ToolCallInput {
  mcp: McpToolCall | { params: McpToolCall; [k: string]: unknown }
  'openai-chat': OpenAiChatToolCall
  'openai-responses': OpenAiResponsesFunctionCall
  anthropic: AnthropicToolUse
  gemini: GeminiFunctionCall | { functionCall: GeminiFunctionCall }
}

/** `dispatch(format, call)` 返回的工具结果消息。 */
export interface ToolResultMessage {
  mcp: McpCallToolResult
  'openai-chat': OpenAiChatToolMessage
  'openai-responses': OpenAiResponsesFunctionCallOutput
  anthropic: AnthropicToolResult
  gemini: GeminiFunctionResponsePart
}
