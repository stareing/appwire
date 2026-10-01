// 纯 Dart 示例：注册购物车工具并连接本机 Host。
//
//   APP_MCP_NATIVE_PATH=.../libapp_mcp.so dart run example/cart.dart [ws://127.0.0.1:7717/app]
import 'dart:async';
import 'dart:io';

import 'package:app_mcp/app_mcp.dart';

Future<void> main(List<String> argv) async {
  final client = AppMcp(
    appId: 'dart-shop',
    appName: 'Dart 商店',
    hostUrl: argv.isNotEmpty ? argv.first : null,
    overview: const AppOverview(summary: '命令行购物车示例', locale: 'zh-CN'),
  );
  final cart = <String, int>{};
  late final ResourceHandle cartResource;

  client.tool('cart.add',
      description: '把商品加入购物车',
      inputSchema: const {
        'type': 'object',
        'properties': {
          'id': {'type': 'string'},
          'qty': {'type': 'integer', 'minimum': 1},
        },
        'required': ['id'],
      }, handler: (args, ctx) {
    final id = args['id'] as String;
    cart[id] = (cart[id] ?? 0) + ((args['qty'] as int?) ?? 1);
    cartResource.notifyChanged();
    return ToolResult({'items': cart}, stateHints: const ['cart']);
  });
  client.tool('cart.checkout', description: '结算', risk: Risk.payment, handler: (args, ctx) async {
    if (cart.isEmpty) throw ToolCallError(ErrorKind.toolDisabled, '购物车为空');
    // 模拟耗时操作，并响应取消。
    await Future.any([Future<void>.delayed(const Duration(seconds: 1)), ctx.cancelled]);
    if (ctx.isCancelled) throw ToolCallError(ErrorKind.cancelled, '已取消');
    final count = cart.values.fold<int>(0, (a, b) => a + b);
    cart.clear();
    return {'count': count};
  });
  cartResource = client.resource('cart', description: '购物车内容', read: () => {'items': cart});

  client.states.listen((s) => stderr.writeln('状态：$s'));
  client.onPaired.listen((token) => stderr.writeln('已配对，请持久化 token：$token'));
  client.start();

  await ProcessSignal.sigint.watch().first;
  client.dispose();
}
