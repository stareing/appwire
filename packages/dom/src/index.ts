/**
 * @app-mcp/dom：只写 HTML 属性（data-mcp-*，或 W3C WebMCP 声明式表单属性）即可把按钮、表单、列表、
 * 状态区域声明为 MCP 工具与资源，并提供只含已声明工具与资源的精简页面快照。
 *
 * ```ts
 * import { createAppMcp } from '@app-mcp/web'
 * import { attachDom } from '@app-mcp/dom'
 * const detach = attachDom(createAppMcp({ appId: 'notes', appName: '便签' }))
 * ```
 */

export { attachDom, DEFAULT_SNAPSHOT_NAME } from './attach'
export type { AttachDomOptions, DetachDom } from './attach'
export { ATTR, WEBMCP, SELECTOR } from './attrs'
export { ERROR_EVENT } from './invoke'
export type { AgentSubmitEvent } from './invoke'
