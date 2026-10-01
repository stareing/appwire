/**
 * 原生模块（bindings/hub-node，napi-rs）的 JS 形状与加载。
 *
 * 原生模块由 `scripts/build-native.mjs` 构建，复制为
 * `native/app_mcp_hub_node.<platform>-<arch>.node`。
 * 所有复杂结构以 JSON 文本进出；错误消息形如 `[CODE] 说明`（见 errors.ts）。
 */

import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

export interface NativeHub {
  readonly listenAddr: string | null
  readonly ipcEndpoint: string | null
  readonly isShutdown: boolean
  shutdown(): Promise<void>
  apps(): string
  status(): string
  tools(filterJson?: string | null): string
  resources(): string
  overview(appId: string): string | null
  callTool(requestJson: string): Promise<string>
  cancelCall(callId: string): void
  readResource(uri: string): Promise<string>
  subscribe(uri: string): void
  unsubscribe(uri: string): void
  selectInstance(appId: string, instanceId?: string | null): void
  resetSession(session?: string | null): void
  exportTools(format: string, filterJson?: string | null): string
  dispatch(format: string, toolCallJson: string, session?: string | null): Promise<string>
  serveHttp(addr: string, allowRemote?: boolean | null): Promise<string>
  onEvent(listener: ((eventJson: string) => void) | null): void
  /** handler 必须返回 Promise<boolean>（封装层负责规整）。 */
  setApprovalHandler(handler: (requestJson: string) => Promise<boolean>): void
  setPairingHandler(handler: (requestJson: string) => Promise<boolean>): void
  /** handler 必须返回 Promise：resolve null = 成功；resolve 字符串 = 失败 `{"kind","message"}`。null 恢复默认实现。 */
  setWaker(handler: ((requestJson: string) => Promise<string | null>) | null): void
}

export interface HubBinding {
  Hub: { start(configJson?: string | null): Promise<NativeHub> }
}

/** 原生模块文件名：`app_mcp_hub_node.<platform>-<arch>.node`。 */
export function nativeFileName(platform: string = process.platform, arch: string = process.arch): string {
  return `app_mcp_hub_node.${platform}-${arch}.node`
}

let cached: HubBinding | undefined

/**
 * 加载原生模块。查找顺序：环境变量 `APP_MCP_HUB_NATIVE`（完整路径）→ 包内 `native/` 目录。
 */
export function loadNativeBinding(): HubBinding {
  if (cached) return cached
  const require = createRequire(import.meta.url)
  const override = process.env.APP_MCP_HUB_NATIVE
  const here = dirname(fileURLToPath(import.meta.url))
  const file = nativeFileName()
  // dist/index.js 与 src/native.ts 都位于包根目录的下一级。
  const candidates = override ? [override] : [join(here, '..', 'native', file)]
  for (const path of candidates) {
    if (existsSync(path)) {
      cached = require(path) as HubBinding
      return cached
    }
  }
  throw new Error(
    `找不到 @app-mcp/hub 的原生模块 ${file}（已查找：${candidates.join(', ')}）。` +
      '请运行 `pnpm --filter @app-mcp/hub build:native`，或用环境变量 APP_MCP_HUB_NATIVE 指定路径。',
  )
}
