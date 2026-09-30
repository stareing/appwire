/**
 * preload 脚本：在页面的 `window` 上暴露最小桥接对象（只能访问 app-mcp 的两个 IPC 通道）。
 *
 * ```ts
 * // preload.ts
 * import { contextBridge, ipcRenderer } from 'electron'
 * import { exposeAppMcpBridge } from '@app-mcp/electron/preload'
 * exposeAppMcpBridge(contextBridge, ipcRenderer)
 * ```
 */

import {
  BRIDGE_VERSION,
  CHANNEL_EVENT,
  CHANNEL_OP,
  DEFAULT_BRIDGE_KEY,
  type AppMcpBridge,
  type MainEvent,
  type OpReply,
  type RendererOp,
} from './protocol.js'

export type { AppMcpBridge, MainEvent, OpReply, RendererOp } from './protocol.js'

/** Electron `ipcRenderer` 的最小接口。 */
export interface IpcRendererLike {
  invoke(channel: string, ...args: any[]): Promise<any>
  on(channel: string, listener: (event: unknown, ...args: any[]) => void): unknown
  removeListener(channel: string, listener: (event: unknown, ...args: any[]) => void): unknown
}

/** Electron `contextBridge` 的最小接口。 */
export interface ContextBridgeLike {
  exposeInMainWorld(apiKey: string, api: any): void
}

export interface ExposeOptions {
  /** 暴露在 `window` 上的名称，默认 `appMcpBridge`。 */
  key?: string
  /** 页面卸载（`pagehide`）时通知主进程注销本页登记。默认 true。 */
  resetOnPageHide?: boolean
  /** 未开启 contextIsolation 时直接挂到此对象上（默认 `globalThis`）。 */
  target?: Record<string, unknown>
}

/** 创建桥接对象（不暴露）。 */
export function createAppMcpBridge(ipcRenderer: IpcRendererLike): AppMcpBridge {
  return {
    version: BRIDGE_VERSION,
    request: (op: RendererOp) => ipcRenderer.invoke(CHANNEL_OP, op) as Promise<OpReply>,
    onMessage(listener: (event: MainEvent) => void) {
      const wrapped = (_event: unknown, message: MainEvent) => listener(message)
      ipcRenderer.on(CHANNEL_EVENT, wrapped)
      return () => {
        ipcRenderer.removeListener(CHANNEL_EVENT, wrapped)
      }
    },
  }
}

/**
 * 把桥接对象暴露给页面。`contextBridge` 为 null（未开启 contextIsolation）时直接挂到 `target` 上。
 */
export function exposeAppMcpBridge(
  contextBridge: ContextBridgeLike | null,
  ipcRenderer: IpcRendererLike,
  options: ExposeOptions = {},
): AppMcpBridge {
  const bridge = createAppMcpBridge(ipcRenderer)
  const key = options.key ?? DEFAULT_BRIDGE_KEY
  if (contextBridge) contextBridge.exposeInMainWorld(key, bridge)
  else (options.target ?? (globalThis as Record<string, unknown>))[key] = bridge

  const events = globalThis as { addEventListener?: (type: string, listener: () => void) => void }
  if (options.resetOnPageHide !== false && typeof events.addEventListener === 'function') {
    events.addEventListener('pagehide', () => {
      void ipcRenderer.invoke(CHANNEL_OP, { op: 'reset' } satisfies RendererOp).catch(() => {})
    })
  }
  return bridge
}
