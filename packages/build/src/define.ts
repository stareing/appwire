/**
 * 运行时安全的定义辅助：静态工具定义（不含 handler）与 App 总览。本模块不依赖 Node 或 Vite，可以在浏览器运行时代码中导入
 * （`@app-mcp/build/define`），用同一份定义注册带 handler 的工具，保证静态清单与运行时一致。
 */
import type { Activation, InputDefinition, Risk } from '@app-mcp/web'

export interface StaticToolDefinition<I = unknown> {
  /** 工具名，`[a-zA-Z0-9_.-]{1,64}`，不含 appId。 */
  name: string
  description: string
  title?: string
  /** JSON Schema、zod v4 schema 或带 `toJSONSchema()` 的对象；缺省为无参数。 */
  input?: InputDefinition<I>
  /** 缺省 'write'。 */
  risk?: Risk
  activation?: Activation
}

/** 定义一组静态工具（恒等函数，只用于类型推断）。作为 `staticTools` 模块的默认导出。 */
export function defineStaticTools<const T extends readonly StaticToolDefinition<any>[]>(tools: T): T {
  return tools
}

/** 定义单个静态工具（恒等函数，保留 input 的类型以便注册时推断 handler 参数）。 */
export function defineStaticTool<I>(tool: StaticToolDefinition<I>): StaticToolDefinition<I> {
  return tool
}

export {
  defineOverview,
  OVERVIEW_BODY_MAX,
  OVERVIEW_SECTIONS,
  OVERVIEW_SUMMARY_MAX,
  validateOverview,
} from './overview'
export type { AppOverview } from '@app-mcp/web'
