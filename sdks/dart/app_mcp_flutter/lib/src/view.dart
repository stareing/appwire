// 界面级暴露（spec/protocol.md 3.4、第 4c 项 E）：view 工具只在所在界面"可见且处于最上层"时启用。
//
// 判定（取 AND）：
// - 所在路由是导航栈顶：`ModalRoute.isCurrentOf(context)`（被新页面 / 对话框盖住时为 false；不在任何路由中时视为 true）；
// - 祖先 [McpViewGate] / [McpRouteGate] 给出的状态（嵌套时逐层 AND；没有时为 true）。
// keep-alive 的标签页（IndexedStack、TabBarView 等）不改变路由，需要用 `McpViewGate(active: index == current)` 显式标出。

import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/widgets.dart';

/// 显式标出子树中 view 工具（[ToolSurface.view]）是否启用：[active] 为 false 时子树里的 view 工具禁用
/// （不同步给 Host，Host 收到 `tools/changed`）。嵌套时与祖先门控取 AND。
///
/// ```dart
/// IndexedStack(index: tab, children: [
///   McpViewGate(active: tab == 0, child: const CartPage()),
///   McpViewGate(active: tab == 1, child: const OrdersPage()),
/// ])
/// ```
class McpViewGate extends StatelessWidget {
  const McpViewGate({super.key, required this.active, required this.child});

  final bool active;
  final Widget child;

  /// 子树中 view 工具当前是否应启用（见文件头的判定）。会让 [context] 依赖门控与路由状态。
  static bool isActive(BuildContext context) {
    final gate = context.dependOnInheritedWidgetOfExactType<_McpViewInherited>();
    return (gate?.active ?? true) && (ModalRoute.isCurrentOf(context) ?? true);
  }

  @override
  Widget build(BuildContext context) {
    final parent = context.dependOnInheritedWidgetOfExactType<_McpViewInherited>();
    return _McpViewInherited(active: active && (parent?.active ?? true), child: child);
  }
}

class _McpViewInherited extends InheritedWidget {
  const _McpViewInherited({required this.active, required super.child});

  final bool active;

  @override
  bool updateShouldNotify(_McpViewInherited oldWidget) => oldWidget.active != active;
}

/// 供 [McpRouteGate] 订阅的路由观察者：放进 `MaterialApp.navigatorObservers`（或 go_router 的 `observers`）。
///
/// ```dart
/// final mcpRoutes = McpRouteObserver();
/// MaterialApp(navigatorObservers: [mcpRoutes], ...);
/// ```
class McpRouteObserver extends RouteObserver<ModalRoute<Object?>> {}

/// [RouteAware] 门控：所在路由是栈顶时启用子树中的 view 工具；推入新页面 / 对话框（[didPushNext]）时禁用，
/// 返回（[didPopNext]）时恢复，路由出栈（[didPop]）时禁用。
///
/// 与 [McpViewGate.isActive] 内置的 `ModalRoute.isCurrentOf` 判定一致，另外由 [observer] 的导航事件驱动并经 [onChanged]
/// 通知 App（如暂停轮询、保存草稿）；嵌套 Navigator 时 [observer] 决定看哪一层的导航。
class McpRouteGate extends StatefulWidget {
  const McpRouteGate({super.key, required this.observer, required this.child, this.onChanged});

  final McpRouteObserver observer;
  final Widget child;

  /// 启用状态变化时调用（参数为新状态）。
  final ValueChanged<bool>? onChanged;

  @override
  State<McpRouteGate> createState() => _McpRouteGateState();
}

class _McpRouteGateState extends State<McpRouteGate> with RouteAware {
  ModalRoute<Object?>? _route;
  bool _active = true;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final route = ModalRoute.of(context);
    if (identical(route, _route)) return;
    widget.observer.unsubscribe(this);
    _route = route;
    if (route != null) {
      widget.observer.subscribe(this, route);
      _active = route.isCurrent;
    }
  }

  @override
  void didUpdateWidget(McpRouteGate oldWidget) {
    super.didUpdateWidget(oldWidget);
    final route = _route;
    if (!identical(oldWidget.observer, widget.observer) && route != null) {
      oldWidget.observer.unsubscribe(this);
      widget.observer.subscribe(this, route);
    }
  }

  void _set(bool active) {
    if (!mounted || active == _active) return;
    setState(() => _active = active);
    widget.onChanged?.call(active);
  }

  @override
  void didPush() => _set(true);
  @override
  void didPopNext() => _set(true);
  @override
  void didPushNext() => _set(false);
  @override
  void didPop() => _set(false);

  @override
  void dispose() {
    widget.observer.unsubscribe(this);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => McpViewGate(active: _active, child: widget.child);
}

/// 导航回调（[AppMcp.setNavigationHandler]）的 [Navigator] 适配：页面名 → 命名路由（`pushNamed`），
/// 参数为 [NavigationRequest.params]。未列出的页面以 `NAVIGATION_FAILED` 失败。
///
/// 返回的 Future 在下一帧之后完成：新页面的 [McpTool] 在首帧挂载时注册，Host 随后即可调用。
///
/// ```dart
/// final navigatorKey = GlobalKey<NavigatorState>();
/// client.setNavigationHandler(mcpNavigatorHandler(navigatorKey, routes: {'cart': '/cart', 'orders.detail': '/order'}));
/// ```
NavigationHandler mcpNavigatorHandler(GlobalKey<NavigatorState> navigatorKey,
    {required Map<String, String> routes, bool replace = false}) {
  return (request) async {
    final route = routes[request.page];
    if (route == null) throw StateError('没有页面「${request.page}」');
    final navigator = navigatorKey.currentState;
    if (navigator == null) throw StateError('Navigator 尚未挂载');
    if (replace) {
      navigator.pushReplacementNamed(route, arguments: request.params);
    } else {
      navigator.pushNamed(route, arguments: request.params);
    }
    await mcpNextFrame();
  };
}

/// 通用路由适配（go_router、auto_route、Router 2.0 等以"位置"导航的路由器）：[location] 把请求转为位置字符串
/// （返回 null = 没有该页面，以 `NAVIGATION_FAILED` 失败），[go] 执行跳转。go_router 见 `app_mcp_go_router` 包。
///
/// ```dart
/// client.setNavigationHandler(mcpLocationHandler(
///   go: router.go,
///   location: (r) => switch (r.page) { 'cart' => '/cart', _ => null },
/// ));
/// ```
NavigationHandler mcpLocationHandler(
    {required void Function(String location) go, required String? Function(NavigationRequest request) location}) {
  return (request) async {
    final target = location(request);
    if (target == null) throw StateError('没有页面「${request.page}」');
    go(target);
    await mcpNextFrame();
  };
}

/// 等到下一帧绘制完成（新页面的 widget 已挂载、其中的工具已注册）。widget 测试中需 `pump` 才会完成。
Future<void> mcpNextFrame() {
  final binding = WidgetsBinding.instance;
  binding.scheduleFrame();
  return binding.endOfFrame;
}
