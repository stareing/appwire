// 用假原生库（test/fake_native/fake_app_mcp.c）测试 FFI 封装：
// 回调来自其他线程、临时字符串在回调返回后失效、free_user_data 在任意线程调用。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'package:app_mcp/app_mcp.dart';
import 'package:app_mcp/src/bindings.dart';
import 'package:test/test.dart';

import 'support/fake_native.dart';

void main() {
  final path = buildFakeLibrary();
  if (path == null) {
    test('假原生库', () {}, skip: '没有 C 编译器，跳过');
    return;
  }
  final fake = FakeNative(path);
  late AppMcp client;

  setUp(() {
    client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
  });
  tearDown(() => client.dispose());

  Future<Map<String, dynamic>> invoke(String tool, Object? args) async {
    final idx = fake.invoke(tool, jsonEncode(args));
    expect(idx, greaterThanOrEqualTo(0), reason: '工具 $tool 未注册');
    return jsonDecode(await fake.waitResult(idx)) as Map<String, dynamic>;
  }

  test('结构体布局与 C 一致', () {
    expect(sizeOf<AmClientConfig>(), fake.sizeOf(0));
    expect(sizeOf<AmClientCallbacks>(), fake.sizeOf(1));
    expect(sizeOf<AmToolSpec>(), fake.sizeOf(2));
    expect(sizeOf<AmResourceSpec>(), fake.sizeOf(3));
    expect(sizeOf<AmLifecycle>(), fake.sizeOf(6));
    expect(sizeOf<AmClientOptions>(), fake.sizeOf(7));
  });

  group('生命周期（v3）', () {
    test('默认策略（桌面）与 connectTimeout 传入 am_client_new_ex', () {
      // 默认：persistent、不上报唤醒描述（-1）、connect_timeout 0（原生默认）。
      expect(fake.lifecycle(), '0|60000|15000|10000|0|-1|-|0|0');
      expect(client.lifecycle.mode, LifecycleMode.persistent);
      client.dispose();
      client = AppMcp(
        appId: 'shop',
        appName: '商店',
        libraryPath: path,
        connectTimeout: const Duration(seconds: 2),
        lifecycle: const LifecyclePolicy(
          mode: LifecycleMode.onDemand,
          idleTimeout: Duration(milliseconds: 300),
          hiddenIdleTimeout: Duration.zero,
          grace: Duration(seconds: 1),
          residency: Residency.exitWhenIdle,
          wake: WakeDescriptor.uri('shop', background: true),
        ),
      );
      expect(fake.lifecycle(), '2|300|0|1000|1|1|shop|1|2000');
      client.dispose();
      client = AppMcp(
          appId: 'shop',
          appName: '商店',
          libraryPath: path,
          lifecycle: const LifecyclePolicy(
              mode: LifecycleMode.idle, wake: WakeDescriptor.androidIntent('com.x/.WakeReceiver')));
      expect(fake.lifecycle(), '1|60000|15000|10000|0|5|com.x/.WakeReceiver|1|0');
    });

    test('handleWake / wake / sleep / connectNow 与状态 dormant、waking', () async {
      final states = <ConnectionStatus>[];
      final sub = client.states.listen((s) => states.add(s.status));
      expect(client.handleWake('--foo'), isFalse);
      expect(client.handleWake('app-mcp-wake:tok'), isTrue);
      expect(client.sleep(), isTrue);
      expect(fake.lastSleep(), '3');
      expect(client.sleep(reason: SleepReason.background), isFalse);
      expect(fake.lastSleep(), '2');
      expect(client.wake(reason: WakeReason.visible), isTrue);
      expect(client.wake(), isFalse);
      expect(client.connectNow(), isTrue);
      await eventually(() => states.length == 5);
      expect(states.toSet(), containsAll([ConnectionStatus.waking, ConnectionStatus.dormant]));
      await sub.cancel();
    });

    test('hold：release 幂等，计数归零', () {
      final before = fake.holdCount();
      final h = client.hold();
      expect(fake.holdCount(), before + 1);
      expect(h.isReleased, isFalse);
      h.release();
      h.release();
      expect(h.isReleased, isTrue);
      expect(fake.holdCount(), before);
    });

    test('toolsHash 与 parseWakeToken', () {
      final h0 = client.toolsHash;
      client.tool('a', description: 'a', handler: (a, c) => 1);
      expect(client.toolsHash, isNot(h0));
      expect(AppMcp.parseWakeToken('app-mcp-wake:abc', libraryPath: path), 'abc');
      expect(AppMcp.parseWakeToken('hello', libraryPath: path), isNull);
    });

    test('onIdleExit 在创建客户端的 isolate 上触发', () async {
      final f = client.onIdleExit.first;
      fake.idleExit();
      await f.timeout(const Duration(seconds: 5));
    });

    test('ToolCallError.details 经 am_call_fail_with_details 上报；无法编码时退回', () async {
      client.tool('pay',
          description: '支付',
          handler: (a, c) => throw ToolCallError(ErrorKind.userRejected, '余额不足',
              details: {'balance': 3, 'need': 10}));
      client.tool('pay2',
          description: '支付',
          handler: (a, c) => throw ToolCallError(ErrorKind.userRejected, '坏详情', details: Object()));
      expect(await invoke('pay', {}), {
        'ok': false,
        'kind': 'USER_REJECTED',
        'message': '余额不足',
        'details': {'balance': 3, 'need': 10}
      });
      expect(await invoke('pay2', {}), {'ok': false, 'kind': 'USER_REJECTED', 'message': '坏详情'});
    });

    test('ctx.hold()：调用完成后仍持有，完成后再 hold 抛出', () async {
      McpHold? held;
      ToolContext? saved;
      client.tool('long', description: '长任务', handler: (a, ctx) {
        held = ctx.hold();
        saved = ctx;
        return 'ok';
      });
      final before = fake.holdCount();
      expect((await invoke('long', {}))['data'], 'ok');
      expect(fake.holdCount(), before + 1);
      held!.release();
      expect(fake.holdCount(), before);
      expect(() => saved!.hold(),
          throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.alreadyCompleted)));
    });
  });

  test('AM_API_VERSION 与头文件一致', () {
    final header = File('../../../bindings/c/include/app_mcp.h').readAsStringSync();
    final m = RegExp(r'#define AM_API_VERSION (\d+)').firstMatch(header);
    expect(m, isNotNull);
    expect(AM_API_VERSION, int.parse(m!.group(1)!));
  });

  test('总览传入原生配置', () {
    expect(fake.overview(), isNull);
    client.dispose();
    client = AppMcp(
        appId: 'shop',
        appName: '商店',
        libraryPath: path,
        overview: const AppOverview(summary: '演示商城', body: '## 典型流程', locale: 'zh-CN'));
    expect(fake.overview(), '演示商城|## 典型流程|zh-CN');
    client.dispose();
    client = AppMcp(
        appId: 'shop', appName: '商店', libraryPath: path, overview: const AppOverview(summary: 's'));
    expect(fake.overview(), 's|-|-');
  });

  test('版本与实例 ID', () {
    expect(AppMcp.nativeVersion(libraryPath: path), '0.0.0-fake');
    expect(client.instanceId, 'fake-instance');
    expect(client.token, isNull);
  });

  test('工具调用：参数解码、结果编码，handler 在创建客户端的 isolate 上执行', () async {
    final isolate = Isolate.current;
    Isolate? seen;
    client.tool('cart.add',
        description: '加入购物车',
        inputSchema: {
          'type': 'object',
          'properties': {
            'id': {'type': 'string'}
          }
        },
        risk: Risk.write, handler: (args, ctx) async {
      seen = Isolate.current;
      expect(ctx.toolName, 'cart.add');
      expect(ctx.callId, startsWith('call-'));
      expect(ctx.isCancelled, isFalse);
      return {'added': args['id'], 'qty': args['qty']};
    });
    final r = await invoke('cart.add', {'id': 'apple', 'qty': 2});
    expect(r, {
      'ok': true,
      'data': {'added': 'apple', 'qty': 2},
      'hints': <Object?>[]
    });
    expect(identical(seen, isolate) || seen?.controlPort == isolate.controlPort, isTrue);
  });

  test('ToolResult 带 stateHints；同步 handler', () async {
    client.tool('cart.clear',
        description: '清空', handler: (args, ctx) => const ToolResult(null, stateHints: ['cart']));
    final r = await invoke('cart.clear', null);
    expect(r['data'], isNull);
    expect(r['hints'], ['cart']);
  });

  test('ToolCallError 映射为错误类别', () async {
    client.tool('cart.checkout',
        description: '结算',
        handler: (args, ctx) => throw ToolCallError(ErrorKind.toolDisabled, 'empty'));
    expect(await invoke('cart.checkout', {}), {'ok': false, 'kind': 'TOOL_DISABLED', 'message': 'empty'});
  });

  test('其他异常与无法编码的返回值为 HANDLER_ERROR', () async {
    client.tool('a', description: 'a', handler: (args, ctx) async => throw StateError('boom'));
    client.tool('b', description: 'b', handler: (args, ctx) => Object());
    final a = await invoke('a', {});
    expect(a['kind'], 'HANDLER_ERROR');
    expect(a['message'], contains('boom'));
    expect((await invoke('b', {}))['kind'], 'HANDLER_ERROR');
  });

  test('非法参数为 INVALID_INPUT', () async {
    client.tool('a', description: 'a', handler: (args, ctx) => 1);
    final idx = fake.invoke('a', '[1,2]');
    final r = jsonDecode(await fake.waitResult(idx)) as Map<String, dynamic>;
    expect(r['kind'], 'INVALID_INPUT');
  });

  test('取消：ctx.cancelled 完成，isCancelled 为真，完成返回的结果被丢弃', () async {
    final started = Completer<ToolContext>();
    client.tool('slow', description: '慢', handler: (args, ctx) async {
      started.complete(ctx);
      final reason = await ctx.cancelled;
      expect(ctx.isCancelled, isTrue);
      expect(ctx.cancelReason, reason);
      return 'late';
    });
    final idx = fake.invoke('slow', '{}');
    final ctx = await started.future;
    expect(fake.cancel(idx, 1), 1);
    expect(await ctx.cancelled, CancelReason.timeout);
    final r = jsonDecode(await fake.waitResult(idx)) as Map<String, dynamic>;
    expect(r['kind'], 'CANCELLED');
  });

  test('重名与非法名称抛出 AppMcpException', () {
    client.tool('x', description: 'x', handler: (a, c) => null);
    expect(() => client.tool('x', description: 'x', handler: (a, c) => null),
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.duplicateName)));
    expect(() => client.tool('bad name', description: 'x', handler: (a, c) => null),
        throwsA(isA<AppMcpException>()
            .having((e) => e.code, 'code', AppMcpErrorCode.invalidName)
            .having((e) => e.message, 'message', '非法名称')));
    expect(
        () => client.tool('y',
            description: 'y', inputSchema: {'type': 'array'}, handler: (a, c) => null),
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.invalidSchema)));
  });

  test('dispose 注销工具，free_user_data 从其他线程回来后清理', () async {
    final before = fake.freeCount();
    final t = client.tool('x', description: 'x', handler: (a, c) => 1);
    t.dispose();
    expect(t.isDisposed, isTrue);
    expect(fake.invoke('x', '{}'), -1);
    await eventually(() => fake.freeCount() == before + 1);
    t.dispose(); // 幂等
    // 同名可以重新注册。
    client.tool('x', description: 'x2', handler: (a, c) => 2);
    expect((await invoke('x', {}))['data'], 2);
  });

  test('setHandler 替换 handler，update / setEnabled 生效', () async {
    final t = client.tool('x', description: 'x', handler: (a, c) => 1);
    t.setHandler((a, c) => 2);
    expect((await invoke('x', {}))['data'], 2);
    t.update(description: '新描述');
    expect(fake.toolDescription('x'), '新描述');
    expect(t.spec.description, '新描述');
    t.setEnabled(false);
    expect(fake.toolEnabled('x'), 0);
    expect(fake.invoke('x', '{}'), -1);
  });

  test('资源读取与 notifyChanged', () async {
    var items = ['apple'];
    final r = client.resource('cart', description: '购物车', read: () async => {'items': items});
    var idx = fake.read('cart');
    expect(jsonDecode(await fake.waitResult(idx)), {
      'ok': true,
      'contents': {'items': ['apple']}
    });
    items = [];
    r.notifyChanged();
    expect(fake.notifyCount(), greaterThan(0));
    client.resource('broken', description: 'x', read: () => throw StateError('no'));
    idx = fake.read('broken');
    expect((jsonDecode(await fake.waitResult(idx)) as Map)['kind'], 'HANDLER_ERROR');
  });

  test('scope dispose 注销其下全部工具', () async {
    final s = client.scope('page');
    final inner = s.scope('dialog');
    final t1 = s.tool('p.a', description: 'a', handler: (a, c) => 1);
    final t2 = inner.tool('p.b', description: 'b', handler: (a, c) => 2);
    expect((await invoke('p.b', {}))['data'], 2);
    s.dispose();
    expect(t1.isDisposed && t2.isDisposed && inner.isDisposed, isTrue);
    expect(fake.invoke('p.a', '{}'), -1);
    expect(fake.invoke('p.b', '{}'), -1);
    expect(() => s.tool('z', description: 'z', handler: (a, c) => 0),
        throwsA(isA<AppMcpException>()));
  });

  test('状态流：rejected 的 reason 取自回调（v2 归接收方所有）并被释放', () async {
    final states = <McpConnectionState>[];
    final sub = client.states.listen(states.add);
    client.start();
    await eventually(() => states.isNotEmpty);
    expect(states.last.status, ConnectionStatus.connected);
    fake.emitState(6, 0, 'token 无效');
    await eventually(() => states.length == 2);
    expect(states.last, const McpConnectionState(ConnectionStatus.rejected, reason: 'token 无效'));
    fake.emitState(5, 2000, null);
    await eventually(() => states.length == 3);
    expect(states.last.retryIn, const Duration(seconds: 2));
    expect(client.state.status, ConnectionStatus.backoff);
    expect(fake.ownedOutstanding(), 0);
    await sub.cancel();
  });

  test('配对：token 取自回调并被释放', () async {
    final tokenF = client.onPaired.first;
    fake.pair('tok-123');
    expect(await tokenF, 'tok-123');
    expect(client.token, 'tok-123');
    expect(fake.ownedOutstanding(), 0);
  });

  test('日志回调：级别映射、消息投递到本 isolate，字符串被释放', () async {
    final logs = <McpLogRecord>[];
    final sub = client.logs.listen(logs.add);
    fake.log(0, 'd');
    fake.log(2, '警告');
    fake.log(3, 'e');
    fake.log(99, '未知级别');
    await eventually(() => logs.length == 4);
    final byMsg = {for (final l in logs) l.message: l.level};
    expect(byMsg, {'d': LogLevel.debug, '警告': LogLevel.warn, 'e': LogLevel.error, '未知级别': LogLevel.info});
    await sub.cancel();
    // 没有监听者时丢弃，但字符串仍须释放。
    final before = fake.ownedTotal();
    fake.log(1, 'dropped');
    await eventually(() => fake.ownedTotal() == before + 1 && fake.ownedOutstanding() == 0);
  });

  test('客户端 dispose 后到达的回调字符串仍被释放', () async {
    fake.log(1, 'late');
    fake.pair('late-token');
    fake.emitState(6, 0, 'late');
    client.dispose();
    await eventually(() => fake.ownedOutstanding() == 0);
  });

  test('setVisibility', () {
    client.setVisibility(AppVisibility.hidden, focused: false);
    expect(fake.visibility(), 1);
    expect(fake.focused(), 0);
  });

  test('客户端 dispose 时进行中的调用以 CANCELLED 结束，之后 handler 的结果被丢弃', () async {
    final started = Completer<ToolContext>();
    final release = Completer<void>();
    client.tool('slow', description: '慢', handler: (args, ctx) async {
      started.complete(ctx);
      await release.future;
      return 'late';
    });
    final idx = fake.invoke('slow', '{}');
    final ctx = await started.future;
    client.dispose();
    expect(ctx.isCancelled, isTrue);
    expect((jsonDecode(fake.result(idx)!) as Map)['kind'], 'CANCELLED');
    release.complete();
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(() => client.start(), throwsA(isA<AppMcpException>()));
    // 重新创建一个供 tearDown 释放。
    client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
  });

  test('并发大量调用', () async {
    client.tool('echo', description: 'echo', handler: (args, ctx) async {
      await Future<void>.delayed(const Duration(milliseconds: 1));
      return args['i'];
    });
    final idxs = [for (var i = 0; i < 100; i++) fake.invoke('echo', '{"i":$i}')];
    for (var i = 0; i < idxs.length; i++) {
      expect((jsonDecode(await fake.waitResult(idxs[i])) as Map)['data'], i);
    }
  });
}
