/**
 * Pinia 适配：action 通过 `store[action](...args)` 调用；state 为 `store.$state`，
 * 订阅使用 `store.$subscribe(..., { detached: true })`。
 *
 * Pinia 的 state 被原地修改，资源变化检测基于 JSON 快照（见 README）。
 *
 * ```ts
 * import { exposePinia } from '@app-mcp/store/pinia'
 * exposePinia(appMcp, useCartStore(), { namespace: 'cart', actions: { add: { description: '加入购物车', input } } })
 * ```
 */

import { type Dispose, type ExposeOptions, actionNameOf, exposeStore } from './index'
import type { Registrar } from '@app-mcp/web'

/** Pinia store 实例（`useXxxStore()` 的返回值）的最小结构。 */
export interface PiniaStoreLike<S = any> {
  $state: S
  $subscribe(
    callback: (mutation: any, state: any) => void,
    options?: { detached?: boolean; flush?: 'pre' | 'post' | 'sync' },
  ): () => void
}

function lookup(store: object, name: string): unknown {
  return (store as Record<string, unknown>)[name]
}

export function exposePinia<P extends PiniaStoreLike>(
  registrar: Registrar,
  store: P,
  options: ExposeOptions<P['$state']>,
): Dispose {
  for (const [toolName, config] of Object.entries(options.actions)) {
    const action = actionNameOf(toolName, config)
    if (typeof lookup(store, action) !== 'function') {
      throw new Error(`Pinia store 上不存在 action ${action}（工具 ${toolName}）`)
    }
  }
  return exposeStore<P['$state']>(
    registrar,
    {
      getState: () => store.$state,
      // sync：每次修改都通知，由核心合并到微任务；detached：组件卸载后仍保持订阅，由 dispose 负责取消。
      subscribe: (listener) => store.$subscribe(() => listener(), { detached: true, flush: 'sync' }),
      invoke: (call) => {
        const fn = lookup(store, call.action)
        if (typeof fn !== 'function') throw new Error(`Pinia store 上不存在 action ${call.action}（工具 ${call.tool}）`)
        return (fn as (...args: unknown[]) => unknown).apply(store, call.args)
      },
      mutable: true,
    },
    options,
  )
}

export type { ExposeOptions, ActionOptions, ResourceOptions, Dispose } from './index'
