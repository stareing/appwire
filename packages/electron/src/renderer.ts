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
import { DEFAULT_BRIDGE_KEY, type AppMcpBridge } from './protocol.js'

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
//
// 页面处理 Host 的导航请求（主进程需 `attachAppMcp({ navigation: true })`）：通常直接用 `appMcp.setNavigationHandler(...)`
// （或 @app-mcp/react 的 `useRouterNavigation`）；没有 AppMcp 实例时用 `attachBridgeNavigation(bridge, handler)`。
// 实现与消息类型在 @app-mcp/web（桥接客户端），这里只重新导出。

export {
  attachBridgeNavigation,
  type BridgeNavigation,
  type NavigationHandler as BridgeNavigationHandler,
  type NavigationRequest as BridgeNavigationRequest,
} from '@app-mcp/web'
