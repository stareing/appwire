/**
 * @app-mcp/react：把 React 组件中的业务动作注册为 MCP 工具。
 *
 * - `<AppMcpProvider value={appMcp}>`：提供 AppMcp 实例。
 * - `useTool(name, definition)`：组件挂载时注册工具，卸载时注销。
 * - `useResource(name, { description, read, deps })`：把界面状态暴露为资源。
 * - `<ToolScope name>`：生命周期边界，卸载时注销其下全部工具与资源。
 * - `useHold(active?)`：组件挂载期间阻止自动休眠（`lifecycle.mode` 为 idle / on-demand 时）。
 */
export { AppMcpProvider, useAppMcp, type AppMcpProviderProps } from './context'
export { ToolScope, type ToolScopeProps } from './tool-scope'
export { useTool } from './use-tool'
export { useResource, type UseResourceOptions } from './use-resource'
export { useConnectionState } from './use-connection-state'
export { useHold } from './use-hold'

export type {
  AppMcp,
  ConnectionState,
  HoldHandle,
  LifecycleOptions,
  ResourceDefinition,
  ResourceHandle,
  Registrar,
  Risk,
  Activation,
  Scope,
  ToolContext,
  ToolDefinition,
  LazyToolDefinition,
  ToolHandler,
  ToolHandlerLoader,
  ToolHandle,
  ToolResult,
} from '@app-mcp/web'
