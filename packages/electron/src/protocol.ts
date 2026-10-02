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

/**
 * 导航（第 4c 项，spec/protocol.md 3.4）：桥接协议版本 1 内的可选新增，不升版本（旧主进程对未知 op 回复错误，页面据此得知不支持）。
 *
 * - 页面 → 主进程：`navigation.set {enabled}`（本页处理导航；最近一次开启的页面为目标）、
 *   `navigate.result {navId, ok, kind?, message?}`（回复一次导航）；工具定义可带 `surface` / `page`。
 * - 主进程 → 页面：`navigate {navId, page, params?}`。
 *
 * @compat 消息类型的唯一定义应在 @app-mcp/web（`electron-bridge.ts` 的 `RendererOp` / `MainEvent` / `ToolSpecMessage`）；
 *   并入之前暂由本文件定义，Tauri 插件（crates/tauri-plugin/src/bridge.rs）与 @app-mcp/tauri 实现同一形状。
 */
export type NavigationOp =
  | { op: 'navigation.set'; enabled: boolean }
  | { op: 'navigate.result'; navId: number; ok: true }
  | { op: 'navigate.result'; navId: number; ok: false; kind: 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED'; message: string }

/** 主进程请求页面导航。`params` 缺省 = Host 没有给出参数。 */
export interface NavigateEvent {
  type: 'navigate'
  navId: number
  page: string
  params?: unknown
}

/** 工具定义（`ToolSpecMessage`）上的可选新增：对界面的依赖与所在页面；`tool.update` 时缺省表示清除。 */
export interface NavigationToolFields {
  surface?: 'app' | 'view'
  page?: string
}

export type {
  AppMcpBridge,
  HelloReply,
  MainEvent,
  OpReply,
  Outcome,
  RendererOp,
  ToolSpecMessage,
} from '@app-mcp/web'
