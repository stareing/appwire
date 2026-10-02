/// app-mcp 的 Flutter 适配。
///
/// - [AppMcpScope]：向子树提供 [AppMcp] 客户端，并按 `AppLifecycleState` 上报可见性；
///   `idle` / `onDemand` 模式下回到前台时回连。
/// - [AppMcpWakeChannel]：把平台通道 / 链接中的唤醒参数交给 [AppMcp.handleWake]。
/// - [McpToolGroup]：为子树创建一个作用域，卸载时注销其下全部工具与资源。
/// - [McpTool] / [McpResource]：挂载时注册、卸载时注销；重建时只刷新 handler，不重新注册。
/// - [McpToolsMixin]：在 `State.build` 中以 `useMcpTool(...)` 的方式注册工具。
/// - [McpViewGate] / [McpRouteGate]（[RouteAware]，配合 [McpRouteObserver]）：`surface: ToolSurface.view` 的工具只在
///   所在界面可见且处于最上层时启用；没有门控时按所在路由是否为栈顶判定。
/// - [mcpNavigatorHandler] / [mcpLocationHandler]：[AppMcp.setNavigationHandler] 的 Navigator / 位置路由适配
///   （go_router 见 `app_mcp_go_router` 包）。
/// - [McpUiFallback] / [McpDeclared]：进程内控件兜底（`ui.outline` / `click` / `fill` 等，spec/ui-fallback.md），默认关闭。
library;

export 'package:app_mcp/app_mcp.dart';

export 'src/lifecycle.dart';
export 'src/scope.dart';
export 'src/tool.dart';
export 'src/ui_fallback/fallback.dart' show McpDeclared, McpUiFallback;
export 'src/ui_fallback/format.dart' show UiOutline;
export 'src/ui_fallback/inspector.dart' show McpUiInspector;
export 'src/view.dart';
export 'src/wake.dart';
