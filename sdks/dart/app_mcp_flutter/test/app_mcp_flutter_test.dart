// 需要 Flutter SDK（flutter test）。McpTool 测试使用 app_mcp 的假原生库（需要 cc）。
import 'dart:ffi';
import 'dart:io';

import 'package:app_mcp_flutter/app_mcp_flutter.dart';
import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

String? _buildFake() {
  if (!Platform.isLinux && !Platform.isMacOS) return null;
  final src = File('../app_mcp/test/fake_native/fake_app_mcp.c').absolute.path;
  final dir = Directory.systemTemp.createTempSync('app_mcp_flutter_fake_');
  final out = '${dir.path}/libfake_app_mcp${Platform.isMacOS ? '.dylib' : '.so'}';
  try {
    final r = Process.runSync('cc', ['-shared', '-fPIC', '-o', out, src, '-lpthread']);
    return r.exitCode == 0 ? out : null;
  } on ProcessException {
    return null;
  }
}

void main() {
  group('visibilityForLifecycle', () {
    test('桌面端', () {
      expect(visibilityForLifecycle(AppLifecycleState.resumed, isMobile: false),
          (visibility: AppVisibility.visible, focused: true));
      expect(visibilityForLifecycle(AppLifecycleState.inactive, isMobile: false),
          (visibility: AppVisibility.visible, focused: false));
      expect(visibilityForLifecycle(AppLifecycleState.hidden, isMobile: false),
          (visibility: AppVisibility.hidden, focused: false));
      expect(visibilityForLifecycle(AppLifecycleState.paused, isMobile: false),
          (visibility: AppVisibility.hidden, focused: false));
      expect(visibilityForLifecycle(AppLifecycleState.detached, isMobile: false),
          (visibility: AppVisibility.frozen, focused: false));
    });
    test('移动端 paused 为冻结', () {
      expect(visibilityForLifecycle(AppLifecycleState.paused, isMobile: true),
          (visibility: AppVisibility.frozen, focused: false));
    });
  });

  test('wakeOnResume：只有 idle / onDemand 回连', () {
    expect(wakeOnResume(const LifecyclePolicy()), isFalse);
    expect(wakeOnResume(const LifecyclePolicy(mode: LifecycleMode.idle)), isTrue);
    expect(wakeOnResume(const LifecyclePolicy(mode: LifecycleMode.onDemand)), isTrue);
  });

  test('becameVisible：只在隐藏 / 冻结（或首次）→ 可见时回连', () {
    expect(becameVisible(null, AppVisibility.visible), isTrue);
    expect(becameVisible(AppVisibility.hidden, AppVisibility.visible), isTrue);
    expect(becameVisible(AppVisibility.frozen, AppVisibility.visible), isTrue);
    expect(becameVisible(AppVisibility.visible, AppVisibility.visible), isFalse);
    expect(becameVisible(null, AppVisibility.hidden), isFalse);
    expect(becameVisible(AppVisibility.visible, AppVisibility.frozen), isFalse);
  });

  final path = _buildFake();

  testWidgets('AppMcpScope：onDemand 首次进入前台回连；焦点变化不回连，从后台回到可见时回连', (tester) async {
    final client = AppMcp(
        appId: 'shop',
        appName: '商店',
        libraryPath: path,
        lifecycle: const LifecyclePolicy(mode: LifecycleMode.onDemand, sleepOnBackground: true));
    addTearDown(client.dispose);
    client.sleep(); // on-demand 启动后处于休眠
    expect(client.state.status, ConnectionStatus.dormant);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpWidget(AppMcpScope(client: client, child: const SizedBox()));
    expect(client.state.status, ConnectionStatus.waking); // 首次上报即可见

    client.sleep();
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    expect(client.state.status, ConnectionStatus.dormant); // 只是焦点变化

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    expect(client.state.status, ConnectionStatus.waking); // 隐藏 → 可见
    await tester.pumpWidget(const SizedBox());
  }, skip: path == null);

  testWidgets('McpResource：realtime 传入注册，变化时重新注册', (tester) async {
    final lib = DynamicLibrary.open(path!);
    final realtimeOf = lib.lookupFunction<Int32 Function(Pointer<Utf8>), int Function(Pointer<Utf8>)>(
        'fake_resource_realtime');
    int realtime(String name) => using((a) => realtimeOf(name.toNativeUtf8(allocator: a)));
    final client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
    addTearDown(client.dispose);
    Widget app({required bool live}) => AppMcpScope(
          client: client,
          trackLifecycle: false,
          child: McpResource(name: 'order.status', description: '订单状态', realtime: live, read: () => 1),
        );
    await tester.pumpWidget(app(live: true));
    expect(realtime('order.status'), 1);
    await tester.pumpWidget(app(live: false));
    expect(realtime('order.status'), 0);
    await tester.pumpWidget(const SizedBox());
    expect(realtime('order.status'), -1);
  }, skip: path == null);

  testWidgets('AppMcpScope：AppLifecycleListener 上报可见性，idle 模式回到前台时回连', (tester) async {
    final lib = DynamicLibrary.open(path!);
    final visibility = lib.lookupFunction<Int32 Function(), int Function()>('fake_visibility');
    final focused = lib.lookupFunction<Int32 Function(), int Function()>('fake_focused');
    final client = AppMcp(
        appId: 'shop',
        appName: '商店',
        libraryPath: path,
        lifecycle: const LifecyclePolicy(mode: LifecycleMode.idle));
    addTearDown(client.dispose);
    await tester.pumpWidget(AppMcpScope(client: client, child: const SizedBox()));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    expect((visibility(), focused()), (0, 0));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    expect((visibility(), focused()), (1, 0));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    expect(visibility(), 2); // flutter_test 的 defaultTargetPlatform 为 android：paused → frozen
    // 模拟后台休眠，然后回到前台。
    client.sleep(reason: SleepReason.background);
    expect(client.state.status, ConnectionStatus.dormant);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    expect((visibility(), focused()), (0, 1));
    expect(client.state.status, ConnectionStatus.waking);
    await tester.pumpWidget(const SizedBox());
  }, skip: path == null);

  testWidgets('AppMcpWakeChannel：平台通道的 handleWake 转交客户端', (tester) async {
    final client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
    addTearDown(client.dispose);
    final wake = AppMcpWakeChannel(client)..attach();
    addTearDown(wake.detach);
    const codec = StandardMethodCodec();
    Future<Object?> call(String method, Object? args) async {
      final reply = await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          AppMcpWakeChannel.channelName, codec.encodeMethodCall(MethodCall(method, args)), (_) {});
      return reply;
    }

    Object? decode(ByteData? data) => codec.decodeEnvelope(data!);
    ByteData? reply;
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(AppMcpWakeChannel.channelName,
        codec.encodeMethodCall(const MethodCall('handleWake', 'app-mcp-wake:tok')), (d) => reply = d);
    expect(decode(reply), isTrue);
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(AppMcpWakeChannel.channelName,
        codec.encodeMethodCall(const MethodCall('handleWake', 'myapp://other')), (d) => reply = d);
    expect(decode(reply), isFalse);
    await call('unknown', null);
    expect(wake.handleLink(Uri.parse('x:y')), isFalse);
    expect(wake.handleLink(null), isFalse);
  }, skip: path == null);

  testWidgets('McpTool：挂载注册、重建只更新、卸载注销', (tester) async {
    final lib = DynamicLibrary.open(path!);
    final describe = lib.lookupFunction<Pointer<Utf8> Function(Pointer<Utf8>),
        Pointer<Utf8> Function(Pointer<Utf8>)>('fake_tool_description');
    final freeCount = lib.lookupFunction<Int32 Function(), int Function()>('fake_free_count');
    final stringFree = lib.lookupFunction<Void Function(Pointer<Utf8>),
        void Function(Pointer<Utf8>)>('am_string_free');
    String? desc(String name) {
      final p = using((a) => describe(name.toNativeUtf8(allocator: a)));
      if (p == nullptr) return null;
      final s = p.toDartString();
      stringFree(p);
      return s;
    }

    final client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
    addTearDown(client.dispose);

    Widget app(String description, {bool show = true}) => AppMcpScope(
          client: client,
          trackLifecycle: false,
          child: show
              ? McpToolGroup(
                  name: 'page',
                  child: McpTool(
                      name: 'cart.add', description: description, handler: (a, c) => description),
                )
              : const SizedBox(),
        );

    await tester.pumpWidget(app('加入购物车'));
    expect(desc('cart.add'), '加入购物车');

    await tester.pumpWidget(app('加入购物车（新）'));
    expect(desc('cart.add'), '加入购物车（新）');
    final freesBefore = freeCount();

    await tester.pumpWidget(app('x', show: false));
    expect(desc('cart.add'), isNull);
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 50)));
    expect(freeCount(), greaterThan(freesBefore));
  }, skip: path == null);

  testWidgets('McpTool：注解与 outputSchema 传入注册，变化时整体替换', (tester) async {
    final lib = DynamicLibrary.open(path!);
    final options = lib.lookupFunction<Pointer<Utf8> Function(Pointer<Utf8>),
        Pointer<Utf8> Function(Pointer<Utf8>)>('fake_tool_options');
    final stringFree = lib.lookupFunction<Void Function(Pointer<Utf8>),
        void Function(Pointer<Utf8>)>('am_string_free');
    String? toolOptions(String name) {
      final p = using((a) => options(name.toNativeUtf8(allocator: a)));
      if (p == nullptr) return null;
      final s = p.toDartString();
      stringFree(p);
      return s;
    }

    final client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
    addTearDown(client.dispose);

    Widget app(ToolAnnotations? annotations) => AppMcpScope(
          client: client,
          trackLifecycle: false,
          child: McpTool(
              name: 'order.submit',
              description: '下单',
              annotations: annotations,
              outputSchema: const {'type': 'object'},
              handler: (a, c) => const ToolResult(null, status: ToolResultStatus.pending)),
        );

    await tester.pumpWidget(app(const ToolAnnotations(openWorldHint: true)));
    expect(toolOptions('order.submit'), '{"openWorldHint":true}|{"type":"object"}');
    await tester.pumpWidget(app(null));
    expect(toolOptions('order.submit'), 'null|{"type":"object"}');
  }, skip: path == null);
}
