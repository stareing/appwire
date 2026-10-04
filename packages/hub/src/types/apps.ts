/** @app-mcp/hub 的公开类型：App、实例、工具与资源（spec/hub-api.md 3.1）。 */

import type { Activation, ContentAnnotations, Risk, ToolAnnotations, ToolSurface, Visibility } from '../types.js'
import type { ToolDeprecation } from './evolution.js'

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
  /** App 工具声明实现的标准意图（spec/intents.md，如 `message.send@1`）；未声明、内置与上游工具缺省。 */
  implements?: string[]
  /** App 工具定义的 `schemaHash`（spec/hub-api.md 3.21）：`inputSchema` / `outputSchema` 变化时随之变化；内置与上游工具缺省。 */
  schemaHash?: string
  /** App 工具的弃用声明（原样）；未弃用、内置与上游工具缺省。 */
  deprecated?: ToolDeprecation
  /** App 工具声明了 `undoable`（spec/protocol.md 3.8，只用于展示：结果可能带撤销）；未声明、内置与上游工具缺省。 */
  undoable?: boolean
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
