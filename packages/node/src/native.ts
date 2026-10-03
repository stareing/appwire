/**
 * 原生模块（bindings/node，napi-rs）的 JS 形状与加载。
 *
 * 原生模块由 `scripts/build-native.mjs` 构建，复制为
 * `native/app_mcp_node.<platform>-<arch>.node`。
 */

import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

export type NativeStatus =
  | 'idle'
  | 'connecting'
  | 'handshaking'
  | 'pending-pairing'
  | 'connected'
  | 'backoff'
  | 'rejected'
  | 'stopped'
  | 'dormant'
  | 'waking'
  | 'host-mismatch'

export interface NativeStateInfo {
  status: NativeStatus
  retryInMs?: number | null
  reason?: string | null
  /** 错误码（spec/protocol.md 10.1）。 */
  code?: string | null
}

export type NativeClientEvent =
  | { type: 'state'; state: NativeStateInfo }
  | { type: 'paired'; token: string }
  | { type: 'log'; level: 'debug' | 'info' | 'warn' | 'error'; message: string }
  /** 已休眠且驻留策略允许退出进程（spec/lifecycle.md 第 3 节 residency）。 */
  | { type: 'idle-exit' }

/** 生命周期策略（原生侧字段；取值见 spec/lifecycle.md 第 3 节）。 */
export interface NativeLifecycleConfig {
  mode?: 'persistent' | 'idle' | 'on-demand'
  idleTimeoutMs?: number
  hiddenIdleTimeoutMs?: number
  graceMs?: number
  residency?: 'keep' | 'exit-when-idle' | 'exit-always'
  wake?: { kind: string; target?: string; background?: boolean }
  hostAbsentRetries?: number
  legacyTimers?: boolean
  mergeWindowMs?: number
  sleepOnBackground?: boolean
}

/** 阻止自动休眠的持有；`release()` 幂等。 */
export interface NativeHold {
  release(): void
}

export interface NativeClientConfig {
  appId: string
  appName: string
  instanceId?: string
  clientKind?: 'native' | 'hybrid' | 'web'
  hostUrl?: string
  appVersion?: string
  instanceTitle?: string
  token?: string
  launchToken?: string
  maxConcurrentCalls?: number
  /** 排队中的调用上限（spec/protocol.md 5.3），缺省 64，0 = 不限（旧版原生模块忽略）。 */
  maxQueuedCalls?: number
  overview?: { summary: string; body?: string; locale?: string }
  lifecycle?: NativeLifecycleConfig
  connectTimeoutMs?: number
  heartbeat?: 'auto' | 'always' | 'off'
  /** 调用去重（旧版原生模块忽略）。 */
  callDedup?: { ttlMs?: number; maxEntries?: number }
  /** 按名寻址：在系统名字服务登记（旧版原生模块忽略）。 */
  registerName?: boolean
  /** 登记实例名（旧版原生模块忽略）。 */
  nameInstance?: string
}

export interface NativeToolSpec {
  name: string
  description: string
  inputSchemaJson?: string
  risk?: string
  activation?: string
  title?: string
  enabled?: boolean
  /** 标准 MCP 工具注解（旧版原生模块忽略）。 */
  annotations?: NativeToolAnnotations
  /** 结果的 JSON Schema 文本（旧版原生模块忽略）。 */
  outputSchemaJson?: string
  /** `'app'`（缺省）/ `'view'`：对界面的依赖（spec/protocol.md 3.4；旧版原生模块忽略）。 */
  surface?: string
  /** 所在页面名（旧版原生模块忽略）。 */
  page?: string
  /** 后台替代：同一 App 中一个 `app` 工具的局部名（spec/protocol.md 3.4；旧版原生模块忽略）。 */
  backgroundTool?: string
  /** 本工具同时执行的调用上限（spec/protocol.md 5.3）；缺省 / 0 = 不单独限制（旧版原生模块忽略）。 */
  concurrency?: number
  /** 互斥组（spec/protocol.md 5.3）：同组的工具同一时刻至多一个在执行（旧版原生模块忽略）。 */
  exclusive?: string
}

export interface NativeToolAnnotations {
  title?: string
  readOnlyHint?: boolean
  destructiveHint?: boolean
  idempotentHint?: boolean
  openWorldHint?: boolean
}

export interface NativeContentAnnotations {
  audience?: string[]
  priority?: number
  lastModified?: string
}

/** `NativeCall.completeWith` 的参数（缺省 = 无返回值、`done`）。 */
export interface NativeCallResult {
  dataJson?: string | null
  stateHints?: string[]
  /** `'done'` / `'pending'` / `'partial'` / `'noop'`。 */
  status?: string
  stateResource?: string
  summary?: string
  annotations?: NativeContentAnnotations
}

export interface NativeResourceSpec {
  name: string
  description: string
  mimeType?: string
  realtime?: boolean
  /** 资源内容的标注（旧版原生模块忽略）。 */
  annotations?: NativeContentAnnotations
}

export type NativeCancelReason = 'requested' | 'timeout' | 'disconnected' | 'stopped'

export interface NativeCall {
  readonly callId: string
  readonly toolName: string
  readonly argumentsJson: string
  /** Agent 给出的幂等键（spec/protocol.md 3.3）；没有时为 `null`。旧版原生模块没有此属性（`undefined`）。 */
  readonly idempotencyKey?: string | null
  isCancelled(): boolean
  setCancelListener(listener: (reason: NativeCancelReason) => void): void
  complete(dataJson?: string | null, stateHints?: string[]): void
  /** 成功完成并附带业务状态、摘要与内容标注。旧版原生模块没有此方法。 */
  completeWith?(result: NativeCallResult): void
  fail(kind: string, message: string): void
  /** 失败完成并附带详情（JSON 文本）。旧版原生模块没有此方法。 */
  failWithDetails?(kind: string, message: string, detailsJson?: string | null): void
  /** 调用完成后仍阻止自动休眠，直到释放。调用已结束时抛出 `ALREADY_COMPLETED`。 */
  hold?(): NativeHold
  /** 报告进度（spec/protocol.md 3.3）。调用已结束时抛出 `ALREADY_COMPLETED`。旧版原生模块没有此方法。 */
  reportProgress?(progress: number, total?: number | null, message?: string | null): void
}

export interface NativeRead {
  readonly resourceName: string
  complete(contentsJson: string): void
  fail(kind: string, message: string): void
  /** 失败完成并附带详情（JSON 文本）。旧版原生模块没有此方法。 */
  failWithDetails?(kind: string, message: string, detailsJson?: string | null): void
}

/** 一次导航请求（Host 的 `app/navigate`，spec/protocol.md 3.4）。完成只能一次，重复完成抛出 `ALREADY_COMPLETED`。 */
export interface NativeNavigate {
  readonly page: string
  /** 页面参数 JSON 文本；Host 没有给出时为 `undefined` / `null`。 */
  readonly paramsJson?: string | null
  complete(): void
  /** 导航失败（`NAVIGATION_FAILED`，`reason: "error"`）。 */
  fail(message: string): void
  /** 拒绝导航（`NAVIGATION_DENIED`，`reason: "app"`）。 */
  deny(message: string): void
  /** 需要用户操作（`USER_ACTION_REQUIRED`，`reason` / `uri` 缺省不出现）。旧版原生模块没有此方法。 */
  failUserAction?(message: string, reason?: string | null, uri?: string | null): void
}

export interface NativeTool {
  readonly name: string
  /** 整体替换定义；已声明的 `annotations` / `outputSchemaJson` 保持不变（忽略参数中的这两项）。 */
  update(spec: NativeToolSpec): void
  /** 整体替换定义与选项：`annotations` / `outputSchemaJson` 缺省表示清除。旧版原生模块没有此方法。 */
  updateWith?(spec: NativeToolSpec): void
  setEnabled(enabled: boolean): void
  dispose(): void
}

export interface NativeResource {
  readonly name: string
  notifyChanged(): void
  dispose(): void
}

export interface NativeRegistrar {
  registerTool(spec: NativeToolSpec, handler: (call: NativeCall) => void): NativeTool
  registerResource(spec: NativeResourceSpec, reader: (read: NativeRead) => void): NativeResource
  createScope(name: string): NativeScope
}

export interface NativeScope extends NativeRegistrar {
  dispose(): void
}

export interface NativeClient extends NativeRegistrar {
  readonly instanceId: string
  /** Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）。 */
  readonly connectionId?: string | null
  readonly state: NativeStateInfo
  readonly token: string | null
  start(): void
  stop(): void
  setVisibility(visibility: string, focused: boolean): void
  /** 设置导航回调；`null` 清除。握手时声明能力，应在 `start()` 之前设置。旧版原生模块没有此方法。 */
  setNavigationHandler?(handler: ((navigate: NativeNavigate) => void) | null): void
  /** 不可见时导航请求是否仍交给导航回调（缺省按平台：桌面 true，移动端 false）。旧版原生模块没有此方法。 */
  setNavigateInBackground?(enabled: boolean): void
  // ---- 生命周期（旧版原生模块没有这些方法）----
  handleWake?(args: string): boolean
  wake?(reason?: string | null): boolean
  connectNow?(): boolean
  sleep?(reason?: string | null): boolean
  hold?(): NativeHold
  toolsHash?(): string
  /** 测试用：tokio 运行时当前是否存在（休眠时为 false）。 */
  runtimeActive?(): boolean
}

export interface NativeBinding {
  NativeClient: new (
    config: NativeClientConfig,
    listener?: (event: NativeClientEvent) => void,
  ) => NativeClient
}

/** 原生模块文件名：`app_mcp_node.<platform>-<arch>.node`。 */
export function nativeFileName(platform: string = process.platform, arch: string = process.arch): string {
  return `app_mcp_node.${platform}-${arch}.node`
}

let cached: NativeBinding | undefined

/**
 * 加载原生模块。查找顺序：环境变量 `APP_MCP_NODE_NATIVE`（完整路径）→ 包内 `native/` 目录。
 */
export function loadNativeBinding(): NativeBinding {
  if (cached) return cached
  const require = createRequire(import.meta.url)
  const override = process.env.APP_MCP_NODE_NATIVE
  const here = dirname(fileURLToPath(import.meta.url))
  const file = nativeFileName()
  // dist/index.js 与 src/native.ts 都位于包根目录的下一级。
  const candidates = override ? [override] : [join(here, '..', 'native', file)]
  for (const path of candidates) {
    if (existsSync(path)) {
      cached = require(path) as NativeBinding
      return cached
    }
  }
  throw new Error(
    `找不到 @app-mcp/node 的原生模块 ${file}（已查找：${candidates.join(', ')}）。` +
      '请运行 `pnpm --filter @app-mcp/node build:native`，或用环境变量 APP_MCP_NODE_NATIVE 指定路径。',
  )
}
