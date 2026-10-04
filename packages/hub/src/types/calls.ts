/** @app-mcp/hub 的公开类型：调用、进度与事件（spec/hub-api.md 3.2、3.12）。 */

import type { ContentAnnotations, ErrorKind, ResultStatus, Visibility } from '../types.js'
import type { AppOverviewInfo } from './apps.js'
import type { AppEvent } from './events.js'
import type { UndoOffer } from './undo.js'

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
  /**
   * 调用优先级（第 16 项 P6，spec/hub-api.md 3.15）：原样转交 App，App 的调用队列先按它、再按到达顺序调度；缺省 `'normal'`。
   * 不合法时 `callTool` 抛 `HubError`（请求 JSON 无法解析）。
   */
  priority?: CallPriority | null
  /**
   * 不查只读结果缓存、照常调用 App 并以新结果覆盖（spec/hub-api.md 3.20；MCP 出口为请求 `_meta` 的
   * `dev.appwire/cache: "bypass"`）。缺省 `false`。
   */
  cacheBypass?: boolean
}

/** 调用优先级：`interactive`（用户在场等结果）> `normal` > `background`（定时、批量等后台作业）。 */
export type CallPriority = 'interactive' | 'normal' | 'background'

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
  /**
   * 结果来自只读结果缓存（未转发给 App、未唤醒）时距 App 产出的毫秒数；未命中时缺省（spec/hub-api.md 3.20，与 MCP 结果 `_meta`
   * 的 `dev.appwire/cached.ageMs` 相同）。
   */
  cachedAgeMs?: number
  /** 本次调用已登记撤销（spec/hub-api.md 3.23），可用 `apps.undo` 撤销；未登记或旧 Hub 缺省。 */
  undo?: UndoOffer
  /** `apps.undo` 的结果：被撤销调用的 callId（与 MCP 结果 `_meta` 的 `dev.appwire/undoOf` 相同）；其他调用缺省。 */
  undoOf?: string
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
  /** App 发出的事件（已去重与校验，不论有无订阅；spec/hub-api.md 3.17）。 */
  | ({ type: 'appEvent' } & AppEvent)
