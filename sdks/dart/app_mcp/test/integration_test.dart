// 集成测试：真实原生库（bindings/c）+ fake host（crates/native/examples/fake_host）。
//
// 先构建：
//   cargo build -p app-mcp-c
//   cargo build -p app-mcp-native --example fake_host
// 路径可用环境变量 APP_MCP_NATIVE_PATH、APP_MCP_FAKE_HOST 覆盖；找不到时跳过。
@Tags(['integration'])
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app_mcp/app_mcp.dart';
import 'package:test/test.dart';

String get _targetDir =>
    Platform.environment['CARGO_TARGET_DIR'] ??
    '${Directory.current.path}/../../../target';

String? _existing(String? path) => path != null && File(path).existsSync() ? path : null;

final String? nativePath = _existing(Platform.environment['APP_MCP_NATIVE_PATH']) ??
    _existing('$_targetDir/debug/${defaultNativeLibraryName()}');
final String? fakeHostPath = _existing(Platform.environment['APP_MCP_FAKE_HOST']) ??
    _existing('$_targetDir/debug/examples/fake_host${Platform.isWindows ? '.exe' : ''}');

/// 运行中的 fake host。
final class FakeHost {
  FakeHost._(this.process, this.addr, this.lines);

  final Process process;
  final String addr;
  final StreamQueue lines;

  /// [listen] 为监听参数：默认 TCP 随机端口；`['--ipc', <端点>]` 时监听本地 IPC，[addr] 为端点字符串本身。
  static Future<FakeHost> start(List<String> args,
      {List<String> listen = const ['--addr', '127.0.0.1:0']}) async {
    final p = await Process.start(fakeHostPath!, [...listen, ...args]);
    p.stderr.transform(utf8.decoder).listen((s) => stderr.write('[fake_host] $s'));
    final queue = StreamQueue(p.stdout.transform(utf8.decoder).transform(const LineSplitter()));
    final first = await queue.next.timeout(const Duration(seconds: 10));
    expect(first, startsWith('LISTENING '));
    return FakeHost._(p, first.substring('LISTENING '.length).trim(), queue);
  }

  Future<Map<String, dynamic>> nextJson() async =>
      jsonDecode(await lines.next.timeout(const Duration(seconds: 15))) as Map<String, dynamic>;
}

/// 极简的行队列（避免依赖 package:async）。
final class StreamQueue {
  StreamQueue(Stream<String> stream) {
    stream.listen((l) {
      if (_waiters.isNotEmpty) {
        _waiters.removeAt(0).complete(l);
      } else {
        _buffer.add(l);
      }
    }, onDone: () {
      for (final w in _waiters) {
        w.completeError(StateError('fake host 输出已结束'));
      }
      _waiters.clear();
      _done = true;
    });
  }

  final List<String> _buffer = [];
  final List<Completer<String>> _waiters = [];
  bool _done = false;

  Future<String> get next {
    if (_buffer.isNotEmpty) return Future.value(_buffer.removeAt(0));
    if (_done) return Future.error(StateError('fake host 输出已结束'));
    final c = Completer<String>();
    _waiters.add(c);
    return c.future;
  }
}

void main() {
  final skip = nativePath == null
      ? '找不到原生库（先 cargo build -p app-mcp-c）'
      : fakeHostPath == null
          ? '找不到 fake_host（先 cargo build -p app-mcp-native --example fake_host）'
          : false;

  test('注册工具与资源，由 fake host 调用', () async {
    final host = await FakeHost.start([
      '--invoke', 'cart.add', '--args', '{"id":"apple","qty":2}',
      '--invoke', 'cart.checkout', '--args', '{}',
      '--invoke', 'cart.fail',
      '--invoke', 'cart.cid',
      '--read', 'cart',
      '--timeout-ms', '15000',
    ]);
    final cart = <String, int>{};
    final client = AppMcp(
      appId: 'dart-shop',
      appName: 'Dart 商店',
      hostUrl: 'ws://${host.addr}',
      libraryPath: nativePath,
      overview: const AppOverview(summary: 'Dart 演示商店', body: '## 典型流程\n加入购物车后结算。', locale: 'zh-CN'),
    );
    final states = <McpConnectionState>[];
    client.states.listen(states.add);
    final logs = <McpLogRecord>[];
    client.logs.listen(logs.add);
    late ResourceHandle cartRes;
    client.tool('cart.add',
        description: '加入购物车',
        inputSchema: {
          'type': 'object',
          'properties': {
            'id': {'type': 'string'},
            'qty': {'type': 'integer'}
          },
          'required': ['id', 'qty'],
        },
        risk: Risk.write, handler: (args, ctx) async {
      await Future<void>.delayed(const Duration(milliseconds: 10));
      cart.update(args['id'] as String, (v) => v + (args['qty'] as int),
          ifAbsent: () => args['qty'] as int);
      cartRes.notifyChanged();
      return ToolResult({'count': cart.length}, stateHints: ['cart']);
    });
    client.tool('cart.checkout',
        description: '结算',
        risk: Risk.payment,
        handler: (args, ctx) => cart.isEmpty
            ? throw ToolCallError(ErrorKind.toolDisabled, '购物车为空')
            : {'total': cart.values.fold<int>(0, (a, b) => a + b)});
    client.tool('cart.fail', description: '总是失败', handler: (args, ctx) => throw StateError('boom'));
    // 连接期间（handler 内）可查 Host 分配的连接 ID（fake_host 返回 "fake-<pid>"，spec/protocol.md 10.3）。
    String? cidInCall;
    client.tool('cart.cid', description: '连接 ID', handler: (args, ctx) {
      cidInCall = client.connectionId;
      return cidInCall;
    });
    cartRes = client.resource('cart', description: '购物车内容', read: () => {'items': cart});
    client.start();

    try {
      final tools = await host.nextJson();
      expect(tools['type'], 'tools');
      final names = [for (final t in tools['tools'] as List) t is Map ? t['name'] : t];
      expect(names, containsAll(['cart.add', 'cart.checkout', 'cart.fail']));

      final add = await host.nextJson();
      expect(add['type'], 'invoke');
      expect(add['name'], 'cart.add');
      expect((add['result'] as Map)['data'], {'count': 1});
      expect((add['result'] as Map)['stateHints'], ['cart']);

      final checkout = await host.nextJson();
      expect((checkout['result'] as Map)['data'], {'total': 2});

      final fail = await host.nextJson();
      expect(fail['error'], isNotNull);
      expect(jsonEncode(fail['error']), contains('HANDLER_ERROR'));

      final cid = await host.nextJson();
      expect(cid['name'], 'cart.cid');
      expect(cidInCall, startsWith('fake-'));

      final read = await host.nextJson();
      expect(read['type'], 'read');
      expect(jsonEncode(read['result']), contains('apple'));

      expect(await host.process.exitCode.timeout(const Duration(seconds: 10)), 0);
      expect(states.map((s) => s.status), contains(ConnectionStatus.connected));
      // fake host 退出后连接关闭，原生库输出一条日志（v2：message 归接收方所有）。
      final deadline = DateTime.now().add(const Duration(seconds: 10));
      while (logs.isEmpty && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(logs, isNotEmpty);
      expect(logs.first.message, isNotEmpty);
    } finally {
      host.process.kill();
      client.dispose();
    }
  }, skip: skip, timeout: const Timeout(Duration(seconds: 60)));

  test('经本地 IPC 正向连接（Windows 每进程命名管道 / 临时目录 Unix 套接字）：连接、connectionId、调用', () async {
    // @why 不用平台默认端点：常驻 Host 可能正在用。
    final Directory? dir =
        Platform.isWindows ? null : Directory.systemTemp.createTempSync('app_mcp_dart_ipc_');
    final endpoint = Platform.isWindows
        ? 'pipe:\\\\.\\pipe\\app-mcp-dart-test-$pid'
        : 'unix:${dir!.path}/h.sock';
    final host = await FakeHost.start(['--invoke', 'greet', '--timeout-ms', '15000'],
        listen: ['--ipc', endpoint]);
    expect(host.addr, endpoint);
    final client = AppMcp(
      appId: 'dart-ipc',
      appName: 'Dart IPC',
      hostUrl: endpoint,
      libraryPath: nativePath,
      connectTimeout: const Duration(seconds: 5),
    );
    final states = <ConnectionStatus>[];
    client.states.listen((s) => states.add(s.status));
    String? cidInCall;
    client.tool('greet', description: '问候', handler: (args, ctx) {
      cidInCall = client.connectionId;
      return 'Hello over IPC!';
    });
    client.start();
    try {
      final tools = await host.nextJson();
      expect(tools['type'], 'tools');
      expect(tools['tools'], contains('greet'));

      final greet = await host.nextJson();
      expect(greet['type'], 'invoke');
      expect(greet['name'], 'greet');
      expect((greet['result'] as Map)['data'], 'Hello over IPC!');
      expect(cidInCall, isNotNull);
      expect(cidInCall, isNotEmpty);
      expect(cidInCall, startsWith('fake-'));

      expect(await host.process.exitCode.timeout(const Duration(seconds: 10)), 0);
      expect(states, contains(ConnectionStatus.connected));
    } finally {
      host.process.kill();
      client.dispose();
      dir?.deleteSync(recursive: true);
    }
  }, skip: skip, timeout: const Timeout(Duration(seconds: 60)));

  test('生命周期往返：idle 休眠 → handleWake 快速恢复（跳过 sync）→ 调用 → 再休眠；details 到达 Host',
      () async {
    final host = await FakeHost.start([
      '--invoke', 'pay',
      '--await-sleep', '--wake',
      '--invoke', 'greet', '--args', '{"name":"World"}',
      '--await-sleep',
      '--timeout-ms', '20000',
    ]);
    final client = AppMcp(
      appId: 'dart-life',
      appName: 'Dart 生命周期',
      hostUrl: 'ws://${host.addr}',
      libraryPath: nativePath,
      lifecycle: const LifecyclePolicy(
        mode: LifecycleMode.idle,
        idleTimeout: Duration(milliseconds: 300),
        wake: WakeDescriptor.uri('dartlife'),
      ),
      connectTimeout: const Duration(seconds: 2),
    );
    final states = <ConnectionStatus>[];
    client.states.listen((s) => states.add(s.status));
    client.tool('pay',
        description: '支付',
        handler: (args, ctx) =>
            throw ToolCallError(ErrorKind.userRejected, '余额不足', details: {'balance': 3, 'need': 10}));
    client.tool('greet',
        description: '问候',
        inputSchema: {
          'type': 'object',
          'properties': {
            'name': {'type': 'string'}
          },
          'required': ['name'],
        },
        handler: (args, ctx) => 'Hello, ${args['name']}!');
    final hashBefore = client.toolsHash;
    client.start();
    try {
      final tools = await host.nextJson();
      expect(tools['type'], 'tools');
      expect(tools['tools'], containsAll(['pay', 'greet']));

      final pay = await host.nextJson();
      expect(pay['type'], 'invoke');
      final error = pay['error'] as Map<String, dynamic>;
      expect(jsonEncode(error), contains('USER_REJECTED'));
      final data = error['data'] as Map<String, dynamic>;
      expect(data['balance'], 3);
      expect(data['need'], 10);

      final sleep1 = await host.nextJson();
      expect(sleep1, containsPair('type', 'sleep'));
      expect(sleep1['accepted'], isTrue);
      expect(sleep1['reason'], 'idle');
      expect(sleep1['toolsHash'], hashBefore);

      final wake = await host.nextJson();
      expect(wake['type'], 'wake');
      // 等 SDK 真正进入休眠再唤醒（模拟 OS 激活参数到达）。
      final deadline = DateTime.now().add(const Duration(seconds: 5));
      while (client.state.status != ConnectionStatus.dormant && DateTime.now().isBefore(deadline)) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
      expect(client.state.status, ConnectionStatus.dormant);
      expect(client.handleWake('--unrelated'), isFalse);
      expect(client.handleWake(wake['arg'] as String), isTrue);

      final hello = await host.nextJson();
      expect(hello['type'], 'hello');
      expect(hello['launchToken'], wake['token']);
      expect(hello['resumeToken'], isNotNull);
      expect(hello['wakeReason'], 'os-activation');
      expect(hello['toolsCurrent'], isTrue);

      final tools2 = await host.nextJson();
      expect(tools2['type'], 'tools');
      expect(tools2['synced'], isFalse, reason: 'toolsCurrent 时应跳过 tools/sync');
      expect(tools2['tools'], containsAll(['pay', 'greet']));

      final greet = await host.nextJson();
      expect(greet['name'], 'greet');
      expect((greet['result'] as Map)['data'], 'Hello, World!');

      final sleep2 = await host.nextJson();
      expect(sleep2['type'], 'sleep');
      expect(sleep2['accepted'], isTrue);

      expect(await host.process.exitCode.timeout(const Duration(seconds: 10)), 0);
      final deadline2 = DateTime.now().add(const Duration(seconds: 5));
      while (client.state.status != ConnectionStatus.dormant && DateTime.now().isBefore(deadline2)) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
      expect(client.state.status, ConnectionStatus.dormant);
      expect(states, containsAll([ConnectionStatus.dormant, ConnectionStatus.connected]));
    } finally {
      host.process.kill();
      client.dispose();
    }
  }, skip: skip, timeout: const Timeout(Duration(seconds: 60)));

  test('真实原生库：连不上的端点 → backoff 带 code（HOST_NOT_RUNNING），connectionId 为 null', () async {
    // @why 不用 ws://127.0.0.1:1：WSL 等环境下回环连接未监听端口可能超时（CONNECT_TIMEOUT）而非被拒绝。
    final client = AppMcp(
      appId: 'dart-diag',
      appName: 'Dart 诊断',
      hostUrl: Platform.isWindows
          ? r'pipe:\\.\pipe\app-mcp-dart-test-missing'
          : 'unix:/nonexistent-app-mcp-dart-test/hub.sock',
      libraryPath: nativePath,
    );
    try {
      expect(client.state.code, isNull);
      expect(client.connectionId, isNull);
      final backoff = client.states.firstWhere((s) => s.status == ConnectionStatus.backoff);
      client.start();
      final s = await backoff.timeout(const Duration(seconds: 10));
      expect(s.code, 'HOST_NOT_RUNNING');
      expect(s.reason, isNotEmpty);
      final now = client.state;
      if (now.status == ConnectionStatus.backoff) expect(now.code, 'HOST_NOT_RUNNING');
      expect(client.connectionId, isNull);
      client.stop();
      expect(client.state.code, isNull);
    } finally {
      client.dispose();
    }
    expect(() => client.connectionId, throwsA(isA<AppMcpException>()));
  }, skip: nativePath == null ? '找不到原生库' : false);

  test('真实原生库：注册、作用域、错误码与释放（不连接）', () async {
    final client = AppMcp(
      appId: 'dart-life',
      appName: '生命周期',
      hostUrl: 'ws://127.0.0.1:9',
      libraryPath: nativePath,
      overview: const AppOverview(summary: '测试', locale: 'zh-CN'),
    );
    expect(AppMcp.nativeVersion(libraryPath: nativePath), isNotEmpty);
    expect(client.instanceId, isNotEmpty);
    expect(client.state.status, ConnectionStatus.idle);
    final t = client.tool('a', description: 'a', handler: (a, c) => 1);
    expect(() => client.tool('a', description: 'a', handler: (a, c) => 1),
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.duplicateName)));
    expect(() => client.tool('bad name', description: 'x', handler: (a, c) => 1),
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.invalidName)));
    expect(
        () => client.tool('s', description: 's', inputSchema: {'type': 'array'}, handler: (a, c) => 1),
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.invalidSchema)));
    t.update(description: 'a2', risk: Risk.read);
    t.setEnabled(false);
    t.dispose();
    client.tool('a', description: 'again', handler: (a, c) => 2);
    final s = client.scope('page');
    s.tool('p.x', description: 'x', handler: (a, c) => 3);
    s.resource('p.r', description: 'r', read: () => 1).notifyChanged();
    s.dispose();
    client.setVisibility(AppVisibility.hidden, focused: false);
    // v3：生命周期 API 在未连接时也可调用。
    expect(client.toolsHash, hasLength(16));
    expect(client.handleWake('not-a-wake'), isFalse);
    final h = client.hold();
    h.release();
    expect(AppMcp.parseWakeToken('shop://app-mcp/wake?token=abc', libraryPath: nativePath), 'abc');
    client.stop();
    expect(() => client.tool('late', description: 'x', handler: (a, c) => 1),
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.stopped)));
    client.dispose();
    client.dispose();
    // 让库线程上的 free_user_data 回来。
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }, skip: nativePath == null ? '找不到原生库' : false);
}
