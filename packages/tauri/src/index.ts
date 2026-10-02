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
  type RendererOp,
} from '@app-mcp/web'

export type { AppMcpBridge, HelloReply, MainEvent, OpReply, RendererOp } from '@app-mcp/web'
/** 页面 handler 抛出以指定错误类别（如 `ToolCallError.userActionRequired(...)`），类别与详情经插件原样送到 Rust 侧。 */
export { ToolCallError, type UserActionReason, type UserActionRequiredOptions } from '@app-mcp/web'
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

// ---------------------------------------------------------------------------
// 导航（spec/protocol.md 3.4）
// ---------------------------------------------------------------------------
//
// 消息（桥接协议版本 1 内的可选新增，形状与 @app-mcp/electron 的 `NavigationOp` / `NavigateEvent` 相同，Rust 侧实现在
// crates/tauri-plugin/src/bridge.rs）：页面 → Rust `navigation.set {enabled}`、`navigate.result {navId, ok, kind?, message?}`；
// Rust → 页面 `navigate {navId, page, params?}`。
// @compat 唯一定义应并入 @app-mcp/web 的 RendererOp / MainEvent；与 @app-mcp/electron/renderer 的 attachBridgeNavigation
//   为同一实现的两份副本（两包互不依赖），并入 @app-mcp/web 时一并收拢。

/** 导航请求（与 @app-mcp/web / @app-mcp/node 的 `NavigationRequest` 同形）。 */
export interface TauriNavigationRequest {
  page: string
  params: Record<string, unknown> | undefined
}

/** 页面的导航回调：切换路由后返回；抛出 `ToolCallError.navigationDenied(...)` 拒绝，其他异常按导航失败回复。 */
export type TauriNavigationHandler = (request: TauriNavigationRequest) => void | Promise<void>

export interface TauriNavigation {
  /** Rust 侧接受本页处理导航后兑现；插件未开启 `Builder::page_navigation(true)` 或旧插件时拒绝。 */
  readonly ready: Promise<void>
  /** 停止处理导航（通知 Rust 侧，取消订阅）。幂等。 */
  dispose(): void
}

type NavigationOp =
  | { op: 'navigation.set'; enabled: boolean }
  | { op: 'navigate.result'; navId: number; ok: true }
  | { op: 'navigate.result'; navId: number; ok: false; kind: 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED'; message: string }

interface NavigateEvent {
  type: 'navigate'
  navId: number
  page: string
  params?: unknown
}

/**
 * 让本页处理 Host 的导航请求（Rust 侧 `Builder::page_navigation(true)`）：插件把 `app/navigate` 转给最近一次开启导航的
 * WebView，本函数调用 `handler` 并回复结果。
 *
 * ```ts
 * const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城' })
 * attachTauriNavigation(({ page, params }) => router.push({ name: page, query: params }))
 * ```
 *
 * @error 找不到桥接时抛出；回调抛出 `kind` 为 `NAVIGATION_DENIED` 的错误 → 拒绝，其他 → 失败。
 */
export function attachTauriNavigation(handler: TauriNavigationHandler, bridge?: AppMcpBridge): TauriNavigation {
  const target = bridge ?? getTauriBridge()
  if (!target) throw new Error('未找到 window.appMcpBridge：请在 Rust 侧注册插件 tauri_plugin_app_mcp')
  const send = (op: NavigationOp) => target.request(op as unknown as RendererOp)
  let disposed = false
  const unsubscribe = target.onMessage((message) => {
    const event = message as unknown as NavigateEvent
    if (disposed || event.type !== 'navigate') return
    const params = isPlainObject(event.params) ? event.params : undefined
    Promise.resolve()
      .then(() => handler({ page: event.page, params }))
      .then(
        () => send({ op: 'navigate.result', navId: event.navId, ok: true }),
        (error: unknown) => send({ op: 'navigate.result', navId: event.navId, ok: false, ...navigationFailure(error) }),
      )
      .catch(() => {
        // @why 插件已停止或本页已卸载：Rust 侧的等待随页面卸载失败，这里无需处理
      })
  })
  const ready = send({ op: 'navigation.set', enabled: true }).then((reply) => {
    if (!reply.ok) throw new Error(reply.message)
  })
  return {
    ready,
    dispose() {
      if (disposed) return
      disposed = true
      unsubscribe()
      void send({ op: 'navigation.set', enabled: false }).catch(() => {})
    },
  }
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function navigationFailure(error: unknown): { kind: 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED'; message: string } {
  const e = (typeof error === 'object' && error !== null ? error : {}) as { kind?: unknown; message?: unknown }
  const message = typeof e.message === 'string' && e.message !== '' ? e.message : String(error ?? '导航失败')
  return { kind: e.kind === 'NAVIGATION_DENIED' ? 'NAVIGATION_DENIED' : 'NAVIGATION_FAILED', message }
}
