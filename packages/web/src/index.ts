/**
 * @app-mcp/web：浏览器 SDK。
 *
 * ```ts
 * import { createAppMcp } from '@app-mcp/web'
 * export const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城' })
 * appMcp.tool('cart.clear', { description: '清空购物车', handler: () => cart.clear() })
 * ```
 */

import { createDriver } from './driver'
import { createBridgeAppMcp, findElectronBridge } from './electron-bridge'
import { globalBroadcastChannel } from './instance-guard'
import { createDisabledAppMcp } from './noop'
import type { AppMcp, AppMcpOptions } from './types'
import { createSharedLink } from './shared-connection'
import { loadWasmCore } from './wasm-loader'

export * from './types'
export { DEFAULT_HOST_URL, DEFAULT_HOST_URLS, SDK_VERSION } from './driver'
export { isToolResultEnvelope } from './result'
export { MAX_EVENT_PAYLOAD_BYTES } from './events'
export { attachBridgeNavigation, type BridgeNavigation } from './bridge-navigation'
// 界面级暴露（spec/protocol.md 3.4）：层栈与 view 工具门控
export { createViewLayer, openViewLayers, refreshViewTools } from './view'
// 路由适配共用（React Router 适配在 @app-mcp/react，Vue Router 适配在 @app-mcp/web/vue-router）
export { guardDenial, pagePath, routePages, type GuardResult, type RouteLike } from './router'
export {
  BRIDGE_VERSION,
  createBridgeAppMcp,
  DEFAULT_BRIDGE_KEY,
  findElectronBridge,
  type AppMcpBridge,
  type EventMessage,
  type EventOp,
  type HelloReply,
  type MainEvent,
  type NavigateEvent,
  type NavigationOp,
  type OpReply,
  type Outcome,
  type RendererOp,
  type ToolSpecMessage,
} from './electron-bridge'

/**
 * 创建 SDK 实例（同步返回）。WASM 核心在后台按需加载，加载前的注册缓存在 JS 侧。
 * `enabled: false` 时返回空操作实现，不加载 WASM、不连接。
 *
 * 默认同一来源的多个标签页经 SharedWorker 共用一条到 Host 的连接（`sharedConnection: false` 关闭），
 * 每个标签页仍是独立实例。
 *
 * 在 Electron 渲染进程中（preload 用 `@app-mcp/electron/preload` 暴露了桥接对象）自动改走主进程：
 * 经 IPC 登记工具，不加载 WASM、不连接 Host（见 electron-bridge.ts）。
 */
export function createAppMcp(options: AppMcpOptions): AppMcp {
  if (options.enabled === false) return createDisabledAppMcp(options)
  const bridge = findElectronBridge()
  if (bridge) return createBridgeAppMcp(options, bridge)
  const createBroadcastChannel = globalBroadcastChannel()
  return createDriver(options, {
    loadCore: loadWasmCore,
    ...(createBroadcastChannel && { createBroadcastChannel }),
    // 同一来源的标签页共用一条连接（spec/protocol.md 第 9 节）
    ...(options.sharedConnection !== false && { createSharedLink }),
  })
}
