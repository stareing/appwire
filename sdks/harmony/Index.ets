/**
 * @app-mcp/harmony：鸿蒙（HarmonyOS NEXT / OpenHarmony，API 12+）ArkTS SDK。
 *
 * 原生部分是 libapp_mcp_harmony.so（bindings/harmony，Node-API），与 @app-mcp/node 共用同一份绑定源码。
 */

export { AppMcp, NativeClientFactory, mapState } from './src/main/ets/AppMcp';
export { ToolCallError, Failure, toFailure, isErrorKind } from './src/main/ets/Errors';
export {
  HarmonyAppMcp, HilogLogger, WAKE_PARAM, WAKE_URI_PATH, createAppMcp, nativeClientFactory, wakeArgsFromWant,
} from './src/main/ets/Harmony';
export {
  Activation, AppMcpOptions, AppOverview, Audience, BusyHandle, BusyPolicy, CallDedupOptions, CancelReason, ConnectionState, ConnectionStatus, ContentAnnotations,
  EventDefinition,
  ErrorKind, HeartbeatMode, HoldHandle, LifecycleMode, LifecycleOptions, Logger, NavigationHandler, NavigationRequest, Registrar, Residency, ResourceDefinition,
  ResourceHandle, ResultStatus, Risk, Scope, ToolAnnotations, ToolChanges, ToolContext, ToolDefinition, ToolHandle,
  ToolHandler, ToolResult, ToolResultOptions, ToolSurface, UserActionRequiredOptions, Visibility, WakeDescriptor, WakeKind,
  WakeReason,
} from './src/main/ets/Types';
export { HarmonyUiFallback, UiFallbackOptions } from './src/main/ets/uifallback/HarmonyUiFallback';
export {
  ArkUiPlatform, ArkUiWindowDriver, InspectorJson, MCP_DECLARED_PROPERTY,
} from './src/main/ets/uifallback/ArkUiElements';
export { UIContextDriver } from './src/main/ets/uifallback/ArkUiDriver';
export { UiInspector, UiPlatform } from './src/main/ets/uifallback/UiInspector';
export { UiKey } from './src/main/ets/uifallback/UiFallbackInput';
export {
  UiActionResult, UiOutline, UiOutlineItem, UiReadResult,
} from './src/main/ets/uifallback/UiOutlineFormat';
