/** @app-mcp/hub 的公开类型：运行状态（spec/hub-api.md 3.9；与 `GET /status` 相同）。 */

import type { LimitsConfig, OutputValidation, PolicyStatus, Risk, ToolAnnotations } from '../types.js'
import type { InstanceInfo } from './apps.js'

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
  /** 已初始化的 legacy MCP 会话数。 */
  mcpSessions: number
  /** 进行中的 `subscriptions/listen` 流数（spec/hub-api.md 3.6「通知」）；旧 Hub 缺省。 */
  mcpListenStreams?: number
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
  /** 已登记的 Agent 名（第 16 项 N5，不含令牌）；旧 Hub 不报告。 */
  agents?: string[]
  /** 按调用方记账（第 16 项 P3），按主体排序；旧 Hub 不报告。 */
  usage?: UsageStatus[]
  /** 未到期的对象锁（第 16 项 N6），按 appId、key 排序；旧 Hub 不报告。 */
  locks?: LockStatus[]
}

/** 一把未到期的对象锁（{@link HubStatus.locks}，spec/hub-api.md 3.6「对象锁」）。 */
export interface LockStatus {
  appId: string
  /** 命名锁的名字；App 锁缺省。 */
  key?: string
  /** 持有者的调用方键（`mcp:<n>` / `principal:<主体>` / `api` / `api:<session>` 等）。 */
  caller: string
  /** 持有者的记账主体：`agent:<名>` / `local` / `api`。 */
  holder: string
  /** 距到期的毫秒数。 */
  expiresInMs: number
}

/** 计数（主体合计与每 App 共用）。`wakes`：为该主体发起的唤醒（与进行中的唤醒合并的也计入）。 */
export interface UsageCounts {
  calls: number
  wakes: number
  rateLimited: number
  argumentsBytes: number
  resultBytes: number
}

/** 一个记账主体的用量：`subject` 为 `agent:<名>` / `local` / `api` / `other`（主体数达上限后）。 */
export interface UsageStatus extends UsageCounts {
  subject: string
  /** 主体为已登记 Agent 时的名字。 */
  agent?: string
  /** 按 App 细分，按 appId 排序。 */
  apps: Array<UsageCounts & { appId: string }>
  /** 细分条目达到上限后，新 App 的用量只计入合计。 */
  appsTruncated?: boolean
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
  /** 发起方 Agent 名（第 16 项 N5）；本机主体与 Hub API 缺省。 */
  agent?: string
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
