import 'package:app_mcp_go_router/app_mcp_go_router.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';

void main() {
  test('splitGoRouteParams：路径参数与查询参数', () {
    final none = splitGoRouteParams('/orders/:id', null);
    expect(none.path, isEmpty);
    expect(none.query, isEmpty);
    final r = splitGoRouteParams('/orders/:id', {'id': 'o1', 'tab': 2});
    expect(r.path, {'id': 'o1'});
    expect(r.query, {'tab': '2'});
    expect(() => splitGoRouteParams('/x', [1]), throwsArgumentError);
  });

  testWidgets('mcpGoRouterHandler：页面名 → 命名路由，参数分到路径 / 查询，extra 为请求；未知页面失败', (tester) async {
    final seen = <GoRouterState>[];
    Widget page(BuildContext context, GoRouterState state) {
      seen.add(state);
      return const SizedBox();
    }

    final router = GoRouter(routes: [
      GoRoute(path: '/', name: 'home', builder: page, routes: [
        GoRoute(path: 'orders/:id', name: 'orders.detail', builder: page),
      ]),
      GoRoute(path: '/cart', name: 'shop.cart', builder: page),
    ]);
    addTearDown(router.dispose);
    await tester.pumpWidget(WidgetsApp.router(routerConfig: router, color: const Color(0xFF000000)));

    final handler = mcpGoRouterHandler(router, names: {'cart': 'shop.cart'});
    var done = Future.sync(() => handler(const NavigationRequest('orders.detail', '{"id":"o1","tab":2}')));
    await tester.pumpAndSettle();
    await done;
    expect(seen.last.uri.toString(), '/orders/o1?tab=2');
    expect(seen.last.pathParameters, {'id': 'o1'});
    expect((seen.last.extra! as NavigationRequest).page, 'orders.detail');

    done = Future.sync(() => handler(const NavigationRequest('cart', null)));
    await tester.pumpAndSettle();
    await done;
    expect(seen.last.uri.path, '/cart');

    await expectLater(Future.sync(() => handler(const NavigationRequest('nowhere', null))), throwsStateError);
    await expectLater(Future.sync(() => handler(const NavigationRequest('cart', '[1]'))), throwsArgumentError);
  });
}
