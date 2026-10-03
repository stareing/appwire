/**
 * `enabled: false` 时的空操作实现：不加载 WASM、不连接、不访问存储。
 */

import type { AppMcp, AppMcpOptions, ConnectionState, HoldHandle, ResourceHandle, Scope, ToolHandle } from './types'

const DISABLED: ConnectionState = Object.freeze({ status: 'disabled' })
const noop = (): void => {}

/** 空操作的持有句柄（`enabled: false` 与 Electron 桥接模式共用）。 */
export function noopHold(): HoldHandle {
  return { release: noop }
}

function tool(name: string): ToolHandle {
  return { name, update: noop, setHandler: noop, dispose: noop }
}

function resource(name: string): ResourceHandle {
  return { name, notifyChanged: noop, setReader: noop, dispose: noop }
}

function scope(name: string): Scope {
  return { name, tool, resource, scope, dispose: noop }
}

export function createDisabledAppMcp(options: AppMcpOptions): AppMcp {
  return {
    options: Object.freeze({ ...options }),
    instanceId: '',
    state: DISABLED,
    onStateChange: () => noop,
    tool,
    resource,
    scope,
    dispose: noop,
    wake: noop,
    sleep: noop,
    connectNow: noop,
    hold: noopHold,
    setNavigationHandler: noop,
    setNavigateInBackground: noop,
    setBusy: noop,
    beginBusy: noopHold,
    isBusy: () => false,
    setBusyPolicy: noop,
  }
}
