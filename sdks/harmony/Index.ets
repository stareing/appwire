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
  Activation, AppMcpOptions, AppOverview, CancelReason, ConnectionState, ConnectionStatus, ErrorKind, HeartbeatMode,
  HoldHandle, LifecycleMode, LifecycleOptions, Logger, Registrar, Residency, ResourceDefinition, ResourceHandle, Risk, Scope,
  ToolChanges, ToolContext, ToolDefinition, ToolHandle, ToolHandler, ToolResult, Visibility, WakeDescriptor,
  WakeKind, WakeReason,
} from './src/main/ets/Types';
