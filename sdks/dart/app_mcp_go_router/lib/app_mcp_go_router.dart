/// AppWire 的 go_router 适配（第 4c 项 E）：把 Host 的导航请求（`app/navigate`，spec/protocol.md 3.4）转为 go_router 跳转。
///
/// ```dart
/// final router = GoRouter(observers: [mcpRoutes], routes: [
///   GoRoute(path: '/cart', name: 'cart', builder: (c, s) => const CartPage()),
///   GoRoute(path: '/orders/:id', name: 'orders.detail', builder: (c, s) => OrderPage(id: s.pathParameters['id']!)),
/// ]);
/// client.setNavigationHandler(mcpGoRouterHandler(router));
/// ```
///
/// 页面名默认即 [GoRoute.name]（与清单 `pages[].name`、工具的 `page` 同一命名空间），也可用 `names` 改映射。
/// 导航参数（JSON 对象）中出现在路由路径（`:id`）里的键作为路径参数，其余作为查询参数；完整请求另经 `extra` 传给页面。
library;

import 'package:app_mcp_flutter/app_mcp_flutter.dart';
import 'package:go_router/go_router.dart';

export 'package:app_mcp_flutter/app_mcp_flutter.dart';

/// 返回 [AppMcp.setNavigationHandler] 用的回调。
///
/// - [names]：页面名 → GoRoute 名；为 null 或其中没有该页面时，页面名即路由名。
/// - [push]：true 时 `pushNamed`（保留返回栈），默认 `goNamed`（按路由树重建栈）。
/// - 找不到路由、参数不是对象时以 `NAVIGATION_FAILED` 失败；跳转后等下一帧（新页面的工具已注册）再完成。
NavigationHandler mcpGoRouterHandler(GoRouter router, {Map<String, String>? names, bool push = false}) {
  return (request) async {
    final name = names?[request.page] ?? request.page;
    final route = findGoRouteByName(router.configuration.routes, name);
    if (route == null) throw StateError('没有页面「${request.page}」（go_router 中没有名为 $name 的路由）');
    final (:path, :query) = splitGoRouteParams(route.path, request.params);
    if (push) {
      router.pushNamed<Object?>(name, pathParameters: path, queryParameters: query, extra: request);
    } else {
      router.goNamed(name, pathParameters: path, queryParameters: query, extra: request);
    }
    await mcpNextFrame();
  };
}

/// 在路由树中按名称查找 [GoRoute]（深度优先）。
GoRoute? findGoRouteByName(List<RouteBase> routes, String name) {
  for (final r in routes) {
    if (r is GoRoute && r.name == name) return r;
    final child = findGoRouteByName(r.routes, name);
    if (child != null) return child;
  }
  return null;
}

final RegExp _pathParam = RegExp(r':(\w+)');

/// 导航参数 → (路径参数, 查询参数)：键出现在 [routePath] 的 `:key` 中的进路径，其余进查询；值按 JSON 标量转字符串。
///
/// @error [params] 不是 null 或对象时抛 [ArgumentError]（导航以 `NAVIGATION_FAILED` 失败）。
({Map<String, String> path, Map<String, String> query}) splitGoRouteParams(String routePath, Object? params) {
  if (params == null) return (path: const {}, query: const {});
  if (params is! Map) throw ArgumentError.value(params, 'params', '导航参数应为 JSON 对象');
  final names = {for (final m in _pathParam.allMatches(routePath)) m.group(1)!};
  final path = <String, String>{};
  final query = <String, String>{};
  for (final e in params.entries) {
    final key = '${e.key}';
    (names.contains(key) ? path : query)[key] = '${e.value}';
  }
  return (path: path, query: query);
}
