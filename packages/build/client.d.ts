/**
 * 虚拟模块 `virtual:app-mcp/annotated` 的类型声明（由 @app-mcp/build 的 `annotations` 选项生成）。
 *
 * 在项目的 `src/vite-env.d.ts` 中引用：
 * ```ts
 * /// <reference types="@app-mcp/build/client" />
 * ```
 */
declare module 'virtual:app-mcp/annotated' {
  import type { Activation, JsonSchema, Registrar, Risk } from '@app-mcp/web'

  /** 由 `@mcp` 注释声明的工具元数据。 */
  export interface AnnotatedToolInfo {
    name: string
    description: string
    title?: string
    risk?: Risk
    activation?: Activation
    /** 从 TypeScript 参数类型推导的 JSON Schema。 */
    inputSchema: JsonSchema
    /** 源文件相对项目根目录的路径。 */
    source: string
    /** 模块导出名。 */
    exportName: string
    /** 类的静态方法名。 */
    member?: string
  }

  /** 全部注释工具的元数据。 */
  export const annotatedTools: readonly AnnotatedToolInfo[]

  /** 在 AppMcp 实例或 Scope 上注册全部注释工具，返回统一的 dispose。 */
  export function registerAnnotated(registrar: Pick<Registrar, 'tool'>): () => void
}
