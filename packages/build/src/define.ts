/**
 * 运行时安全的定义辅助：静态工具定义（不含 handler）与 App 总览。本模块不依赖 Node 或 Vite，可以在浏览器运行时代码中导入
 * （`@app-mcp/build/define`），用同一份定义注册带 handler 的工具，保证静态清单与运行时一致。
 */
import type { Activation, InputDefinition, OutputDefinition, Risk, ToolAnnotations, ToolSurface } from '@app-mcp/web'

export interface StaticToolDefinition<I = unknown> {
  /** 工具名，`[a-zA-Z0-9_.-]{1,64}`，不含 appId。 */
  name: string
  description: string
  title?: string
  /** JSON Schema、zod v4 schema 或带 `toJSONSchema()` 的对象；缺省为无参数。 */
  input?: InputDefinition<I>
  /** 旧写法：缺省 'write'。新代码优先用 `annotations`。 */
  risk?: Risk
  /** 标准 MCP 工具注解。 */
  annotations?: ToolAnnotations
  /** 结果的 schema（MCP `outputSchema`）：JSON Schema、zod v4 schema（按输出形态转换）或带 `toJSONSchema()` 的对象。 */
  outputSchema?: OutputDefinition<unknown>
  activation?: Activation
  /** 对界面的依赖（spec/protocol.md 3.4），缺省 `app`。 */
  surface?: ToolSurface
  /** 所在页面名；写在 {@link PageDefinition.tools} 中时可省略（写了必须等于页面名）。 */
  page?: string
}

/**
 * 页面定义（清单 `pages` 条目，spec/manifest.md 2.3）：页面说明、导航参数与页面内工具。用于路由扫描无法静态确定的页面
 * （工具输入是 zod schema、工具名不是字面量等），或补充 / 覆盖扫描结果（同名页面以显式定义为准）。
 */
export interface PageDefinition {
  /** 页面名 `[a-zA-Z0-9_.-]{1,64}`，即 `app/navigate` 的 `page`。 */
  name: string
  title?: string
  /** 页面说明（给模型读）。 */
  description?: string
  /** App 内路由（如 `/orders/:id`）。 */
  route?: string
  /** 导航参数：JSON Schema、zod v4 schema 或带 `toJSONSchema()` 的对象（根类型 object）。 */
  params?: InputDefinition<unknown>
  /** 能否由 Agent 导航过去，缺省 true。 */
  navigable?: boolean
  activation?: Activation
  /** 页面内工具（通常 `surface: 'view'`）。 */
  tools?: readonly StaticToolDefinition<any>[]
}

/** 定义单个页面（恒等函数，保留工具定义的类型以便运行时复用）。 */
export function definePage<const T extends PageDefinition>(page: T): T {
  return page
}

/** 定义一组页面。作为插件 `pages` 模块的默认导出。 */
export function definePages<const T extends readonly PageDefinition[]>(pages: T): T {
  return pages
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
