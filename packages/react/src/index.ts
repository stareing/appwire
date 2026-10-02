/**
 * @app-mcp/react：把 React 组件中的业务动作注册为 MCP 工具。
 *
 * - `<AppMcpProvider value={appMcp}>`：提供 AppMcp 实例。
 * - `useTool(name, definition)`：组件挂载时注册工具，卸载时注销。
 * - `useResource(name, { description, read, deps })`：把界面状态暴露为资源。
 * - `<ToolScope name>`：生命周期边界，卸载时注销其下全部工具与资源。
 * - `useHold(active?)`：组件挂载期间阻止自动休眠（`lifecycle.mode` 为 idle / on-demand 时）。
 * - `<ToolScope anchor page surface>`：其下 `view` 工具按锚点可见性启用（spec/protocol.md 3.4）。
 * - `<ToolLayer name>`：对话框 / 抽屉等界面层，打开期间下层 `view` 工具暂停。
 * - `useRouterNavigation({ navigate, pages })`：React Router（或任何路由）的导航适配；`useNavigationHandler` 为通用形式。
 */
export { AppMcpProvider, useAppMcp, type AppMcpProviderProps } from './context'
export { ToolScope, type AnchorProp, type ToolScopeProps } from './tool-scope'
export { ToolLayer, type ToolLayerProps } from './tool-layer'
export { useNavigationHandler, useRouterNavigation, type RouterNavigationOptions } from './navigation'
export { useTool } from './use-tool'
export { useResource, type UseResourceOptions } from './use-resource'
export { useConnectionState } from './use-connection-state'
export { useHold } from './use-hold'

export type {
  AppMcp,
  ConnectionState,
  ConnectionBlockCause,
  ConnectionBlockCode,
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
  AnyToolDefinition,
  ToolHandler,
  ToolHandlerLoader,
  ToolHandle,
  ToolResult,
  // 声明与结果契约（spec/protocol.md 3.2）
  ToolAnnotations,
  ContentAnnotations,
  Audience,
  ResultStatus,
  ToolResultEnvelope,
  InputDefinition,
  OutputDefinition,
  OutputSchema,
  JsonSchema,
  ZodLike,
  // 界面级暴露与导航（spec/protocol.md 3.4）
  ToolSurface,
  ViewVisibility,
  ViewLayer,
  ScopeOptions,
  NavigationHandler,
  NavigationOptions,
  NavigationRequest,
  RouteLike,
  GuardResult,
  // 错误（含 USER_ACTION_REQUIRED，spec/protocol.md 第 4 节）
  ErrorKind,
  UserActionReason,
  UserActionRequiredOptions,
} from '@app-mcp/web'
