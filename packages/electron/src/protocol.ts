/**
 * 主进程 ↔ 渲染进程的 IPC 消息（@app-mcp/electron 内部协议）。
 *
 * - 渲染进程 → 主进程：`ipcRenderer.invoke(CHANNEL_OP, op)`，主进程 `ipcMain.handle` 返回 {@link OpReply}。
 * - 主进程 → 渲染进程：`webContents.send(CHANNEL_EVENT, event)`。
 *
 * 渲染进程只登记工具 / 资源的描述，handler 留在页面中；主进程在自己的 @app-mcp/node 客户端上
 * 代为注册，调用时经 IPC 转发给页面执行。所有消息都是可结构化克隆的普通对象。
 *
 * 消息类型的唯一定义在 @app-mcp/web（`src/electron-bridge.ts`）：页面直接使用 `createAppMcp`
 * 时，@app-mcp/web 检测到桥接对象后按本协议工作，不加载 WASM。
 */

export const CHANNEL_OP = 'app-mcp:op'
export const CHANNEL_EVENT = 'app-mcp:event'
/** preload 默认把桥接对象暴露为 `window.appMcpBridge`。 */
export const DEFAULT_BRIDGE_KEY = 'appMcpBridge'
/**
 * 须与 @app-mcp/web 的 `BRIDGE_VERSION` 一致（见 compat.test.ts）。
 * 这里不从 @app-mcp/web 运行时导入：preload（可能是沙箱环境）与主进程不应加载页面 SDK。
 */
export const BRIDGE_VERSION = 1

export type {
  AppMcpBridge,
  HelloReply,
  MainEvent,
  OpReply,
  Outcome,
  RendererOp,
  ToolSpecMessage,
} from '@app-mcp/web'
