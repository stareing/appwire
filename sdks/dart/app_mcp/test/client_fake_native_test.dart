// 用假原生库（test/fake_native/fake_app_mcp.c）测试 FFI 封装：
// 回调来自其他线程、临时字符串在回调返回后失效、free_user_data 在任意线程调用。
import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'package:app_mcp/app_mcp.dart';
import 'package:app_mcp/src/bindings.dart';
import 'package:ffi/ffi.dart';
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
    expect(sizeOf<AmResourceOptions>(), fake.sizeOf(14));
    expect(sizeOf<AmToolOptions>(), fake.sizeOf(16));
    expect(sizeOf<AmCallResult>(), fake.sizeOf(17));
  });

  test('AmClientOptions 的 v17 字段偏移与 C 一致', () {
    // 经 Dart 结构体写入，按 C 的 offsetof 读回。
    final probe = calloc<AmClientOptions>();
    try {
      probe.ref
        ..register_name = true
        ..name_instance = Pointer<Utf8>.fromAddress(0x1234);
      expect(probe.cast<Uint8>()[fake.sizeOf(23)], 1);
      expect(Pointer<IntPtr>.fromAddress(probe.address + fake.sizeOf(24)).value, 0x1234);
    } finally {
      calloc.free(probe);
    }
  });

  test('v18 字段偏移与 C 一致（AmClientOptions.max_queued_calls、AmToolOptions.exclusive）', () {
    final client = calloc<AmClientOptions>();
    final tool = calloc<AmToolOptions>();
    try {
      client.ref.max_queued_calls = -7;
      expect(Pointer<Int32>.fromAddress(client.address + fake.sizeOf(25)).value, -7);
      tool.ref.exclusive = Pointer<Utf8>.fromAddress(0x5678);
      expect(Pointer<IntPtr>.fromAddress(tool.address + fake.sizeOf(26)).value, 0x5678);
    } finally {
      calloc.free(client);
      calloc.free(tool);
    }
  });

  test('v21 字段偏移与 C 一致（AmToolOptions.implements / implements_len）', () {
    final tool = calloc<AmToolOptions>();
    try {
      tool.ref.implements = Pointer<Pointer<Utf8>>.fromAddress(0x9abc);
      tool.ref.implements_len = 3;
      expect(Pointer<IntPtr>.fromAddress(tool.address + fake.sizeOf(27)).value, 0x9abc);
      expect(Pointer<Size>.fromAddress(tool.address + fake.sizeOf(28)).value, 3);
    } finally {
      calloc.free(tool);
    }
  });

  test('implements 经 AmToolOptions（v21）传入；update 未提供保持、null 清除', () {
    final t = client.tool('link.open',
        description: '打开链接', implements: ['link.open@1', 'file.share@1'], handler: (args, ctx) => null);
    expect(fake.toolImplements('link.open'), 'link.open@1,file.share@1');
    t.update(description: '打开链接（新）');
    expect(fake.toolImplements('link.open'), 'link.open@1,file.share@1');
    t.update(implements: ['link.open@1']);
    expect(fake.toolImplements('link.open'), 'link.open@1');
    t.update(implements: null);
    expect(fake.toolImplements('link.open'), '-');
    expect(t.spec.implements, isEmpty);
    expect(() => t.update(implements: 'link.open@1'), throwsArgumentError);
    expect(ToolSpec(name: 'a', description: 'b', implements: ['x.y@1']), isNot(ToolSpec(name: 'a', description: 'b')));
    expect(ToolSpec(name: 'a', description: 'b', implements: ['x.y@1']),
        ToolSpec(name: 'a', description: 'b', implements: ['x.y@1']));
    expect(ToolSpec(name: 'a', description: 'b', implements: ['x.y@1']).hashCode,
        ToolSpec(name: 'a', description: 'b', implements: ['x.y@1']).hashCode);
    expect(ToolSpec(name: 'a', description: 'b').copyWith(implements: ['x.y@1']).implements, ['x.y@1']);
    client.tool('plain.app4', description: '普通', handler: (args, ctx) => null);
    expect(fake.toolImplements('plain.app4'), '-');
  });

  test('surface / page 经 AmToolOptions（v14）传入；update 未提供保持、null 清除', () {
    final t = client.tool('cart.checkout',
        description: '结算', surface: ToolSurface.view, page: 'cart', handler: (args, ctx) => null);
    expect(fake.toolView('cart.checkout'), '1|cart');
    t.update(description: '结算（新）');
    expect(fake.toolView('cart.checkout'), '1|cart');
    t.update(page: null);
    expect(fake.toolView('cart.checkout'), '1|null');
    t.update(surface: null, page: 'orders');
    expect(fake.toolView('cart.checkout'), '0|orders');
    expect(t.spec, isNot(ToolSpec(name: 'cart.checkout', description: '结算（新）')));
    expect(ToolSpec(name: 'a', description: 'b', page: 'p'), ToolSpec(name: 'a', description: 'b', page: 'p'));
    expect(ToolSpec(name: 'a', description: 'b').copyWith(surface: ToolSurface.view).surface, ToolSurface.view);
    client.tool('plain.app', description: '普通', handler: (args, ctx) => null);
    expect(fake.toolView('plain.app'), '0|null');
  });

  test('backgroundTool 经 AmToolOptions（v15）传入；update 未提供保持、null 清除', () {
    final t = client.tool('cart.view',
        description: '购物车', surface: ToolSurface.view, page: 'cart', backgroundTool: 'cart.summary',
        handler: (args, ctx) => null);
    expect(fake.toolBackground('cart.view'), 'cart.summary');
    t.update(description: '购物车（新）');
    expect(fake.toolBackground('cart.view'), 'cart.summary');
    expect(t.spec.backgroundTool, 'cart.summary');
    t.update(backgroundTool: null);
    expect(fake.toolBackground('cart.view'), isNull);
    expect(() => t.update(backgroundTool: 1), throwsArgumentError);
    expect(ToolSpec(name: 'a', description: 'b', backgroundTool: 'x'),
        isNot(ToolSpec(name: 'a', description: 'b')));
    expect(ToolSpec(name: 'a', description: 'b').copyWith(backgroundTool: 'x').backgroundTool, 'x');
    client.tool('plain.app2', description: '普通', handler: (args, ctx) => null);
    expect(fake.toolBackground('plain.app2'), isNull);
  });

  test('concurrency / exclusive 经 AmToolOptions（v18）传入；update 未提供保持、null 恢复缺省', () {
    final t = client.tool('doc.edit',
        description: '改文档', concurrency: 2, exclusive: 'doc', handler: (args, ctx) => null);
    expect(fake.toolSchedule('doc.edit'), '2|doc');
    t.update(description: '改文档（新）');
    expect(fake.toolSchedule('doc.edit'), '2|doc');
    t.update(concurrency: null, exclusive: null);
    expect(fake.toolSchedule('doc.edit'), '0|-');
    expect(t.spec.concurrency, 0);
    expect(() => t.update(concurrency: 'x'), throwsArgumentError);
    expect(() => t.update(concurrency: -1), throwsArgumentError);
    expect(fake.toolSchedule('doc.edit'), '0|-');
    expect(ToolSpec(name: 'a', description: 'b', concurrency: 1), isNot(ToolSpec(name: 'a', description: 'b')));
    expect(ToolSpec(name: 'a', description: 'b', exclusive: 'g'), isNot(ToolSpec(name: 'a', description: 'b')));
    expect(ToolSpec(name: 'a', description: 'b').copyWith(concurrency: 3, exclusive: 'g'),
        ToolSpec(name: 'a', description: 'b', concurrency: 3, exclusive: 'g'));
    client.tool('plain.app3', description: '普通', handler: (args, ctx) => null);
    expect(fake.toolSchedule('plain.app3'), '0|-');
  });

  group('工具声明与结构化结果（v9）', () {
    test('注解与 outputSchema 经 am_tool_register_ex 传入；replace 可清除', () {
      final t = client.tool('order.submit',
          description: '下单',
          annotations: const ToolAnnotations(idempotentHint: false, openWorldHint: true),
          outputSchema: {'type': 'object'},
          handler: (args, ctx) => null);
      expect(fake.toolOptions('order.submit'), '{"idempotentHint":false,"openWorldHint":true}|{"type":"object"}');
      client.tool('plain', description: '普通', handler: (args, ctx) => null);
      expect(fake.toolOptions('plain'), 'null|null');
      t.update(annotations: const ToolAnnotations(readOnlyHint: true));
      expect(fake.toolOptions('order.submit'), '{"readOnlyHint":true}|{"type":"object"}');
      t.replace(ToolSpec(name: 'order.submit', description: '下单'));
      expect(fake.toolOptions('order.submit'), 'null|null');
    });

    test('update：未提供的字段保持不变，显式 null 清除，给值则替换', () {
      final t = client.tool('doc.save',
          description: '保存',
          inputSchema: {'type': 'object', 'properties': <String, Object?>{}},
          risk: Risk.destructive,
          activation: Activation.foreground,
          title: '保存文档',
          annotations: const ToolAnnotations(readOnlyHint: false),
          outputSchema: {'type': 'object'},
          handler: (args, ctx) => null);
      final before = t.spec;
      t.update(description: '保存（新）');
      expect(fake.toolDescription('doc.save'), '保存（新）');
      expect(fake.toolOptions('doc.save'), '{"readOnlyHint":false}|{"type":"object"}');
      expect(t.spec.inputSchema, before.inputSchema);
      expect(
          [t.spec.risk, t.spec.activation, t.spec.title, t.spec.annotations],
          [before.risk, before.activation, before.title, before.annotations]);

      t.update(title: '另存', activation: Activation.background, outputSchema: {'type': 'array'});
      expect(t.spec.title, '另存');
      expect(t.spec.activation, Activation.background);
      expect(fake.toolOptions('doc.save'), '{"readOnlyHint":false}|{"type":"array"}');

      t.update(title: null, activation: null, annotations: null, outputSchema: null, inputSchema: null, risk: null);
      expect(fake.toolOptions('doc.save'), 'null|null');
      expect(
          [t.spec.title, t.spec.activation, t.spec.annotations, t.spec.outputSchema, t.spec.inputSchema],
          [null, null, null, null, null]);
      expect(t.spec.risk, Risk.write);
      expect(t.spec.description, '保存（新）');

      t.update(description: null); // description 不可清除
      expect(t.spec.description, '保存（新）');
      expect(() => t.update(title: 1), throwsArgumentError);
      expect(t.spec.title, isNull);
    });

    test('返回带业务状态的 ToolResult 经 am_call_complete_ex 完成', () async {
      client.tool('order.submit',
          description: '下单',
          handler: (args, ctx) => const ToolResult({'orderId': 'o1'},
              stateHints: ['cart'],
              status: ToolResultStatus.pending,
              stateResource: 'order.state',
              summary: 'submitted',
              annotations: ContentAnnotations(audience: [ContentAudience.user], priority: 0.5)));
      final r = await invoke('order.submit', {});
      expect(r, {
        'ok': true,
        'data': {'orderId': 'o1'},
        'hints': ['cart'],
        'status': AmResultStatus.pending,
        'stateResource': 'order.state',
        'summary': 'submitted',
        'annotations': {
          'audience': ['user'],
          'priority': 0.5
        },
      });
    });

    test('上下文 idempotencyKey 原样取自 am_call_idempotency_key（v16）；没有时为 null', () async {
      client.tool('k.key', description: 'k', handler: (args, ctx) => {'key': ctx.idempotencyKey});
      final idx = fake.invokeWithKey('k.key', '{}', 'Agent 键 / 1');
      expect(jsonDecode(await fake.waitResult(idx)), {
        'ok': true,
        'data': {'key': 'Agent 键 / 1'},
        'hints': <Object?>[]
      });
      expect(await invoke('k.key', {}), {
        'ok': true,
        'data': {'key': null},
        'hints': <Object?>[]
      });
    });

    test('只带 stateHints 的 ToolResult 与普通返回值仍走 am_call_complete（回归）', () async {
      client.tool('a', description: 'a', handler: (args, ctx) => const ToolResult(1, stateHints: ['x']));
      client.tool('b', description: 'b', handler: (args, ctx) => {'ok': true});
      expect(await invoke('a', {}), {'ok': true, 'data': 1, 'hints': ['x']});
      expect(await invoke('b', {}), {
        'ok': true,
        'data': {'ok': true},
        'hints': <Object?>[]
      });
    });
  });

  group('功耗选项（v7 / v8）', () {
    test('heartbeat 与生命周期新字段按 C ABI 编码传入', () {
      client.dispose();
      client = AppMcp(
        appId: 'shop',
        appName: '商店',
        libraryPath: path,
        heartbeat: HeartbeatMode.off,
        lifecycle: const LifecyclePolicy(
          mode: LifecycleMode.idle,
          hostAbsentRetries: 0,
          legacyTimers: true,
          mergeWindow: Duration.zero,
          sleepOnBackground: true,
        ),
      );
      // 0 = 一直重连 / 不留窗口 → C ABI 负数。
      expect(fake.lifecycle(), '1|60000|15000|10000|0|-1|-|0|0|2|-1|1|-1|1');
      client.dispose();
      client = AppMcp(
        appId: 'shop',
        appName: '商店',
        libraryPath: path,
        heartbeat: HeartbeatMode.always,
        lifecycle: const LifecyclePolicy(hostAbsentRetries: 7, mergeWindow: Duration(milliseconds: 500)),
      );
      expect(fake.lifecycle(), '0|60000|15000|10000|0|-1|-|0|0|1|7|0|500|0');
    });

    test('资源 realtime 经 am_resource_register_ex 传入', () {
      client.resource('order.status', description: '订单状态', realtime: true, read: () => {'s': 1});
      client.resource('cart', description: '购物车', read: () => const <Object?>[]);
      final scope = client.root.scope('page');
      scope.resource('page.live', description: '实时', realtime: true, read: () => null);
      expect(fake.resourceRealtime('order.status'), 1);
      expect(fake.resourceRealtime('cart'), 0);
      expect(fake.resourceRealtime('page.live'), 1);
    });

    test('资源内容标注经 am_resource_register_ex 传入（v13）', () {
      client.resource('order.status',
          description: '订单状态',
          annotations: const ContentAnnotations(audience: [ContentAudience.user], priority: 0.5),
          read: () => null);
      client.resource('cart', description: '购物车', read: () => null);
      expect(fake.resourceAnnotations('order.status'), '{"audience":["user"],"priority":0.5}');
      expect(fake.resourceAnnotations('cart'), isNull);
    });

    test('调用去重经 am_client_new_ex 传入（v13）', () {
      // 默认 5 分钟 / 64 条，原样传递。
      expect(fake.callDedup(), '300000|64');
      client.dispose();
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path, callDedup: CallDedupPolicy.off);
      // 0 = 关闭 → C ABI 负数。
      expect(fake.callDedup(), '-1|-1');
      client.dispose();
      client = AppMcp(
          appId: 'shop',
          appName: '商店',
          libraryPath: path,
          callDedup: const CallDedupPolicy(ttl: Duration(seconds: 1), maxEntries: 3));
      expect(fake.callDedup(), '1000|3');
    });

    test('按名寻址经 am_client_new_ex 传入（v17）', () {
      // 默认不登记。
      expect(fake.nameService(), '0|-');
      client.dispose();
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path, registerName: true);
      expect(fake.nameService(), '1|-');
      client.dispose();
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path, registerName: true, nameInstance: 'w2');
      expect(fake.nameService(), '1|w2');
    });

    test('排队上限经 am_client_new_ex 传入（v18）', () {
      // 默认 64 原样传递；0 = 不限 → C ABI 负数。
      expect(fake.maxQueuedCalls(), 64);
      client.dispose();
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path, maxQueuedCalls: 0);
      expect(fake.maxQueuedCalls(), -1);
      client.dispose();
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path, maxQueuedCalls: 3);
      expect(fake.maxQueuedCalls(), 3);
    });

    test('用户正在操作（v19）：busyPolicy 创建后传入，setBusy / isBusy / setBusyPolicy', () {
      // 默认 reject；创建时的策略随即设置。
      expect(fake.busyPolicy(), AmBusyPolicy.reject);
      client.dispose();
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path, busyPolicy: BusyPolicy.queue);
      expect(fake.busyPolicy(), AmBusyPolicy.queue);
      expect(client.isBusy, isFalse);
      client.setBusy(true);
      expect(client.isBusy, isTrue);
      client.setBusy(false);
      expect(client.isBusy, isFalse);
      client.setBusyPolicy(BusyPolicy.reject);
      expect(fake.busyPolicy(), AmBusyPolicy.reject);

      // 作用域：引用计数；嵌套时先结束一个仍为 busy，重复 release 不多减。
      final outer = client.beginBusy();
      final inner = client.beginBusy();
      expect((client.isBusy, fake.busy()), (true, 1));
      inner.release();
      inner.release();
      expect((client.isBusy, fake.busy(), inner.isReleased), (true, 1, true));
      outer.release();
      expect((client.isBusy, fake.busy()), (false, 0));

      // 开关与作用域互不清除。
      final held = client.beginBusy();
      client.setBusy(false);
      expect((client.isBusy, fake.busy()), (true, 1));
      client.setBusy(true);
      held.release();
      expect((client.isBusy, fake.busy()), (true, 1));
      client.setBusy(false);
      expect((client.isBusy, fake.busy()), (false, 0));

      final lingering = client.beginBusy();
      client.dispose();
      lingering.release(); // 客户端已释放：不抛出
      expect(() => client.setBusy(true), throwsA(isA<AppMcpException>()));
      expect(() => client.isBusy, throwsA(isA<AppMcpException>()));
    });
  });

  group('事件（v20）', () {
    setUp(fake.eventsReset);

    test('declareEvent / emitEvent / removeEvent：参数编码、未连接返回 false、错误码', () {
      AppMcpErrorCode codeOf(void Function() f) {
        try {
          f();
        } on AppMcpException catch (e) {
          return e.code;
        }
        fail('应抛出 AppMcpException');
      }

      expect(codeOf(() => client.emitEvent('order.shipped')), AppMcpErrorCode.invalidName); // 未声明
      expect(codeOf(() => client.declareEvent('bad name', '非法')), AppMcpErrorCode.invalidName);
      expect(codeOf(() => client.declareEvent('order.shipped', '订单已发货', payloadSchema: [1])),
          AppMcpErrorCode.invalidSchema);
      client.declareEvent('order.shipped', '订单已发货', payloadSchema: {'type': 'object'});
      expect(fake.eventInfo('order.shipped'), '订单已发货|{"type":"object"}');
      client.declareEvent('download.done', '下载完成');
      expect(fake.eventInfo('download.done'), '下载完成|-');

      expect(client.emitEvent('order.shipped', {'orderId': 'o1'}), isFalse); // 未连接：丢弃
      expect(fake.lastEmit(), isNull);
      fake.eventsConnected(true);
      expect(client.emitEvent('order.shipped', {'orderId': 'o1'}), isTrue);
      expect(fake.lastEmit(), 'order.shipped|{"orderId":"o1"}');
      expect(client.emitEvent('download.done'), isTrue);
      expect(fake.lastEmit(), 'download.done|-');
      expect(codeOf(() => client.emitEvent('order.shipped', 1)), AppMcpErrorCode.invalidJson);

      expect(client.removeEvent('order.shipped'), isTrue);
      expect(client.removeEvent('order.shipped'), isFalse);
      expect(codeOf(() => client.emitEvent('order.shipped')), AppMcpErrorCode.invalidName);

      client.dispose();
      expect(() => client.emitEvent('download.done'), throwsA(isA<AppMcpException>()));
      client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
    });
  });

  group('生命周期（v3）', () {
    test('默认策略（桌面）与 connectTimeout 传入 am_client_new_ex', () {
      // 默认：persistent、不上报唤醒描述（-1）、connect_timeout 0（原生默认）。
      expect(fake.lifecycle(), '0|60000|15000|10000|0|-1|-|0|0|0|3|0|2000|0');
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
      expect(fake.lifecycle(), '2|300|0|1000|1|1|shop|1|2000|0|3|0|2000|0');
      client.dispose();
      client = AppMcp(
          appId: 'shop',
          appName: '商店',
          libraryPath: path,
          lifecycle: const LifecyclePolicy(
              mode: LifecycleMode.idle, wake: WakeDescriptor.androidIntent('com.x/.WakeReceiver')));
      expect(fake.lifecycle(), '1|60000|15000|10000|0|5|com.x/.WakeReceiver|1|0|0|3|0|2000|0');
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

    test('UserActionRequiredError 经 am_call_fail_user_action 上报；reason / uri 缺省时不传', () async {
      client.tool('login',
          description: '需登录',
          handler: (a, c) =>
              throw UserActionRequiredError('登录已过期', reason: UserActionReason.login, uri: 'shop://login'));
      client.tool('front', description: '需前台', handler: (a, c) async => throw UserActionRequiredError('请切到前台'));
      expect(await invoke('login', {}), {
        'ok': false,
        'kind': 'USER_ACTION_REQUIRED',
        'message': '登录已过期',
        'reason': 'login',
        'uri': 'shop://login'
      });
      expect(await invoke('front', {}), {'ok': false, 'kind': 'USER_ACTION_REQUIRED', 'message': '请切到前台'});
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

    test('ctx.progress()：交给原生层；调用完成后无副作用', () async {
      ToolContext? saved;
      fake.progress();
      client.tool('export', description: '导出', handler: (a, ctx) {
        ctx.progress(1, total: 3, message: '第 1 页');
        ctx.progress(2);
        saved = ctx;
        return 'ok';
      });
      expect((await invoke('export', {}))['data'], 'ok');
      expect(fake.progress(), '1|3|第 1 页\n2|-|\n');
      saved!.progress(3);
      expect(fake.progress(), '');
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

  test('资源读取失败：UserActionRequiredError 带 reason / uri，ToolCallError 带详情（v12）', () async {
    client.resource('session',
        description: '会话',
        read: () => throw UserActionRequiredError('登录已过期', reason: UserActionReason.login, uri: 'shop://login'));
    client.resource('front', description: '前台', read: () async => throw UserActionRequiredError('请切到前台'));
    client.resource('quota',
        description: '额度', read: () => throw ToolCallError(ErrorKind.userRejected, '额度不足', details: {'quota': 0}));
    client.resource('quota2',
        description: '额度', read: () => throw ToolCallError(ErrorKind.userRejected, '坏详情', details: Object()));
    expect(jsonDecode(await fake.waitResult(fake.read('session'))), {
      'ok': false,
      'kind': 'USER_ACTION_REQUIRED',
      'message': '登录已过期',
      'reason': 'login',
      'uri': 'shop://login'
    });
    expect(jsonDecode(await fake.waitResult(fake.read('front'))),
        {'ok': false, 'kind': 'USER_ACTION_REQUIRED', 'message': '请切到前台'});
    expect(jsonDecode(await fake.waitResult(fake.read('quota'))), {
      'ok': false,
      'kind': 'USER_REJECTED',
      'message': '额度不足',
      'details': {'quota': 0}
    });
    expect(jsonDecode(await fake.waitResult(fake.read('quota2'))),
        {'ok': false, 'kind': 'USER_REJECTED', 'message': '坏详情'});
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

  test('状态流与 state 带 code（v6 am_client_state_code）；connectionId 取自 am_client_connection_id', () async {
    final states = <McpConnectionState>[];
    final sub = client.states.listen(states.add);
    expect(client.connectionId, isNull); // 未连接
    client.start();
    await eventually(() => states.isNotEmpty);
    expect(states.last.status, ConnectionStatus.connected);
    expect(states.last.code, isNull);
    expect(client.connectionId, 'fake-cid-1');

    fake.emitStateCode(5, 1000, '连接被拒绝', 'HOST_NOT_RUNNING');
    await eventually(() => states.length == 2);
    expect(states.last,
        const McpConnectionState(ConnectionStatus.backoff,
            retryIn: Duration(seconds: 1), reason: '连接被拒绝', code: 'HOST_NOT_RUNNING'));
    expect(client.state.code, 'HOST_NOT_RUNNING');
    expect(client.state.reason, '连接被拒绝');
    expect(client.connectionId, isNull);

    fake.emitStateCode(10, 0, '不是 app-mcp', 'HOST_NOT_APP_MCP');
    await eventually(() => states.length == 3);
    expect(states.last.status, ConnectionStatus.hostMismatch);
    expect(states.last.code, 'HOST_NOT_APP_MCP');

    // 不带码的状态即使原生侧残留 code 也为 null。
    fake.emitStateCode(4, 0, null, 'STALE');
    await eventually(() => states.length == 4);
    expect(states.last.code, isNull);
    expect(client.state.code, isNull);
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
