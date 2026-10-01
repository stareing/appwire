/**
 * @app-mcp/tauri：Tauri 页面侧。
 *
 * Rust 侧注册 `tauri-plugin-app-mcp` 后，插件给每个 WebView 注入初始化脚本（crates/tauri-plugin/js/bridge.js），
 * 在页面上暴露与 Electron preload 同形的宿主 IPC 桥接对象 `window.appMcpBridge`（协议见 @app-mcp/web 的
 * `electron-bridge.ts`）。因此页面里**直接用 `@app-mcp/web` 的 `createAppMcp`（或 `@app-mcp/react`）即可**，
 * 不需要本包：SDK 检测到桥接后经 Tauri `invoke` 把工具登记到 Rust 侧的原生客户端，不加载 WASM、不连接 Host。
 *
 * 本包提供显式入口：确定运行在 Tauri 中、希望插件缺失时直接报错（而不是退回 WebSocket 直连）时使用。
 *
 * ```ts
 * import { createTauriAppMcp } from '@app-mcp/tauri'
 * export const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城' })
 * appMcp.tool('cart.clear', { description: '清空购物车', handler: () => cart.clear() })
 * ```
 *
 * 身份与连接由 Rust 侧负责：`appId` / `appName` / `hostUrl` 等选项在页面中被忽略。
 */

import {
  BRIDGE_VERSION,
  createBridgeAppMcp,
  DEFAULT_BRIDGE_KEY,
  findElectronBridge,
  type AppMcp,
  type AppMcpBridge,
  type AppMcpOptions,
} from '@app-mcp/web'

export type { AppMcpBridge, HelloReply, MainEvent, OpReply, RendererOp } from '@app-mcp/web'
export { BRIDGE_VERSION }

/** 插件命令（页面 → Rust）。App 的 capability 需包含 `app-mcp:default`。 */
export const TAURI_OP_COMMAND = 'plugin:app-mcp|op'

/** Rust → 页面的事件经此全局函数送达（由插件的初始化脚本定义）。 */
export const TAURI_DISPATCH_FN = '__APP_MCP_TAURI_DISPATCH__'

export interface TauriAppMcpOptions extends AppMcpOptions {
  /** 桥接对象，缺省取插件注入的 `window.appMcpBridge`。 */
  bridge?: AppMcpBridge
}

/** 是否运行在 Tauri v2 WebView 中。 */
export function isTauri(target: object = globalThis): boolean {
  return '__TAURI_INTERNALS__' in target
}

/** 读取插件注入的桥接对象；不在 Tauri 中、未注册插件或版本不兼容时返回 undefined。 */
export function getTauriBridge(target: Record<string, unknown> = globalThis as unknown as Record<string, unknown>): AppMcpBridge | undefined {
  return findElectronBridge(target, DEFAULT_BRIDGE_KEY)
}

/** 在 Tauri 页面中创建 AppMcp（经插件桥接登记到 Rust 侧）。找不到桥接时抛出说明性错误。 */
export function createTauriAppMcp(options: TauriAppMcpOptions): AppMcp {
  const { bridge: explicit, ...rest } = options
  if (options.enabled === false) return createBridgeAppMcp(rest, null)
  const bridge = explicit ?? getTauriBridge()
  if (!bridge) {
    throw new Error(
      isTauri()
        ? '未找到 window.appMcpBridge：请在 Rust 侧注册插件 .plugin(tauri_plugin_app_mcp::init(...))（且未关闭 inject_bridge）'
        : '当前页面不在 Tauri WebView 中（没有 __TAURI_INTERNALS__）',
    )
  }
  return createBridgeAppMcp(rest, bridge)
}
