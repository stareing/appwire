/**
 * 渲染进程（页面）侧：通过 preload 暴露的桥接对象把工具 / 资源登记到主进程。
 *
 * ```ts
 * import { createRendererAppMcp } from '@app-mcp/electron/renderer'
 * export const appMcp = createRendererAppMcp({ appId: 'shop', appName: '示例商城' })
 * appMcp.tool('cart.clear', { description: '清空购物车', handler: () => cart.clear() })
 * ```
 *
 * 实现位于 @app-mcp/web（`createBridgeAppMcp`）：页面直接用 `@app-mcp/web` 的 `createAppMcp` 时，
 * 检测到桥接对象也会走同一实现（不加载 WASM）。本入口的区别只是桥接对象可以显式传入、找不到时抛错。
 *
 * 返回值满足 @app-mcp/web 的 `AppMcp` 接口，页面代码（包括 @app-mcp/react）无需修改。
 * 身份与连接由主进程负责：`appId` / `appName` / `hostUrl` 等选项在页面中被忽略，
 * `instanceId`、`state` 与 `connectionId` 取自主进程客户端（首次握手完成前 `instanceId` 为空字符串）。
 */

import { createBridgeAppMcp, type AppMcp, type AppMcpOptions } from '@app-mcp/web'
import {
  DEFAULT_BRIDGE_KEY,
  type AppMcpBridge,
  type NavigateEvent,
  type NavigationOp,
  type RendererOp,
} from './protocol.js'

export type { AppMcpBridge, MainEvent, NavigateEvent, NavigationOp, RendererOp } from './protocol.js'
/** 页面 handler 抛出以指定错误类别（如 `ToolCallError.userActionRequired(...)`），类别与详情经 IPC 原样送到主进程。 */
export { ToolCallError, type UserActionReason, type UserActionRequiredOptions } from '@app-mcp/web'

export interface RendererAppMcpOptions extends AppMcpOptions {
  /** 桥接对象，缺省取 `window[bridgeKey]`。 */
  bridge?: AppMcpBridge
  /** 缺省 `appMcpBridge`（与 preload 的 `exposeAppMcpBridge` 一致）。 */
  bridgeKey?: string
}

/** 读取 preload 暴露的桥接对象；不在 Electron 页面中时返回 undefined。 */
export function getAppMcpBridge(key: string = DEFAULT_BRIDGE_KEY): AppMcpBridge | undefined {
  const bridge = (globalThis as Record<string, unknown>)[key] as AppMcpBridge | undefined
  if (bridge && typeof bridge.request === 'function' && typeof bridge.onMessage === 'function') return bridge
  return undefined
}

/** 页面到主进程的 IPC 传输（即 preload 暴露的桥接对象）；找不到时抛出说明性错误。 */
export function createIpcTransport(key: string = DEFAULT_BRIDGE_KEY): AppMcpBridge {
  const bridge = getAppMcpBridge(key)
  if (!bridge) {
    throw new Error(`未找到 window.${key}：请在 preload 中调用 exposeAppMcpBridge(contextBridge, ipcRenderer)`)
  }
  return bridge
}

/** 在 Electron 页面中创建 AppMcp（经 preload 桥接登记到主进程）。 */
export function createRendererAppMcp(options: RendererAppMcpOptions): AppMcp {
  const enabled = options.enabled !== false
  const bridge = enabled ? (options.bridge ?? createIpcTransport(options.bridgeKey)) : null
  const { bridge: _bridge, bridgeKey: _key, ...rest } = options
  return createBridgeAppMcp(rest, bridge)
}

// ---------------------------------------------------------------------------
// 导航（spec/protocol.md 3.4）
// ---------------------------------------------------------------------------

/** 导航请求（与 @app-mcp/web / @app-mcp/node 的 `NavigationRequest` 同形）。 */
export interface BridgeNavigationRequest {
  page: string
  params: Record<string, unknown> | undefined
}

/** 页面的导航回调：切换路由后返回；抛出 `ToolCallError.navigationDenied(...)` 拒绝，其他异常按导航失败回复。 */
export type BridgeNavigationHandler = (request: BridgeNavigationRequest) => void | Promise<void>

export interface BridgeNavigation {
  /** 主进程接受本页处理导航后兑现；主进程未开启导航转发（`attachAppMcp({ navigation: true })`）或旧主进程时拒绝。 */
  readonly ready: Promise<void>
  /** 停止处理导航（通知主进程，取消订阅）。幂等。 */
  dispose(): void
}

/**
 * 让本页处理 Host 的导航请求（主进程需 `attachAppMcp({ navigation: true })`）：主进程把 `app/navigate` 转给最近一次
 * 开启导航的页面，本函数调用 `handler` 并回复结果。在 `createRendererAppMcp` / `createAppMcp` 之后调用（页面的 `hello`
 * 会重置本页登记）。
 *
 * ```ts
 * const appMcp = createRendererAppMcp({ appId: 'shop', appName: '示例商城' })
 * attachBridgeNavigation(getAppMcpBridge()!, ({ page, params }) => router.push({ name: page, query: params }))
 * ```
 *
 * @why 页面侧 SDK（@app-mcp/web 的桥接实现）尚未提供 `setNavigationHandler` 时的显式入口；消息形状见 protocol.ts `NavigationOp`。
 * @error 回调抛出 `kind` 为 `NAVIGATION_DENIED` 的错误 → 拒绝；其他 → 失败（消息为异常的 `message`）。
 */
export function attachBridgeNavigation(bridge: AppMcpBridge, handler: BridgeNavigationHandler): BridgeNavigation {
  // @compat RendererOp / MainEvent 的唯一定义在 @app-mcp/web，尚未包含导航消息
  const send = (op: NavigationOp) => bridge.request(op as unknown as RendererOp)
  let disposed = false
  const unsubscribe = bridge.onMessage((message) => {
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
        // @why 主进程已关闭或本页会话已注销：主进程侧的等待会随会话结束失败，这里无需处理
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

/** 回调抛出的值 → 回复的类别与消息（按结构识别 ToolCallError 的 `kind`）。 */
function navigationFailure(error: unknown): { kind: 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED'; message: string } {
  const e = (typeof error === 'object' && error !== null ? error : {}) as { kind?: unknown; message?: unknown }
  const message = typeof e.message === 'string' && e.message !== '' ? e.message : String(error ?? '导航失败')
  return { kind: e.kind === 'NAVIGATION_DENIED' ? 'NAVIGATION_DENIED' : 'NAVIGATION_FAILED', message }
}
