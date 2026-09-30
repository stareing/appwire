/**
 * 惰性 handler（spec/lifecycle.md 第 10.2 节第 3 条）：工具只声明元数据，handler 在首次调用时加载。
 * 驱动层与 Electron 桥接共用。
 */

import type { ToolHandler, ToolHandlerLoader } from './types'

/** 保存 handler / 加载器的记录（驱动层的 ToolRec、桥接层的 ToolEntry）。 */
export interface LazySlot {
  handler: ToolHandler<any, any> | undefined
  load: ToolHandlerLoader<any, any> | undefined
  loading: Promise<ToolHandler<any, any>> | undefined
}

/** `handler` 与 `load` 必须恰好给出一个。 */
export function checkHandlerOrLoad(name: string, def: { handler?: unknown; load?: unknown }): void {
  const hasHandler = typeof def.handler === 'function'
  const hasLoad = typeof def.load === 'function'
  if (hasHandler && hasLoad) throw new Error(`工具 ${JSON.stringify(name)} 同时给出了 handler 与 load，只能二选一`)
  if (!hasHandler && !hasLoad) throw new Error(`工具 ${JSON.stringify(name)} 缺少 handler（或惰性加载器 load）`)
}

/** 解析加载器的结果：handler 本身或 `{ default: handler }`。 */
export function resolveLoadedHandler(loaded: unknown): ToolHandler<any, any> {
  if (typeof loaded === 'function') return loaded as ToolHandler<any, any>
  const d = (loaded as { default?: unknown } | null | undefined)?.default
  if (typeof d === 'function') return d as ToolHandler<any, any>
  throw new Error('load() 应返回 handler 函数或 { default: handler }')
}

/** 执行惰性加载（并发调用共享同一次加载；失败后下次调用重试；加载期间 setHandler 优先）。 */
export function loadHandler(rec: LazySlot): Promise<ToolHandler<any, any>> {
  if (rec.handler) return Promise.resolve(rec.handler)
  const load = rec.load
  if (!load) return Promise.reject(new Error('没有 handler'))
  if (!rec.loading) {
    const loading: Promise<ToolHandler<any, any>> = Promise.resolve()
      .then(() => load())
      .then(resolveLoadedHandler)
      .then(
        (handler) => {
          if (rec.loading === loading) {
            rec.handler = handler
            rec.loading = undefined
          }
          return rec.handler ?? handler
        },
        (e: unknown) => {
          if (rec.loading === loading) rec.loading = undefined
          throw e
        },
      )
    rec.loading = loading
  }
  return rec.loading
}
