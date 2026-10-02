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
/** App 工具对界面的依赖（spec/protocol.md 3.4）：`app` 不依赖界面（未声明即此值）；`view` 只在所在界面处于最上层时注册。 */
export type ToolSurface = 'app' | 'view'

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
   * 调用频率超出 {@link LimitsConfig}，调用未转发。`details`：`retryAfterMs`（建议等待时长）、`scope`（`tool` / `app`）、
   * `perMinute`、`burst`、`appId`、`tool`。
   */
  | 'RATE_LIMITED'
  /** 参数 / 结果 / 资源内容超过大小上限（不截断）。`details`：`part`（`arguments` / `result` / `resource`）、`sizeBytes`、`limitBytes`。 */
  | 'PAYLOAD_TOO_LARGE'
  /**
   * 被本机的 `deny` 策略规则拒绝（{@link PolicyConfig}），操作未执行；重试不会改变结果。
   * `details`：`ruleId`（命中规则的 id，不附规则内容）、`hook`（`call` / `wake`）、`appId`、`tool`。
   */
  | 'POLICY_DENIED'
  /** 需要用户本人操作（登录、授权、切到前台、在 App 内确认）后才能继续。`details`：`reason?`（`login` / `permission` / `foreground` / `confirm` 等）、`uri?`。 */
  | 'USER_ACTION_REQUIRED'
  /** 导航没有完成（不支持 / 出错 / 超时 / 导航后工具未出现，spec/protocol.md 3.4）。`details`：`reason`、`appId`、`page`。 */
  | 'NAVIGATION_FAILED'
  /** 导航被拒绝（App 拒绝或页面不可由 Agent 导航）。`details`：`reason`（`app` / `not-navigable`）、`appId`、`page`。 */
  | 'NAVIGATION_DENIED'

/**
 * 标准 MCP 工具注解（spec/protocol.md 第 3 节）。Hub 原样传递 App 的声明，不据此做判断；
 * `HubTool.annotations` 中缺少的字段已按 `risk` 推导。
 */
export interface ToolAnnotations {
  title?: string
  /** 不修改任何状态。 */
  readOnlyHint?: boolean
  /** 可能做出破坏性 / 不可撤销的修改。 */
  destructiveHint?: boolean
  /** 以相同参数重复调用没有额外效果。 */
  idempotentHint?: boolean
  /** 会与外部世界交互。 */
  openWorldHint?: boolean
}

/** 内容的接收方（MCP `Role`）。 */
export type Audience = 'user' | 'assistant'

/** 标准 MCP 内容注解（结果 / 资源内容的标注），原样来自 App。 */
export interface ContentAnnotations {
  audience?: Audience[]
  /** 重要程度，0 到 1。 */
  priority?: number
  /** 最后修改时刻（ISO 8601）。 */
  lastModified?: string
}

/**
 * 调用结果的业务状态（spec/protocol.md 3.2）：`done` 已完成；`pending` 已受理、待用户在 App 内确认或异步完成；
 * `partial` 只完成了一部分；`noop` 没有做任何改动。
 */
export type ResultStatus = 'done' | 'pending' | 'partial' | 'noop'

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

/**
 * 自适应租约策略：租约 = 同一（会话, App）最近 `window` 个调用间隔的 p90 + `marginMs`，限制在 [`minMs`, `maxMs`]；
 * 样本不足 3 个时用 `leaseTtlMs`。`window = 0` 或 `minMs > maxMs` 时 `Hub.start` 失败。
 */
export interface LeaseConfig {
  /** 是否按调用间隔自适应，缺省 true；false = 固定 `leaseTtlMs`（4e 之前的行为）。 */
  adaptive?: boolean
  /** 统计最近多少个间隔，缺省 20。 */
  window?: number
  /** p90 之上的余量，缺省 5000。 */
  marginMs?: number
  /** 自适应租约下限，缺省 5000。 */
  minMs?: number
  /** 自适应租约上限，缺省 60000；超过它的间隔不计入统计。 */
  maxMs?: number
  /** 会话无请求这么久后收回其默认租约，缺省 30000；0 = 不因空闲收回。 */
  idleRevokeMs?: number
}

/**
 * 资源保护（spec/hub-api.md 3.11）：调用频率（令牌桶，`*PerMinute` 为 0 = 不限；不限以外 `*Burst` 必须 ≥ 1，否则 `Hub.start` 失败）
 * 与大小上限（字节，0 = 不限）。超出时调用以 `RATE_LIMITED` / `PAYLOAD_TOO_LARGE` 结束。缺省字段取默认值。
 */
export interface LimitsConfig {
  /** 每（App, 工具）每分钟调用数，缺省 120。 */
  toolRatePerMinute?: number
  /** 每（App, 工具）的突发量，缺省 30。 */
  toolRateBurst?: number
  /** 每 App（所有工具合计）每分钟调用数，缺省 600。 */
  appRatePerMinute?: number
  /** 每 App 的突发量，缺省 60。 */
  appRateBurst?: number
  /** 调用参数（JSON）上限，缺省 1 MiB。 */
  maxArgumentsBytes?: number
  /** 调用结果上限，缺省 4 MiB。 */
  maxResultBytes?: number
  /** 资源内容上限，缺省 4 MiB。 */
  maxResourceBytes?: number
}

/** 结果与工具 `outputSchema` 不符时的处理：`off` 不校验；`log`（默认）只记日志；`reject` 调用以 `HANDLER_ERROR` 结束。 */
export type OutputValidation = 'off' | 'log' | 'reject'

/** 策略规则的动作：`hide` 不出现在任何列表中、调用为 `TOOL_NOT_FOUND`；`deny` 可见，在 `hooks` 指定的执行点以 `POLICY_DENIED` 拒绝。 */
export type PolicyAction = 'hide' | 'deny'

/** 策略执行点。规则的 `hooks` 只能写 `call` / `wake`（`list` 由 `hide` 隐式使用，`handle` 尚未实现）。 */
export type PolicyHook = 'list' | 'call' | 'wake' | 'handle'

/** 按 App 声明的 MCP 注解匹配：给出的每一项都与工具注解相等才命中（工具未声明该项时不命中）。至少给出一项。 */
export interface AnnotationMatch {
  readOnlyHint?: boolean
  destructiveHint?: boolean
  idempotentHint?: boolean
  openWorldHint?: boolean
}

/** 一条策略规则。`tool` 与 `annotations` 都缺省时作用于整个 App。 */
export interface PolicyRule {
  /** `[A-Za-z0-9_.-]{1,64}`，在规则集中唯一；`POLICY_DENIED` 的 `details.ruleId`。 */
  id: string
  action: PolicyAction
  /** appId（或上游名）：精确名，或以 `*` 结尾的前缀；`*` 匹配全部。 */
  app: string
  /** 工具局部名（不含 appId），规则同 `app`。 */
  tool?: string
  annotations?: AnnotationMatch
  /** 只用于 `deny`：`call` / `wake` 的非空子集，缺省 `['call']`。 */
  hooks?: PolicyHook[]
}

/** 策略规则集（spec/hub-api.md 3.13）：按顺序匹配，`deny` 取第一条命中的规则；空规则集 = 不做任何限制。 */
export interface PolicyConfig {
  rules?: PolicyRule[]
}

/** 一条生效的规则及其命中次数（自本规则集生效以来拒绝或按不存在处理的调用 / 唤醒次数，列表过滤不计）。 */
export interface PolicyRuleStatus extends PolicyRule {
  hits: number
}

/** 策略状态（`Hub.policy()`、`HubStatus.policy`）。 */
export interface PolicyStatus {
  rules: PolicyRuleStatus[]
  /** 当前规则集生效的时刻（Unix 毫秒）。 */
  loadedAtMs: number
  /** 最近一次 `setPolicy` 失败的原因（之前的规则继续生效）；之后成功加载时清除。 */
  lastError?: { message: string; atMs: number }
}

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
   * 持久状态目录（spec/hub-api.md 3.5「持久化」）：休眠记录写到 `<stateDir>/dormant/<appId>.json`（原子写、仅当前用户可读），
   * 启动时读回，重启前休眠的 App 仍可列出、可唤醒。缺省不读写任何文件。
   */
  stateDir?: string
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
  /** 调用进度的最小转发间隔（`callTool` 的 `onProgress`，spec/hub-api.md 3.12），缺省 250。 */
  progressIntervalMs?: number
  /** 等待配对回调的上限，缺省 120000。 */
  pairingTimeoutMs?: number
  // ---- 生命周期（spec/hub-api.md 3.5）----
  /** 调用实例完成后发送的租约时长，缺省 60000；0 关闭租约。 */
  leaseTtlMs?: number
  /** 唤醒后等待 App 回连的上限，缺省 15000（超时 → APP_NOT_RESPONDING）。 */
  wakeTimeoutMs?: number
  /**
   * 导航等待上限（App 回复 + 目标工具注册，spec/hub-api.md 3.14 / 3.15），缺省 5000，独立于 `wakeTimeoutMs`
   * （超时 → NAVIGATION_FAILED）。
   */
  navigateTimeoutMs?: number
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
  /** 自适应租约（spec/lifecycle.md 第 13 节 B2、spec/hub-api.md 3.5）。缺省字段取默认值。 */
  lease?: LeaseConfig
  /** 唤醒器，缺省 `system`。 */
  waker?: WakerConfig
  // ---- 资源保护（spec/hub-api.md 3.11）----
  /** 调用频率与大小上限。缺省字段取默认值。 */
  limits?: LimitsConfig
  /** 结果与 `outputSchema` 不符时的处理，缺省 `log`。 */
  outputValidation?: OutputValidation
  // ---- 策略挂点（spec/hub-api.md 3.13）----
  /** 隐藏 / 拒绝规则；缺省无规则（行为不变）。规则不合法时 `Hub.start` 失败。运行中用 `Hub.setPolicy` 替换。 */
  policy?: PolicyConfig
  // ---- 渐进暴露（spec/hub-api.md 3.7）----
  /** 工具暴露方式，缺省 `auto`。 */
  toolExposure?: ToolExposure
  /** `auto` 的阈值，缺省 40。 */
  toolExposureThreshold?: number
  // ---- 无会话 MCP 请求（spec/hub-api.md 3.6 / 3.7）----
  /** 无会话调用方（`principal:<主体>`）的 Agent 任务在请求流空闲多久后回收（收回租约、清除选择），缺省 600000；0 不因空闲回收。 */
  taskIdleTtlMs?: number
  /** 无会话请求的工具暴露方式，缺省 `all`；渐进时列表只含内置工具与全局选定实例的 App，不随调用变化。 */
  statelessToolExposure?: ToolExposure
  /** 无会话请求的主体级 `apps.select` 选择的空闲有效期，缺省 60000；0 不单独过期。 */
  principalSelectTtlMs?: number
  /** 无会话请求的列表结果所带缓存提示 `ttlMs`，缺省 5000。 */
  statelessListTtlMs?: number
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
  /** 旧写法；Agent 侧以 `annotations` 为准。 */
  risk: Risk
  /** Agent 看到的 MCP 工具注解：App 声明的字段原样，缺少的按 `risk` 推导；上游工具为其原样注解。 */
  annotations: ToolAnnotations
  /** App 声明的结果 JSON Schema（原样）；未声明时缺省。 */
  outputSchema?: JsonSchema
  activation: Activation
  availability: Availability
  /** App 工具的界面依赖；内置与上游工具缺省。 */
  surface?: ToolSurface
  /** App 工具所在页面（spec/hub-api.md 3.14）；不属于页面时缺省。 */
  page?: string
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
  /** 资源内容的标注；未声明时缺省。 */
  annotations?: ContentAnnotations
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
  /**
   * Agent 的幂等键（1..=256 个字符），原样转交 App（spec/hub-api.md 3.15）；同一键的重复调用由 App 决定是否只执行一次。
   * 不合法时调用以 `INVALID_INPUT` 结束。
   */
  idempotencyKey?: string | null
}

/** 一条调用进度（spec/hub-api.md 3.12）：已按 `progressIntervalMs` 合并、丢弃不递增的值；`message` 最长 200 字符。 */
export interface ProgressUpdate {
  progress: number
  /** 未知时缺省。 */
  total?: number | null
  message?: string | null
}

/** {@link Hub.callTool} 的选项。 */
export interface CallToolOptions {
  /**
   * 接收调用进度：在 Node 事件循环上逐条调用，全部先于返回的 Promise 完成；调用结束后不再回调。
   * 抛错交给 `onListenerError`，不影响调用。
   */
  onProgress?: (progress: ProgressUpdate) => void
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
  /** App 声明的业务状态；旧 Hub 缺省（视为 `done`）。 */
  status?: ResultStatus
  /** `pending` 时可读取后续状态的资源 URI（`app-mcp://<appId>/<资源名>`）。 */
  stateResource?: string
  /** App 给出的一句结论。 */
  summary?: string
  /** App 对结果内容的标注。 */
  annotations?: ContentAnnotations
  /** App 在后台、Hub 改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）。 */
  routedTo?: string
  /** Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App）；旧 Hub 缺省。 */
  durationMs?: number
  /** 本次 App 工具调用是否经历了唤醒（调用时目标未连接）；内置 / 上游工具为 `false`；旧 Hub 缺省。 */
  woke?: boolean
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

/**
 * 已连接实例当前不能休眠的原因（Hub 可见部分；App 的 hold() 只有 SDK 知道）。
 * `subscription` 只计声明 `realtime` 的资源的订阅（spec/lifecycle.md 第 13 节 B3）。
 */
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
  /** Hub 启动以来因限流被拒绝的调用次数。旧 Hub 缺省。 */
  rateLimited?: number
  /** Hub 启动以来因大小上限被拒绝的参数 / 结果 / 资源次数。旧 Hub 缺省。 */
  tooLarge?: number
  /** 各工具的声明（`risk` 与 MCP 注解）；没有工具或旧 Hub 时缺省。 */
  tools?: ToolDeclaration[]
}

/** 一个工具的声明（{@link AppStatus.tools}）。 */
export interface ToolDeclaration {
  /** 局部名（不含 appId）。 */
  name: string
  /** 旧写法 `risk`（未声明时为 `write`）。 */
  risk: Risk
  /** App 声明的注解（原样）；未声明时缺省。 */
  annotations?: ToolAnnotations
  /** Agent 实际看到的注解（声明优先，缺少的按 `risk` 推导）。 */
  effective: ToolAnnotations
  /** 是否声明了 `outputSchema`。 */
  outputSchema: boolean
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
  /** 租约策略与统计（spec/lifecycle.md 第 13 节 B2）；旧 Hub 缺省。 */
  lease?: LeaseStatus
  /** 资源保护策略（全部字段给出）；旧 Hub 缺省。 */
  limits?: Required<LimitsConfig>
  /** 结果与 `outputSchema` 不符时的处理；旧 Hub 缺省。 */
  outputValidation?: OutputValidation
  /** 策略规则、命中次数与最近的加载错误；旧 Hub 缺省。 */
  policy?: PolicyStatus
  /** 休眠记录持久化状态；未配置 `stateDir` 或旧 Hub 时缺省。 */
  dormantStore?: DormantStoreStatus
  /** Agent 任务（调用方的跨请求状态，spec/hub-api.md 3.6），按 `caller` 排序；旧 Hub 缺省。 */
  tasks?: AgentTaskStatus[]
}

/** 调用方的种类：legacy MCP 会话 / 无会话 MCP 请求的主体 / Hub API 会话。 */
export type CallerKind = 'mcpSession' | 'principal' | 'api'

/** 一个 Agent 任务（{@link HubStatus.tasks}）。 */
export interface AgentTaskStatus {
  /** Hub 签发的任务 ID（`task-<128 位十六进制>`）。 */
  id: string
  /** 调用方键：`mcp:<n>` / `principal:<主体>` / `api` / `api:<session>`。 */
  caller: string
  kind: CallerKind
  /** 未过期的 `apps.select` 选择，按 appId 排序。 */
  selections: TaskSelectionStatus[]
  /** 本任务发出、尚未到期且实例仍连接的租约，按连接 ID 排序。 */
  leases: TaskLeaseStatus[]
  /** 进行中的请求数。 */
  inflight: number
  /** 距最近一次请求活动的毫秒数；没有活动记录时缺省。 */
  idleMs?: number
}

export interface TaskSelectionStatus {
  appId: string
  instanceId: string
  /** 距失效的毫秒数（主体级选择，`principalSelectTtlMs`）；不单独过期时缺省。 */
  expiresInMs?: number
}

export interface TaskLeaseStatus {
  /** 实例的连接 ID（与 {@link InstanceInfo.connectionId} 相同）。 */
  connectionId: string
  /** 距到期的毫秒数。 */
  expiresInMs: number
}

/** 休眠记录持久化状态（`HubConfig.stateDir`）。 */
export interface DormantStoreStatus {
  /** 休眠记录目录（`<stateDir>/dormant`）。 */
  dir: string
  /** 启动时读回的实例数。 */
  loadedInstances: number
  /** 启动时因过期丢弃的实例数。 */
  expiredInstances: number
  /** 启动以来成功写入 / 删除文件的次数。 */
  writes: number
  /** 启动时跳过的文件（损坏、版本未知、超出上限）。 */
  issues: StoreIssue[]
  /** 最近一次写入失败。 */
  lastError?: string
}

/** 读取 / 写入中被跳过的文件或失败。 */
export interface StoreIssue {
  /** 文件名（相对于休眠记录目录）。 */
  file: string
  /** 中文说明。 */
  reason: string
}

/** 租约策略与统计。 */
export interface LeaseStatus {
  /** `adaptive` / `fixed`（固定 `leaseTtlMs`）/ `off`（`leaseTtlMs = 0`）。 */
  mode: 'adaptive' | 'fixed' | 'off'
  /** 默认（无历史 / 固定）租约。 */
  defaultMs: number
  minMs: number
  maxMs: number
  marginMs: number
  window: number
  /** 请求流空闲收回阈值；0 = 不因空闲收回。 */
  idleRevokeMs: number
  adaptiveGrants: number
  defaultGrants: number
  /** 因会话结束收回的次数。 */
  revokedSessionEnd: number
  /** 因请求流空闲收回的次数。 */
  revokedIdle: number
  /** 当前跟踪的（会话, App）。 */
  pairs: LeasePairStatus[]
}

export interface LeasePairStatus {
  /** 会话键：MCP `mcp:<n>`，API `api` / `api:<session>`。 */
  session: string
  appId: string
  /** 窗口内的间隔样本数。 */
  samples: number
  /** 下一次调用完成后将发出的租约。 */
  nextTtlMs: number
  /** `nextTtlMs` 是否来自统计。 */
  adaptive: boolean
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
  /** 旧写法；按声明决定是否确认时以 `annotations` 为准。 */
  risk: Risk
  /** 工具的 MCP 注解（与 {@link HubTool.annotations} 相同）。 */
  annotations: ToolAnnotations
  arguments: unknown
  /** Hub API 为 `CallRequest.session` 原样；MCP 出口为调用方键（`mcp:<n>` / `principal:<主体>`）。 */
  session: string | null
  /** MCP 出口：发起调用的认证主体（取自传输层凭据，现在恒为 `local`）；经 `callTool` 发起时缺省。 */
  principal?: string
  /**
   * MCP 出口：客户端自报的 `clientInfo.name`；经 `callTool` 发起时缺省。
   * 自报、不可信，**仅供显示**，不得据此做授权决定。
   */
  clientName?: string
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
  /** 工具声明了 `outputSchema` 时给出（根类型不是 object 时已包装为 `{ result }`）。 */
  outputSchema?: JsonSchema
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
