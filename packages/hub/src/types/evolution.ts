/** @app-mcp/hub 的公开类型：工具演进（第 16 项 O4，spec/hub-api.md 3.21）——弃用声明与 schema 变化记录。 */

/** App 工具的弃用声明（原样，spec/protocol.md 3.7）。弃用工具照常列出与调用。 */
export interface ToolDeprecation {
  /** 面向模型：为什么弃用、该怎么做。 */
  message: string
  /** 替代工具（同一 App 中的局部名）。 */
  replacement?: string
  /** 计划移除的日期（`YYYY-MM-DD`），只作提示。 */
  until?: string
}

/** 变化级别：`breaking` 旧调用会出错；`warning` 可能破坏。兼容的变化不记录。 */
export type SchemaChangeLevel = 'breaking' | 'warning'

/** 一条变化。 */
export interface SchemaChange {
  level: SchemaChangeLevel
  /** JSON Pointer 风格的位置（相对工具定义），如 `/inputSchema/properties/to`。 */
  path: string
  /** 面向开发者的说明。 */
  message: string
}

/** 一个工具的一次不兼容变化（`HubStatus.schemaChanges`）。 */
export interface SchemaChangeRecord {
  appId: string
  /** 工具局部名。 */
  tool: string
  /** `changes` 中最高的级别。 */
  level: SchemaChangeLevel
  changes: SchemaChange[]
  /** Hub 收到新声明的时刻（Unix 毫秒）。 */
  at: number
}
