// 集成测试：真实原生库（bindings/c）+ fake host（crates/native/examples/fake_host）。
//
// 先构建：
//   cargo build -p app-mcp-c
// fake_host 由测试先用 cargo 构建（与 Python / .NET / C++ 测试一致，避免用到旧的 examples/fake_host）。
// 路径可用环境变量 APP_MCP_NATIVE_PATH、APP_MCP_FAKE_HOST（跳过构建）覆盖；找不到或无法构建时跳过。
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
final String? fakeHostPath = _existing(Platform.environment['APP_MCP_FAKE_HOST']) ?? _buildFakeHost();

/// 构建 fake_host 并返回路径；cargo 不可用或构建失败时返回 null（测试跳过，原因写到 stderr）。
/// @why 只找已有的 examples/fake_host 可能拿到旧版本（cargo test 不刷新它），协议更新后测试莫名失败。
String? _buildFakeHost() {
  final ProcessResult r;
  try {
    r = Process.runSync('cargo', ['build', '-q', '-p', 'app-mcp-native', '--example', 'fake_host'],
        workingDirectory: '${Directory.current.path}/../../..',
        environment: {'CARGO_TARGET_DIR': _targetDir});
  } on ProcessException catch (e) {
    stderr.writeln('无法运行 cargo 构建 fake_host：$e');
    return null;
  }
  if (r.exitCode != 0) {
    stderr.writeln('cargo 构建 fake_host 失败：${r.stderr}');
    return null;
  }
  return _existing('$_targetDir/debug/examples/fake_host${Platform.isWindows ? '.exe' : ''}');
}

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
          ? '找不到 fake_host（cargo build -p app-mcp-native --example fake_host 失败，见 stderr）'
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

  test('工具注解 + outputSchema 到达 Host；结构化结果与普通返回值（回归）', () async {
    final host = await FakeHost.start([
      '--tool-info',
      '--invoke', 'order.submit',
      '--invoke', 'plain',
      '--invoke', 'login',
      '--invoke', 'front',
      '--timeout-ms', '15000',
    ]);
    final client = AppMcp(
      appId: 'dart-result',
      appName: 'Dart Result',
      hostUrl: 'ws://${host.addr}',
      libraryPath: nativePath,
    );
    client.tool('order.submit',
        description: '下单',
        annotations: const ToolAnnotations(idempotentHint: false, openWorldHint: true),
        outputSchema: {
          'type': 'object',
          'properties': {
            'orderId': {'type': 'string'}
          }
        },
        handler: (args, ctx) => const ToolResult({'orderId': 'o1'},
            status: ToolResultStatus.pending,
            stateResource: 'order.state',
            summary: '已提交，等待用户在 App 内付款',
            annotations: ContentAnnotations(priority: 0.5)));
    client.tool('plain', description: '普通', risk: Risk.read, handler: (args, ctx) {
      ctx.progress(1, total: 2, message: '处理中');
      return {'ok': true};
    });
    client.tool('login',
        description: '需登录',
        handler: (args, ctx) =>
            throw UserActionRequiredError('登录已过期', reason: UserActionReason.login, uri: 'shop://login'));
    client.tool('front', description: '需前台', handler: (args, ctx) => throw UserActionRequiredError('请切到前台'));
    client.start();
    try {
      final tools = await host.nextJson();
      expect(tools['type'], 'tools');
      final info = tools['toolInfo'] as Map;
      expect(info['order.submit'], {
        'risk': 'write',
        'annotations': {'idempotentHint': false, 'openWorldHint': true},
        'outputSchema': {
          'type': 'object',
          'properties': {
            'orderId': {'type': 'string'}
          }
        },
      });
      expect(info['plain'], {'risk': 'read'});

      final submit = await host.nextJson();
      expect(submit['result'], {
        'data': {'orderId': 'o1'},
        'status': 'pending',
        'stateResource': 'order.state',
        'summary': '已提交，等待用户在 App 内付款',
        'annotations': {'priority': 0.5},
      });
      final progress = await host.nextJson();
      expect(progress['type'], 'progress');
      expect([progress['progress'], progress['total'], progress['message']], [1.0, 2.0, '处理中']);
      final plain = await host.nextJson();
      expect(plain['result'], {
        'data': {'ok': true}
      });
      // USER_ACTION_REQUIRED（v11）：reason / uri 进入 data；未给时 data 只有 kind
      expect((await host.nextJson())['error'], {
        'code': -32019,
        'message': '登录已过期',
        'data': {'kind': 'USER_ACTION_REQUIRED', 'reason': 'login', 'uri': 'shop://login'},
      });
      expect((await host.nextJson())['error'], {
        'code': -32019,
        'message': '请切到前台',
        'data': {'kind': 'USER_ACTION_REQUIRED'},
      });
      expect(await host.process.exitCode.timeout(const Duration(seconds: 10)), 0);
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

  test('真实原生库：功耗选项与 realtime 资源（摘要随 realtime 变化）', () async {
    String hashWith({required bool realtime}) {
      final c = AppMcp(
        appId: 'dart-power',
        appName: '功耗',
        hostUrl: 'ws://127.0.0.1:9',
        libraryPath: nativePath,
        heartbeat: HeartbeatMode.off,
        lifecycle: const LifecyclePolicy(
            mode: LifecycleMode.onDemand, hostAbsentRetries: 0, mergeWindow: Duration.zero, sleepOnBackground: true),
      );
      c.resource('order.status', description: '订单状态', realtime: realtime, read: () => 1);
      final h = c.toolsHash;
      c.dispose();
      return h;
    }

    // realtime 只在 true 时进入资源声明（spec/lifecycle.md 第 13 节 B3），说明标志确实传到了原生库。
    expect(hashWith(realtime: false), hashWith(realtime: false));
    expect(hashWith(realtime: true), isNot(hashWith(realtime: false)));
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }, skip: nativePath == null ? '找不到原生库' : false);

  test('真实原生库：资源内容标注进入资源声明；调用去重选项被接受（v13）', () async {
    String hashWith(ContentAnnotations? annotations) {
      final c = AppMcp(
        appId: 'dart-v13',
        appName: 'v13',
        hostUrl: 'ws://127.0.0.1:9',
        libraryPath: nativePath,
        callDedup: CallDedupPolicy.off,
      );
      c.resource('order.status', description: '订单状态', annotations: annotations, read: () => 1);
      final h = c.toolsHash;
      c.dispose();
      return h;
    }

    const annotated = ContentAnnotations(audience: [ContentAudience.user], priority: 0.5);
    expect(hashWith(null), hashWith(null));
    expect(hashWith(annotated), isNot(hashWith(null)));
    expect(hashWith(annotated), hashWith(annotated));
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }, skip: nativePath == null ? '找不到原生库' : false);

  test('真实原生库：按名寻址选项（v17）——合法实例名被接受，不合法时 invalidConfig', () async {
    AppMcp create(String? instance) => AppMcp(
          appId: 'dart-named',
          appName: 'named',
          hostUrl: 'ws://127.0.0.1:9',
          libraryPath: nativePath,
          lifecycle: const LifecyclePolicy(mode: LifecycleMode.onDemand, residency: Residency.exitWhenIdle),
          registerName: true,
          nameInstance: instance,
        );
    // 不调用 start：不在系统名字服务登记。
    create(null).dispose();
    create('w2').dispose();
    // 实例名规则只在原生库定义（spec/naming.md 2.1），封装层原样传递。
    for (final bad in ['default', 'W2', '2w', '']) {
      expect(() => create(bad),
          throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.invalidConfig)),
          reason: bad);
    }
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }, skip: nativePath == null ? '找不到原生库' : false);

  test('导航（v14）：回调在本 isolate 上执行；完成 / 拒绝 / 失败到达 Host；view 工具声明 surface / page', () async {
    final host = await FakeHost.start([
      '--tool-info',
      '--navigate', 'cart', '--nav-params', '{"id":7}',
      '--navigate', 'login',
      '--navigate', 'crash',
      '--catalog', '0',
      '--timeout-ms', '15000',
    ]);
    final client = AppMcp(appId: 'dart-nav', appName: 'Dart 导航', hostUrl: 'ws://${host.addr}', libraryPath: nativePath);
    final requests = <NavigationRequest>[];
    client.tool('cart.checkout',
        description: '结算', surface: ToolSurface.view, page: 'cart', handler: (args, ctx) => null);
    client.setNavigationHandler((request) async {
      requests.add(request);
      await Future<void>.delayed(Duration.zero);
      if (request.page == 'login') throw const NavigationDeniedError('需要先登录');
      if (request.page == 'crash') throw StateError('页面崩溃');
    });
    client.start();
    final tools = await host.nextJson();
    expect(tools['type'], 'tools');
    final cart = await host.nextJson();
    expect(cart, containsPair('result', {'ok': true}));
    final login = await host.nextJson();
    expect(login['error']['code'], -31002);
    expect(login['error']['message'], contains('需要先登录'));
    expect(login['error']['data'], {'kind': 'NAVIGATION_DENIED', 'reason': 'app'});
    final crash = await host.nextJson();
    expect(crash['error']['code'], -31001);
    expect(crash['error']['message'], contains('页面崩溃'));
    final catalog = await host.nextJson();
    expect(catalog['tools']['cart.checkout'], allOf(containsPair('surface', 'view'), containsPair('page', 'cart')));
    expect(requests.map((r) => r.page), ['cart', 'login', 'crash']);
    expect(requests.first.params, {'id': 7});
    expect(requests[1].params, isNull);
    await host.process.exitCode;
    client.dispose();
  }, skip: skip, timeout: const Timeout(Duration(seconds: 60)));

  test('真实原生库：surface / page 进入工具声明（摘要变化、update 可清除）；导航回调可设置与清除', () async {
    final c = AppMcp(appId: 'dart-v14', appName: 'v14', hostUrl: 'ws://127.0.0.1:9', libraryPath: nativePath);
    final t = c.tool('cart.checkout', description: '结算', handler: (args, ctx) => null);
    final plain = c.toolsHash;
    t.update(surface: ToolSurface.view, page: 'cart');
    expect(t.spec.surface, ToolSurface.view);
    expect(t.spec.page, 'cart');
    final view = c.toolsHash;
    expect(view, isNot(plain));
    t.update(description: '结算');
    expect(c.toolsHash, view, reason: '未提供的字段保持不变');
    t.update(surface: null, page: null);
    expect(c.toolsHash, plain, reason: '显式 null 清除');
    expect(() => c.tool('bad', description: 'x', page: 'bad page!', handler: (a, x) => null),
        throwsA(isA<AppMcpException>()));
    expect(() => t.update(surface: 'view'), throwsArgumentError);
    c.setNavigationHandler((_) {});
    c.setNavigationHandler(null);
    c.dispose();
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }, skip: nativePath == null ? '找不到原生库' : false);
  test('后台导航（v15）：navigateInBackground 时回调抛 UserActionRequiredError → USER_ACTION_REQUIRED；关闭后核心直接回复；'
      'backgroundTool 进入工具声明', () async {
    final host = await FakeHost.start([
      '--navigate', 'cart',
      '--navigate', 'home',
      '--catalog', '0',
      '--timeout-ms', '15000',
    ]);
    final client = AppMcp(
        appId: 'dart-bg-nav', appName: 'Dart 后台导航', hostUrl: 'ws://${host.addr}', libraryPath: nativePath,
        navigateInBackground: true);
    var calls = 0;
    client.tool('cart.summary', description: '购物车摘要', handler: (args, ctx) => null);
    final view = client.tool('cart.view',
        description: '购物车', surface: ToolSurface.view, page: 'cart', backgroundTool: 'cart.summary',
        handler: (args, ctx) => null);
    client.setNavigationHandler((request) {
      calls++;
      client.setNavigateInBackground(false);
      throw UserActionRequiredError('已发通知，请点开后继续', reason: 'foreground', uri: 'conf://${request.page}');
    });
    client.setVisibility(AppVisibility.hidden, focused: false);
    client.start();
    expect((await host.nextJson())['type'], 'tools');
    final cart = await host.nextJson();
    expect(cart['error'], {
      'code': -32019,
      'message': '已发通知，请点开后继续',
      'data': {'kind': 'USER_ACTION_REQUIRED', 'reason': 'foreground', 'uri': 'conf://cart'},
    });
    final home = await host.nextJson();
    expect(home['error']['code'], -32019);
    expect(home['error']['data'], {'kind': 'USER_ACTION_REQUIRED', 'reason': 'foreground'});
    final catalog = await host.nextJson();
    expect(catalog['tools']['cart.view'], containsPair('backgroundTool', 'cart.summary'));
    expect(calls, 1);
    expect(view.spec.backgroundTool, 'cart.summary');
    await host.process.exitCode;
    client.dispose();
  }, skip: skip, timeout: const Timeout(Duration(seconds: 60)));

  test('真实原生库：backgroundTool 影响摘要、null 清除、名称非法时更新失败', () async {
    final c = AppMcp(appId: 'dart-v15', appName: 'v15', hostUrl: 'ws://127.0.0.1:9', libraryPath: nativePath);
    final t = c.tool('cart.view', description: '购物车', surface: ToolSurface.view, handler: (args, ctx) => null);
    final plain = c.toolsHash;
    t.update(backgroundTool: 'cart.summary');
    expect(c.toolsHash, isNot(plain));
    t.update(backgroundTool: null);
    expect(c.toolsHash, plain);
    expect(() => t.update(backgroundTool: 'bad tool!'), throwsA(isA<AppMcpException>()));
    c.setNavigateInBackground(false);
    c.dispose();
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }, skip: nativePath == null ? '找不到原生库' : false);
}
