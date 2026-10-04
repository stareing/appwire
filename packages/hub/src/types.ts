/**
 * @app-mcp/hub 的公开类型：与 `crates/hub` 的 serde（camelCase）JSON 形态一一对应（spec/hub-api.md 3.1、3.4）。
 */

import type { EventLimitsConfig } from './types/events.js'

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
   * 对象锁冲突（spec/hub-api.md 3.6「对象锁」，只由 Hub 产生）：App 正被其他调用方以 `apps.lock` 锁定，写调用未转发、未唤醒；
   * 或要加的锁已被他人持有。`details`：`appId`、`key?`、`holder`（`agent:<名>` / `local` / `api`）、`retryAfterMs`。
   */
  | 'LOCKED'

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
 * MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」）：`auto`（默认：`initialize` 客户端走 legacy 会话，
 * 每请求自带 `_meta` 的客户端可协商 2026-07-28）/ `legacyOnly`（回退开关：只声明到 2025-11-25，`subscriptions/listen` 不可用）。
 */
export type McpProtocolMode = 'auto' | 'legacyOnly'

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
  /** 每个已登记 Agent（所有 App 合计）每分钟调用数，缺省 0（不限）；本机主体与 Hub API 不受此限。 */
  agentRatePerMinute?: number
  /** 每个已登记 Agent 的突发量（限流时须 ≥ 1）。 */
  agentRateBurst?: number
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
  /** 只对该 Agent（登记的名字，规则同 `app`）发起的操作生效；只用于 `deny`。 */
  agent?: string
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
  // ---- MCP 出口协议版本与通知（spec/hub-api.md 3.6）----
  /** 协商的协议版本范围，缺省 `auto`。 */
  mcpProtocolMode?: McpProtocolMode
  /** 每个主体同时打开的 `subscriptions/listen` 流数上限，缺省 16；0 不提供 listen。 */
  maxListenStreams?: number
  /** 一个 listen 流接受的资源 URI 数上限，缺省 256。 */
  maxListenResources?: number
  /**
   * 每个主体同时存在的任务句柄数上限（spec/hub-api.md 3.6「任务句柄」），缺省 32，超出时 `apps.task.begin` 报 `RATE_LIMITED`；
   * 0 不提供任务句柄（`apps.task.*` 不列出，`taskId` 一律无效）。
   */
  maxTaskHandles?: number
  /**
   * 每个持有者同时持有的对象锁数上限（spec/hub-api.md 3.6「对象锁」），缺省 16，超出时 `apps.lock` 报 `RATE_LIMITED`；
   * 0 不提供对象锁（`apps.lock` / `apps.unlock` 不列出，调用为 `TOOL_NOT_FOUND`）。
   */
  maxLocks?: number
  // ---- 事件信箱（spec/hub-api.md 3.17）----
  /** 订阅数、信箱容量、保留时长与每订阅频率上限；缺省字段取默认值（32 / 100 / 24 小时 / 60）。 */
  eventLimits?: EventLimitsConfig
  // ---- Agent 身份（spec/hub-api.md 3.6）----
  /** 按 Agent 发的访问令牌；缺省不登记（所有请求为本机主体）。不合法时 `Hub.start` 失败。运行中用 `Hub.setAgents` 替换。 */
  agents?: AgentCredential[]
  /** 上游 MCP 服务器：名称（appId 规则）→ 启动方式。 */
  upstreams?: Record<string, UpstreamConfig>
  approval?: ApprovalPolicy
}

/**
 * 一个 Agent 的访问令牌（spec/hub-api.md 3.6「Agent 身份」）：经 MCP HTTP 出口出示此令牌的请求，主体为 `agent:<name>`
 * （只用于区分与归属，不做授权）。
 */
export interface AgentCredential {
  /** 1–64 个 ASCII 字母、数字、`-`、`_`、`.`，以字母或数字开头。 */
  name: string
  /** 32–512 个可见 ASCII 字符、不含空白（`Authorization: Bearer <token>`）。 */
  token: string
}

// 其余各类型按职责分在 types/ 下；对外仍从本文件导出（index.ts 的 `export type * from './types.js'`）。
export type * from './types/apps.js'
export type * from './types/calls.js'
export type * from './types/status.js'
export type * from './types/callbacks.js'
export type * from './types/formats.js'
export type * from './types/events.js'
