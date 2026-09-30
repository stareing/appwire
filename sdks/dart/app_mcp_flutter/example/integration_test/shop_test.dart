// Linux 桌面集成测试：真实原生库 + fake host（crates/native/examples/fake_host）驱动示例 App。
//
//   export APP_MCP_NATIVE_PATH=$CARGO_TARGET_DIR/debug/libapp_mcp.so
//   export APP_MCP_FAKE_HOST=$CARGO_TARGET_DIR/debug/examples/fake_host
//   flutter test integration_test -d linux
//
// 路径也可用 --dart-define 传入；找不到时跳过。
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app_mcp_flutter/app_mcp_flutter.dart';
import 'package:app_mcp_flutter_example/main.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

String? _path(String name, String fromDefine) {
  final v = fromDefine.isNotEmpty ? fromDefine : Platform.environment[name];
  return v != null && v.isNotEmpty && File(v).existsSync() ? v : null;
}

final nativePath =
    _path('APP_MCP_NATIVE_PATH', const String.fromEnvironment('APP_MCP_NATIVE_PATH'));
final fakeHostPath =
    _path('APP_MCP_FAKE_HOST', const String.fromEnvironment('APP_MCP_FAKE_HOST'));

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('fake host 调用示例 App 的工具，界面随之更新', (tester) async {
    final host = (await tester.runAsync(() => Process.start(fakeHostPath!, [
          '--addr', '127.0.0.1:0',
          '--invoke', 'products.list', '--args', '{}',
          '--invoke', 'cart.add', '--args', '{"id":"apple","qty":2}',
          '--read', 'cart',
          '--timeout-ms', '20000',
        ])))!;
    final lines = <String>[];
    final listening = Completer<String>();
    host.stdout.transform(utf8.decoder).transform(const LineSplitter()).listen((l) {
      if (!listening.isCompleted && l.startsWith('LISTENING ')) {
        listening.complete(l.substring('LISTENING '.length).trim());
      } else {
        lines.add(l);
      }
    });
    host.stderr.transform(utf8.decoder).listen((s) => stderr.write('[fake_host] $s'));
    final addr = (await tester.runAsync(() => listening.future.timeout(const Duration(seconds: 10))))!;

    final client = AppMcp(
      appId: 'flutter-shop',
      appName: 'Flutter 商店',
      hostUrl: 'ws://$addr',
      libraryPath: nativePath,
      overview: const AppOverview(summary: '演示用购物车', locale: 'zh-CN'),
    );
    final logs = <McpLogRecord>[];
    client.logs.listen(logs.add);
    final states = <ConnectionStatus>[];
    client.states.listen((s) => states.add(s.status));
    addTearDown(() {
      host.kill();
      client.dispose();
    });

    // 先挂载界面完成注册，再连接，保证 fake host 看到全部工具。
    await tester.pumpWidget(AppMcpScope(client: client, child: const ShopApp()));
    await tester.pump();
    client.start();

    int? exitCode;
    unawaited(host.exitCode.then((c) => exitCode = c));
    final deadline = DateTime.now().add(const Duration(seconds: 30));
    while (exitCode == null && DateTime.now().isBefore(deadline)) {
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
      await tester.pump();
    }
    expect(exitCode, 0, reason: 'fake host 输出：\n${lines.join('\n')}');

    final json = [for (final l in lines) jsonDecode(l) as Map<String, dynamic>];
    final tools = json.firstWhere((m) => m['type'] == 'tools');
    final names = [for (final t in tools['tools'] as List) t is Map ? t['name'] : t];
    // 购物车为空时 cart.remove / cart.checkout 处于禁用状态，不出现在列表中。
    expect(names, containsAll(['products.list', 'cart.add']));
    expect(names, isNot(contains('cart.checkout')));

    final invokes = json.where((m) => m['type'] == 'invoke').toList();
    expect(invokes[0]['name'], 'products.list');
    expect(jsonEncode(invokes[0]['result']), contains('apple'));
    expect(invokes[1]['name'], 'cart.add');
    expect(((invokes[1]['result'] as Map)['data'] as Map)['total'], 700);
    final read = json.firstWhere((m) => m['type'] == 'read');
    expect(jsonEncode(read['result']), contains('"qty":2'));

    // handler 在主 isolate 上执行并 setState，界面已更新。
    await tester.pumpAndSettle();
    expect(find.text('× 2'), findsOneWidget);
    expect(states, contains(ConnectionStatus.connected));

    // fake host 退出后原生库输出日志（API v2：message 归接收方所有）。
    for (var i = 0; i < 100 && logs.isEmpty; i++) {
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
    }
    expect(logs, isNotEmpty);
  },
      skip: nativePath == null || fakeHostPath == null,
      timeout: const Timeout(Duration(seconds: 90)));
}
