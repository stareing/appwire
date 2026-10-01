/**
 * `@app-mcp/web/webmcp`：W3C WebMCP 命令式 API 的兼容层。
 *
 * ```ts
 * import { createAppMcp } from '@app-mcp/web'
 * import { installWebMcp } from '@app-mcp/web/webmcp'
 *
 * const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城' })
 * installWebMcp(appMcp)
 *
 * // 之后，按标准写的代码（含第三方库）注册的工具会进入 app-mcp
 * await document.modelContext.registerTool({ name: 'add_todo', description: '添加待办', execute: ... })
 * ```
 */

export { installWebMcp } from './install'
export { ToolActivatedEvent, ToolCancelEvent, type ToolEventInit } from './events'
export {
  annotationsToRisk,
  fromStandardResult,
  riskToAnnotations,
  standardToToolAnnotations,
  toolToStandardAnnotations,
  toStandardResult,
} from './convert'
export type {
  ModelContext,
  ModelContextExecuteToolOptions,
  ModelContextGetToolOptions,
  ModelContextOptions,
  ModelContextRegisterToolOptions,
  ModelContextTool,
  RegisteredTool,
  ToolAnnotations,
  ToolExecuteOptions,
  ToolRegistration,
  WebMcpMode,
  WebMcpOptions,
  WebMcpUninstall,
} from './types'
