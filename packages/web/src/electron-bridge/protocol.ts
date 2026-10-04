/**
 * Electron 桥接协议（op/event 消息类型，BRIDGE_VERSION = 1）与桥接对象查找（自 electron-bridge.ts 拆出）。
 * 本文件是该协议消息类型的唯一定义，经 ../electron-bridge.ts 对外导出，@app-mcp/electron 从那里重新导出。
 */

import type { NormalizedResult } from '../result'
import type {
  Activation,
  CachePolicy,
  ConnectionState,
  ContentAnnotations,
  ErrorKind,
  JsonSchema,
  OutputSchema,
  Risk,
  ToolAnnotations,
  ToolDeprecation,
  ToolSurface,
} from '../types'

// ---------------------------------------------------------------------------
// 协议（BRIDGE_VERSION = 1）
// ---------------------------------------------------------------------------
//
// @compat 版本 1 内只做可选字段的新增，旧页面忽略、新页面缺省为 undefined，因此不升版本
// （升版本会让 findElectronBridge 拒绝新旧混用）。已有新增：`HelloReply.connectionId`、`state` 事件的 `connectionId`、
// `ToolSpecMessage.annotations` / `outputSchema` / `surface` / `page` / `backgroundTool` / `concurrency` / `exclusive` / `implements` / `cache` / `deprecated` / `undoable`、`resource.register` 的 `cache`、导航消息（`navigation.set`、`navigate`、
// `navigate.result`，见 {@link NavigationOp}；旧主进程对未知 op 回复错误，页面据此得知不支持；`navigate.result` 的
// `USER_ACTION_REQUIRED` 与 `details`：旧主进程 / Rust 侧按失败处理）、成功 `Outcome` 的 `status` / `stateResource` / `summary` / `annotations` / `undo`、
// 失败 `Outcome` 的 `details`、用户正在操作 `busy.set`（旧主进程对未知 op 回复错误，页面记警告）、事件 {@link EventOp}
// （同样按未知 op 回复错误，页面记警告）。

/** preload 默认把桥接对象暴露为 `window.appMcpBridge`。 */
export const DEFAULT_BRIDGE_KEY = 'appMcpBridge'
export const BRIDGE_VERSION = 1

export interface ToolSpecMessage {
  description: string
  title?: string
  inputSchema?: JsonSchema
  risk?: Risk
  /** 标准 MCP 工具注解；`tool.update` 时缺省表示清除。 */
  annotations?: ToolAnnotations
  /** 结果的 JSON Schema；`tool.update` 时缺省表示清除。 */
  outputSchema?: OutputSchema
  activation?: Activation
  enabled?: boolean
  /**
   * 对界面的依赖（spec/protocol.md 3.4）；缺省 `app`。`view` 工具的可见性门控在页面侧完成，结果体现在 `enabled` 中
   * （门控变化时页面发 `tool.update`）。`tool.update` 时缺省表示清除。
   */
  surface?: ToolSurface
  /** 所在页面名（spec/protocol.md 3.4）。 */
  page?: string
  /** 后台替代：同一 App 中一个 `app` 工具的名称（spec/protocol.md 3.4）；`tool.update` 时缺省表示清除。 */
  backgroundTool?: string
  /** 本工具同时执行的调用上限（spec/protocol.md 5.3）；`tool.update` 时缺省表示不单独限制。 */
  concurrency?: number
  /** 互斥组（spec/protocol.md 5.3）；`tool.update` 时缺省表示清除。 */
  exclusive?: string
  /** 实现的标准意图（spec/intents.md）；`tool.update` 时缺省表示清除。 */
  implements?: string[]
  /** 结果缓存声明（spec/protocol.md 3.6）；`tool.update` 时缺省表示清除。 */
  cache?: CachePolicy
  /** 弃用声明（spec/protocol.md 3.7）；`tool.update` 时缺省表示清除。 */
  deprecated?: ToolDeprecation
  /** 成功结果可能带 `undo`（spec/protocol.md 3.8）；`tool.update` 时缺省表示取消声明。 */
  undoable?: boolean
}

/**
 * 成功结果的字段同 {@link NormalizedResult}（`data` 之外均可选，旧主进程忽略新增字段）。
 * 失败的 `details` 来自 ToolCallError（JSON 对象，如 `USER_ACTION_REQUIRED` 的 `{ reason?, uri? }`），随错误的 `data` 发给 Host。
 */
export type Outcome =
  | ({ ok: true } & NormalizedResult)
  | { ok: false; kind: ErrorKind; message: string; details?: Record<string, unknown> }

export type RendererOp =
  /** 页面（重新）加载：主进程丢弃该 webContents 之前的全部登记。 */
  | { op: 'hello' }
  /** 页面卸载或 dispose：注销该 webContents 的全部登记。 */
  | { op: 'reset' }
  | { op: 'tool.register'; id: number; scopeId?: number; name: string; spec: ToolSpecMessage }
  | { op: 'tool.update'; id: number; spec: ToolSpecMessage }
  | { op: 'tool.dispose'; id: number }
  | {
      op: 'resource.register'
      id: number
      scopeId?: number
      name: string
      description: string
      mimeType?: string
      realtime?: boolean
      annotations?: ContentAnnotations
      /** 读取结果缓存声明（spec/protocol.md 3.6）。 */
      cache?: CachePolicy
    }
  | { op: 'resource.notify'; id: number }
  | { op: 'resource.dispose'; id: number }
  | { op: 'scope.create'; id: number; scopeId?: number; name: string }
  | { op: 'scope.dispose'; id: number }
  | ({ op: 'call.result'; callId: string } & Outcome)
  /** handler 的 `context.progress()`（spec/protocol.md 3.3），主进程转给 @app-mcp/node 的同名方法。 */
  | { op: 'call.progress'; callId: string; progress: number; total?: number; message?: string }
  | ({ op: 'read.result'; readId: number } & Outcome)
  /** 生命周期（转给主进程的 @app-mcp/node 客户端）：回连。 */
  | { op: 'lifecycle.wake' }
  /** 主动请求休眠。 */
  | { op: 'lifecycle.sleep' }
  /** `on-demand` 模式下主动连接。 */
  | { op: 'lifecycle.connectNow' }
  /** 持有（阻止自动休眠），`holdId` 由页面分配；页面刷新、卸载或 webContents 销毁时主进程释放该页全部持有。 */
  | { op: 'lifecycle.hold'; holdId: number }
  | { op: 'lifecycle.release'; holdId: number }
  /**
   * 本页声明用户正在 / 不再操作（spec/protocol.md 5.3）：对方按页面记录，客户端的 busy 为各页之或（只在汇总值变化时设置）；
   * 页面刷新、卸载或 webContents 销毁时该页的声明失效。
   */
  | { op: 'busy.set'; busy: boolean }
  | EventOp
  | NavigationOp

/**
 * 事件（spec/protocol.md 3.5）：页面 → 主进程 / Rust 侧。声明归本页（按页面记录，页面刷新、卸载或 webContents 销毁时撤销；
 * 其他页面也声明了同名事件时保留）。`event.emit` 的回复 `value` 为是否已发送（布尔）；未声明 / 载荷不合法时回复错误
 * （`code` 同原生：`INVALID_NAME` / `INVALID_JSON`）。页面先在本地校验，并在镜像的状态不是 `connected` 时不发送。
 */
export type EventOp =
  | { op: 'event.declare'; event: EventMessage }
  | { op: 'event.remove'; name: string }
  | { op: 'event.emit'; name: string; payload?: Record<string, unknown> }

/** 事件声明（与协议 `EventInfo` 同形）。 */
export interface EventMessage {
  name: string
  description: string
  payloadSchema?: Record<string, unknown>
}

/**
 * 导航（spec/protocol.md 3.4）：页面 → 主进程 / Rust 侧。`navigation.set`：本页处理（`enabled: true`）/ 不再处理导航，
 * 对方以最近一次开启的页面为目标，未开启导航转发时回复错误；`navigate.result`：回复一次 {@link NavigateEvent}。
 */
export type NavigationOp =
  | { op: 'navigation.set'; enabled: boolean }
  | { op: 'navigate.result'; navId: number; ok: true }
  | {
      op: 'navigate.result'
      navId: number
      ok: false
      kind: 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED' | 'USER_ACTION_REQUIRED'
      message: string
      /** `USER_ACTION_REQUIRED` 的 `{ reason?, uri? }`；其他类别不带。 */
      details?: Record<string, unknown>
    }

/** 主进程 / Rust 侧请求页面导航（Host 的 `app/navigate`）。`params` 缺省 = Host 没有给出参数。 */
export interface NavigateEvent {
  type: 'navigate'
  navId: number
  page: string
  params?: unknown
}

export interface HelloReply {
  instanceId: string
  state: ConnectionState
  /** 主进程客户端当前的连接 ID（spec/protocol.md 10.3）；未连接或旧主进程时缺省。 */
  connectionId?: string
}

export type OpReply = { ok: true; value?: unknown } | { ok: false; code?: string; message: string }

export type MainEvent =
  /** `idempotencyKey`：Agent 的幂等键（spec/protocol.md 3.3），没有时缺省（旧主进程 / Rust 侧也不带）。 */
  | { type: 'call'; callId: string; toolId: number; input: unknown; idempotencyKey?: string }
  | { type: 'cancel'; callId: string; kind: ErrorKind; message: string }
  | { type: 'read'; readId: number; resourceId: number }
  /** `connectionId`：该状态下主进程客户端的连接 ID；未连接或旧主进程时缺省。 */
  | { type: 'state'; state: ConnectionState; connectionId?: string }
  | NavigateEvent

/** preload 暴露给页面的最小桥接对象。 */
export interface AppMcpBridge {
  readonly version: number
  request(op: RendererOp): Promise<OpReply>
  /** 订阅主进程事件，返回取消订阅函数。 */
  onMessage(listener: (event: MainEvent) => void): () => void
}

// ---------------------------------------------------------------------------
// 检测
// ---------------------------------------------------------------------------

function isBridge(value: unknown): value is AppMcpBridge {
  if (typeof value !== 'object' || value === null) return false
  const b = value as Partial<AppMcpBridge>
  return typeof b.request === 'function' && typeof b.onMessage === 'function'
}

/**
 * 查找 Electron preload 暴露的桥接对象：依次尝试 `getAppMcpBridge()`（若页面上有这个函数）与 `appMcpBridge`。
 * 版本不兼容时返回 undefined（调用方退回 WebSocket + WASM）。
 */
export function findElectronBridge(
  target: Record<string, unknown> = globalThis as unknown as Record<string, unknown>,
  key: string = DEFAULT_BRIDGE_KEY,
): AppMcpBridge | undefined {
  let candidate: unknown
  try {
    const getter = target.getAppMcpBridge
    if (typeof getter === 'function') candidate = (getter as () => unknown)()
    if (!isBridge(candidate)) candidate = target[key]
  } catch {
    return undefined
  }
  if (!isBridge(candidate)) return undefined
  const version = (candidate as { version?: unknown }).version
  if (version !== undefined && version !== BRIDGE_VERSION) return undefined
  return candidate
}
