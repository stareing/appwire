/**
 * Zustand 适配：action 是 state 上的函数，调用 `store.getState()[action](...args)`。
 *
 * ```ts
 * import { exposeZustand } from '@app-mcp/store/zustand'
 * exposeZustand(appMcp, useCart, { namespace: 'cart', actions: { add: { description: '加入购物车', input } } })
 * ```
 */

import { type Dispose, type ExposeOptions, actionNameOf, exposeStore } from './index'
import type { Registrar } from '@app-mcp/web'

/** Zustand vanilla store 或 React hook store（`create` 的返回值）的最小结构。 */
export interface ZustandStoreLike<S> {
  getState(): S
  subscribe(listener: (state: S, prevState: S) => void): () => void
}

export function exposeZustand<S>(registrar: Registrar, store: ZustandStoreLike<S>, options: ExposeOptions<S>): Dispose {
  const initial = store.getState() as Record<string, unknown> | null
  for (const [toolName, config] of Object.entries(options.actions)) {
    const action = actionNameOf(toolName, config)
    if (typeof initial?.[action] !== 'function') {
      throw new Error(`Zustand state 上不存在函数 ${action}（工具 ${toolName}）`)
    }
  }
  return exposeStore(
    registrar,
    {
      getState: () => store.getState(),
      subscribe: (listener) => store.subscribe(() => listener()),
    },
    options,
  )
}

export type { ExposeOptions, ActionOptions, ResourceOptions, Dispose } from './index'
