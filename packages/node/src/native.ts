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
  overview?: { summary: string; body?: string; locale?: string }
  lifecycle?: NativeLifecycleConfig
  connectTimeoutMs?: number
}

export interface NativeToolSpec {
  name: string
  description: string
  inputSchemaJson?: string
  risk?: string
  activation?: string
  title?: string
  enabled?: boolean
}

export interface NativeResourceSpec {
  name: string
  description: string
  mimeType?: string
}

export type NativeCancelReason = 'requested' | 'timeout' | 'disconnected' | 'stopped'

export interface NativeCall {
  readonly callId: string
  readonly toolName: string
  readonly argumentsJson: string
  isCancelled(): boolean
  setCancelListener(listener: (reason: NativeCancelReason) => void): void
  complete(dataJson?: string | null, stateHints?: string[]): void
  fail(kind: string, message: string): void
  /** 失败完成并附带详情（JSON 文本）。旧版原生模块没有此方法。 */
  failWithDetails?(kind: string, message: string, detailsJson?: string | null): void
  /** 调用完成后仍阻止自动休眠，直到释放。调用已结束时抛出 `ALREADY_COMPLETED`。 */
  hold?(): NativeHold
}

export interface NativeRead {
  readonly resourceName: string
  complete(contentsJson: string): void
  fail(kind: string, message: string): void
}

export interface NativeTool {
  readonly name: string
  update(spec: NativeToolSpec): void
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
  readonly state: NativeStateInfo
  readonly token: string | null
  start(): void
  stop(): void
  setVisibility(visibility: string, focused: boolean): void
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
