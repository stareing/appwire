/// app-mcp 的 Dart SDK：把 App 内的业务动作以 MCP 工具暴露给模型。
///
/// 通过 `dart:ffi` 调用原生库 `app_mcp`（C ABI，见 bindings/c/include/app_mcp.h）。
/// 所有回调都会回到创建 [AppMcp] 的 isolate 上执行。
library;

export 'src/bindings.dart' show AppMcpBindings, openNativeLibrary, defaultNativeLibraryName;
export 'src/client.dart' show AppMcp, McpScope, ToolHandle, ResourceHandle;
export 'src/types.dart';
