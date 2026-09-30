/// app-mcp 的 Flutter 适配。
///
/// - [AppMcpScope]：向子树提供 [AppMcp] 客户端，并按 `AppLifecycleState` 上报可见性；
///   `idle` / `onDemand` 模式下回到前台时回连。
/// - [AppMcpWakeChannel]：把平台通道 / 链接中的唤醒参数交给 [AppMcp.handleWake]。
/// - [McpToolGroup]：为子树创建一个作用域，卸载时注销其下全部工具与资源。
/// - [McpTool] / [McpResource]：挂载时注册、卸载时注销；重建时只刷新 handler，不重新注册。
/// - [McpToolsMixin]：在 `State.build` 中以 `useMcpTool(...)` 的方式注册工具。
library;

export 'package:app_mcp/app_mcp.dart';

export 'src/lifecycle.dart';
export 'src/scope.dart';
export 'src/tool.dart';
export 'src/wake.dart';
