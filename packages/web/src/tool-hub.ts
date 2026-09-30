/**
 * 驱动层的内部钩子：让同包内的其他入口（如 `@app-mcp/web/webmcp`）观察工具的注册、更新与注销，
 * 并在本地执行工具。不属于公开契约，签名可能随时变化。
 *
 * @internal
 */

import type { CoreOutcome } from './core'
import type { ToolDefinition } from './types'

/** 工具的当前定义（不含 handler 与 anchor）。 */
export type ToolInfo = Omit<ToolDefinition<any, any>, 'handler' | 'anchor'>

/** @internal */
export interface ToolView {
  readonly name: string
  /** 当前定义（`update()` 后反映最新值）。 */
  readonly definition: Readonly<ToolInfo>
  /** 在本地执行工具，与 Host 调用走同一路径（zod 校验、handler、结果规范化）。 */
  call(input: unknown, signal: AbortSignal): Promise<CoreOutcome>
}

/** @internal */
export type ToolHubEvent =
  | { type: 'register'; tool: ToolView }
  | { type: 'update'; tool: ToolView }
  | { type: 'unregister'; name: string }

/** @internal */
export interface ToolHub {
  /** 当前已注册（未注销）的全部工具。 */
  list(): ToolView[]
  /** 订阅注册事件，返回取消订阅函数。 */
  subscribe(listener: (event: ToolHubEvent) => void): () => void
  /**
   * 可让位的工具名 → 让位回调。`appMcp.tool()` 遇到同名工具时，若该名字在此表中，
   * 先调用回调（回调须同步注销旧工具），再继续注册。
   */
  readonly yieldable: Map<string, () => void>
}

const hubs = new WeakMap<object, ToolHub>()

/** @internal 由驱动层在构造时调用。 */
export function attachToolHub(owner: object, hub: ToolHub): void {
  hubs.set(owner, hub)
}

/** @internal 取得 AppMcp 实例的钩子；空操作实现（`enabled: false`）没有钩子。 */
export function getToolHub(owner: object): ToolHub | undefined {
  return hubs.get(owner)
}
