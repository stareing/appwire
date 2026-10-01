// 惯用 Dart 封装：AppMcp 客户端、Scope、工具与资源句柄。
//
// # 线程模型
//
// C ABI 的所有回调（工具调用、资源读取、取消、状态、配对、free_user_data）都在库的
// 分发线程上执行。这里全部用 `NativeCallable.listener` 接收：库线程调用时只是把参数
// 投递到创建它的 isolate 的事件循环，随后在该 isolate（Flutter 中即主 isolate）上执行
// Dart 代码。因此 handler 总在创建 [AppMcp] 的 isolate 上运行，可以直接访问 UI 状态。
//
// 由此带来的约束：
// - listener 是异步的，Dart 代码执行时原生回调早已返回。C ABI 自 API 版本 2 起，状态回调的
//   reason、配对回调的 token、日志回调的 message 归接收方所有，回调返回后仍然有效；这里
//   读取后立即 `am_string_free`（无论目标对象是否还在，都要释放，否则泄漏）。
//   本文件按 AM_API_VERSION 2 编写，不能与 v1 的库混用（v1 下这些字符串会悬空）。
// - AmCall / AmRead 的所有权转移给回调方，在消费（complete / fail）前一直有效，可以异步使用。
//
// # NativeCallable 生命周期
//
// `free_user_data` 可能在库的任意线程、任意时间调用（最后一个引用被丢弃时），关闭后的
// NativeCallable 被调用是未定义行为。因此每个原生库实例只创建一组共享的 listener
// （工具、读取、取消、状态、配对、日志、free 各一个），设置 `keepIsolateAlive = false`，不关闭；
// user_data 只是整数 ID，指向 Dart 侧注册表中的对象。这样既不会调用已关闭的回调，
// 也不会因为回调而阻止 isolate 退出。注册表条目在库调用 free_user_data 或客户端 dispose 时删除。

import 'dart:async';
import 'dart:ffi';
import 'dart:io' show Platform;

import 'package:ffi/ffi.dart';

import 'bindings.dart';
import 'convert.dart';
import 'types.dart';

// ---------------------------------------------------------------------------
// 运行时：共享回调与注册表
// ---------------------------------------------------------------------------

final class _Runtime {
  _Runtime(this.b) {
    for (final c in <NativeCallable<Function>>[tool, read, cancel, state, paired, log, free, idleExit]) {
      c.keepIsolateAlive = false;
    }
  }

  static final Map<AppMcpBindings, _Runtime> _byBindings = Map.identity();

  static _Runtime of(AppMcpBindings b) => _byBindings.putIfAbsent(b, () => _Runtime(b));

  final AppMcpBindings b;

  /// user_data ID → 目标对象。ID 单调递增，从不复用。
  final Map<int, Object> targets = {};
  int _nextId = 1;

  late final NativeCallable<AmToolFnNative> tool =
      NativeCallable<AmToolFnNative>.listener(_onTool);
  late final NativeCallable<AmReadFnNative> read =
      NativeCallable<AmReadFnNative>.listener(_onRead);
  late final NativeCallable<AmCancelFnNative> cancel =
      NativeCallable<AmCancelFnNative>.listener(_onCancel);
  late final NativeCallable<AmStateFnNative> state =
      NativeCallable<AmStateFnNative>.listener(_onState);
  late final NativeCallable<AmPairedFnNative> paired =
      NativeCallable<AmPairedFnNative>.listener(_onPaired);
  late final NativeCallable<AmLogFnNative> log =
      NativeCallable<AmLogFnNative>.listener(_onLog);
  late final NativeCallable<AmFreeFnNative> free =
      NativeCallable<AmFreeFnNative>.listener(_onFree);
  late final NativeCallable<AmIdleExitFnNative> idleExit =
      NativeCallable<AmIdleExitFnNative>.listener(_onIdleExit);

  /// 持有句柄的终结器：[_Hold] 未显式释放就被回收时调用 `am_hold_release`。
  late final NativeFinalizer holdFinalizer = NativeFinalizer(b.am_hold_release_ptr.cast());

  int register(Object target) {
    final id = _nextId++;
    targets[id] = target;
    return id;
  }

  void _onTool(Pointer<Void> userData, Pointer<AmCall> call) {
    final target = targets[userData.address];
    if (target is _ToolEntry && !target.disposed) {
      target.client._dispatchCall(target, call);
    } else {
      // 工具已在 Dart 侧注销，但调用已在途中：直接失败以消费 call。
      _withStrings2(ErrorKind.toolNotFound.wireName, '工具已注销',
          (k, m) => b.am_call_fail(call, k, m));
    }
  }

  void _onRead(Pointer<Void> userData, Pointer<AmRead> read) {
    final target = targets[userData.address];
    if (target is _ResourceEntry && !target.disposed) {
      target.client._dispatchRead(target, read);
    } else {
      _withStrings2(ErrorKind.resourceNotFound.wireName, '资源已注销',
          (k, m) => b.am_read_fail(read, k, m));
    }
  }

  void _onCancel(Pointer<Void> userData, int reason) {
    final target = targets[userData.address];
    if (target is _PendingCall) target.onCancel(cancelReasonFromNative(reason));
  }

  void _onState(Pointer<Void> userData, int status, int retryInMs, Pointer<Utf8> reason) {
    // reason 归本方所有（API v2），先取出并释放。
    final r = _takeString(b, reason);
    final target = targets[userData.address];
    if (target is AppMcp) target._onNativeState(status, retryInMs, r);
  }

  void _onPaired(Pointer<Void> userData, Pointer<Utf8> token) {
    final t = _takeString(b, token);
    final target = targets[userData.address];
    if (target is AppMcp && t != null) target._onNativePaired(t);
  }

  void _onLog(Pointer<Void> userData, int level, Pointer<Utf8> message) {
    final m = _takeString(b, message);
    final target = targets[userData.address];
    if (target is AppMcp && m != null) target._onNativeLog(logLevelFromNative(level), m);
  }

  void _onFree(Pointer<Void> userData) {
    targets.remove(userData.address);
  }

  void _onIdleExit(Pointer<Void> userData) {
    final target = targets[userData.address];
    if (target is AppMcp) target._onNativeIdleExit();
  }

  /// 执行一个带 `bool*` 输出参数的原生函数，返回输出值。
  bool flag(int Function(Pointer<Bool>) f) => using((arena) {
        final out = arena<Bool>();
        out.value = false;
        check(f(out));
        return out.value;
      });

  McpHold hold(int Function(Pointer<Pointer<AmHold>>) f) {
    final ptr = using((arena) {
      final out = arena<Pointer<AmHold>>();
      check(f(out));
      return out.value;
    });
    return _Hold(this, ptr);
  }

  /// 读取最近一次错误并构造异常。必须紧接失败的调用执行（同一线程）。
  AppMcpException error(int status) {
    final p = b.am_last_error_message();
    final msg = p == nullptr ? '' : p.toDartString();
    return AppMcpException(errorCodeFromStatus(status), msg.isEmpty ? '状态码 $status' : msg);
  }

  void check(int status) {
    if (status != AmStatus.ok) throw error(status);
  }
}

/// 按错误类型失败完成调用或读取（C ABI 的 am_call_fail* 与 am_read_fail* 同签名、同语义），总是消费 [ptr]。
///
/// @invariant [UserActionRequiredError] → failUserAction（reason / uri）；带详情 → failWithDetails（详情非法时
/// 未被消费，退回不带详情的 fail）；其他 → fail。
void _failNative<T extends NativeType>(
  Pointer<T> ptr,
  Object error, {
  required int Function(Pointer<T>, Pointer<Utf8>, Pointer<Utf8>) fail,
  required int Function(Pointer<T>, Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>) failWithDetails,
  required int Function(Pointer<T>, Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>) failUserAction,
}) {
  if (error is UserActionRequiredError) {
    using((arena) => failUserAction(ptr, error.message.toNativeUtf8(allocator: arena),
        _optStr(error.reason, arena), _optStr(error.uri, arena)));
    return;
  }
  final f = failureFromError(error);
  final details = f.detailsJson;
  if (details != null) {
    final status = using((arena) => failWithDetails(
        ptr,
        f.kind.toNativeUtf8(allocator: arena),
        f.message.toNativeUtf8(allocator: arena),
        details.toNativeUtf8(allocator: arena)));
    if (status != AmStatus.invalidJson) return;
  }
  _withStrings2(f.kind, f.message, (k, m) => fail(ptr, k, m));
}

T _withStrings2<T>(String a, String b, T Function(Pointer<Utf8>, Pointer<Utf8>) f) {
  return using((arena) => f(a.toNativeUtf8(allocator: arena), b.toNativeUtf8(allocator: arena)));
}

Pointer<Utf8> _optStr(String? s, Allocator arena) =>
    s == null ? nullptr : s.toNativeUtf8(allocator: arena);

String? _takeString(AppMcpBindings b, Pointer<Utf8> p) {
  if (p == nullptr) return null;
  try {
    return p.toDartString();
  } finally {
    b.am_string_free(p);
  }
}

/// [McpHold] 的实现：显式 [release] 或被回收时调用 `am_hold_release`（恰好一次）。
final class _Hold implements McpHold, Finalizable {
  _Hold(this._rt, this._ptr) {
    _rt.holdFinalizer.attach(this, _ptr.cast(), detach: this);
  }

  final _Runtime _rt;
  final Pointer<AmHold> _ptr;
  bool _released = false;

  @override
  bool get isReleased => _released;

  @override
  void release() {
    if (_released) return;
    _released = true;
    _rt.holdFinalizer.detach(this);
    _rt.b.am_hold_release(_ptr);
  }
}

// ---------------------------------------------------------------------------
// 注册表条目
// ---------------------------------------------------------------------------

final class _ToolEntry {
  _ToolEntry(this.client, this.handler);
  final AppMcp client;
  ToolHandler handler;
  bool disposed = false;
}

final class _ResourceEntry {
  _ResourceEntry(this.client, this.reader);
  final AppMcp client;
  ResourceReader reader;
  bool disposed = false;
}

/// 进行中的调用。
final class _PendingCall implements ToolContext {
  _PendingCall(this.rt, this.ptr, this.callId, this.toolName);

  final _Runtime rt;
  AppMcpBindings get b => rt.b;
  Pointer<AmCall> ptr;
  @override
  final String callId;
  @override
  final String toolName;
  int cancelId = 0;
  bool consumed = false;
  CancelReason? _reason;
  final Completer<CancelReason> _cancelled = Completer<CancelReason>();

  void onCancel(CancelReason reason) {
    if (_reason != null) return;
    _reason = reason;
    _cancelled.complete(reason);
  }

  @override
  bool get isCancelled {
    if (_reason != null) return true;
    // 取消通知是异步投递的；未消费时再同步查询一次原生状态。
    if (!consumed && b.am_call_is_cancelled(ptr)) {
      onCancel(CancelReason.requested);
      return true;
    }
    return false;
  }

  @override
  CancelReason? get cancelReason => isCancelled ? _reason : null;

  @override
  Future<CancelReason> get cancelled => _cancelled.future;

  @override
  McpHold hold() {
    if (consumed) throw AppMcpException(AppMcpErrorCode.alreadyCompleted, '调用已完成');
    return rt.hold((out) => b.am_call_hold(ptr, out));
  }

  @override
  void progress(double progress, {double? total, String? message}) {
    if (consumed || _reason != null) return;
    // 调用刚被取消时原生层返回 alreadyCompleted：进度只是提示，忽略返回码。
    using((arena) => b.am_call_progress(
        ptr, progress, total ?? -1.0, message == null ? nullptr : message.toNativeUtf8(allocator: arena)));
  }
}

final class _PendingRead {
  _PendingRead(this.ptr);
  final Pointer<AmRead> ptr;
  bool consumed = false;
}

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

/// app-mcp 客户端。
///
/// ```dart
/// final client = AppMcp(appId: 'shop', appName: '商店');
/// client.tool('cart.add', description: '加入购物车', inputSchema: {...},
///     handler: (args, ctx) async => {'ok': true});
/// client.start();
/// ```
///
/// 所有方法都应在创建它的 isolate 上调用；handler 也在该 isolate 上执行。
final class AppMcp {
  /// 创建客户端并启动原生后台线程（不连接，调用 [start] 后开始连接）。
  ///
  /// [overview] 为 App 总览，Host 在模型第一次接触该 App 时附带。
  ///
  /// [lifecycle] 缺省时按平台取 [LifecyclePolicy.platformDefault]；显式传入的策略原样使用。
  /// [heartbeat] 为心跳策略（spec/lifecycle.md 第 11 节 A3）。
  ///
  /// [libraryPath] 指定原生库路径；缺省时读取环境变量 `APP_MCP_NATIVE_PATH`，
  /// 否则按平台默认名加载。失败时抛出 [AppMcpException]。
  factory AppMcp({
    required String appId,
    required String appName,
    String? hostUrl,
    String? appVersion,
    String? instanceId,
    String? instanceTitle,
    String? token,
    String? launchToken,
    ClientKind clientKind = ClientKind.native,
    int maxConcurrentCalls = 1,
    AppOverview? overview,
    LifecyclePolicy? lifecycle,
    Duration? connectTimeout,
    HeartbeatMode heartbeat = HeartbeatMode.auto,
    String? libraryPath,
    AppMcpBindings? bindings,
  }) {
    final b = bindings ?? _defaultBindings(libraryPath);
    final rt = _Runtime.of(b);
    final policy = lifecycle ??
        LifecyclePolicy.platformDefault(isAndroid: Platform.isAndroid, isIOS: Platform.isIOS);
    final client = AppMcp._(rt, policy);
    final id = rt.register(client);
    client._userDataId = id;
    try {
      using((arena) {
        final config = arena<AmClientConfig>();
        config.ref
          ..app_id = appId.toNativeUtf8(allocator: arena)
          ..app_name = appName.toNativeUtf8(allocator: arena)
          ..instance_id = _optStr(instanceId, arena)
          ..host_url = _optStr(hostUrl, arena)
          ..app_version = _optStr(appVersion, arena)
          ..instance_title = _optStr(instanceTitle, arena)
          ..token = _optStr(token, arena)
          ..launch_token = _optStr(launchToken, arena)
          ..client_kind = clientKindToNative(clientKind)
          ..max_concurrent_calls = maxConcurrentCalls < 0 ? 0 : maxConcurrentCalls
          ..overview_summary = _optStr(overview?.summary, arena)
          ..overview_body = _optStr(overview?.body, arena)
          ..overview_locale = _optStr(overview?.locale, arena);
        final callbacks = arena<AmClientCallbacks>();
        callbacks.ref
          ..on_state = rt.state.nativeFunction
          ..on_paired = rt.paired.nativeFunction
          ..on_log = rt.log.nativeFunction
          ..user_data = Pointer<Void>.fromAddress(id)
          ..free_user_data = rt.free.nativeFunction;
        final life = arena<AmLifecycle>();
        life.ref
          ..mode = lifecycleModeToNative(policy.mode)
          ..idle_timeout_ms = durationToMs(policy.idleTimeout)
          ..hidden_idle_timeout_ms = durationToMs(policy.hiddenIdleTimeout)
          ..grace_ms = durationToMs(policy.grace)
          ..residency = residencyToNative(policy.residency)
          ..wake_kind = wakeKindToNative(policy.wake?.kind)
          ..wake_target = _optStr(policy.wake?.target, arena)
          ..wake_background = policy.wake?.background ?? false;
        final options = arena<AmClientOptions>();
        final timeoutMs = connectTimeout == null ? 0 : durationToMs(connectTimeout);
        options.ref
          ..struct_size = sizeOf<AmClientOptions>()
          ..lifecycle = life
          ..connect_timeout_ms = timeoutMs > 0xFFFFFFFF ? 0xFFFFFFFF : timeoutMs
          ..on_idle_exit = rt.idleExit.nativeFunction
          ..heartbeat = heartbeatToNative(heartbeat)
          ..host_absent_retries = hostAbsentRetriesToNative(policy.hostAbsentRetries)
          ..legacy_timers = policy.legacyTimers
          ..merge_window_ms = mergeWindowToNative(policy.mergeWindow)
          ..sleep_on_background = policy.sleepOnBackground;
        final out = arena<Pointer<AmClient>>();
        rt.check(b.am_client_new_ex(config, callbacks, options, out));
        client._ptr = out.value;
        final scopeOut = arena<Pointer<AmScope>>();
        final status = b.am_client_root_scope(client._ptr, scopeOut);
        if (status != AmStatus.ok) {
          final e = rt.error(status);
          b.am_client_free(client._ptr);
          throw e;
        }
        client._root = McpScope._(client, scopeOut.value, null, isRoot: true);
      });
    } catch (_) {
      rt.targets.remove(id);
      rethrow;
    }
    return client;
  }

  AppMcp._(this._rt, this.lifecycle);

  /// 生命周期策略（创建时确定）。
  final LifecyclePolicy lifecycle;

  static AppMcpBindings? _shared;
  static final Map<String, AppMcpBindings> _byPath = {};

  /// 同一路径只加载一次，保证共享同一组回调（见文件头的生命周期说明）。
  static AppMcpBindings _defaultBindings(String? path) {
    if (path != null) return _byPath.putIfAbsent(path, () => AppMcpBindings.load(path));
    return _shared ??= AppMcpBindings.load();
  }

  final _Runtime _rt;
  AppMcpBindings get _b => _rt.b;
  late final Pointer<AmClient> _ptr;
  late final McpScope _root;
  late final int _userDataId;
  bool _disposed = false;

  final StreamController<McpConnectionState> _states = StreamController<McpConnectionState>.broadcast();
  final StreamController<String> _paired = StreamController<String>.broadcast();
  final StreamController<McpLogRecord> _logs = StreamController<McpLogRecord>.broadcast();
  final StreamController<void> _idleExit = StreamController<void>.broadcast();
  final Set<_PendingCall> _calls = {};
  final Set<_PendingRead> _reads = {};

  /// 该客户端注册过的所有 user_data ID（dispose 时从注册表删除）。
  final Set<int> _ownedIds = {};

  /// 从激活参数中提取唤醒令牌（不需要客户端）。不是唤醒参数时返回 null。
  static String? parseWakeToken(String args, {String? libraryPath}) {
    final b = _defaultBindings(libraryPath);
    return using((arena) => _takeString(b, b.am_parse_wake_token(args.toNativeUtf8(allocator: arena))));
  }

  /// 原生库版本，如 `0.1.0`。
  static String nativeVersion({String? libraryPath}) =>
      _defaultBindings(libraryPath).am_version().toDartString();

  /// 根作用域。
  McpScope get root => _root;

  bool get isDisposed => _disposed;

  /// 连接状态变化（在创建客户端的 isolate 上投递）。
  Stream<McpConnectionState> get states => _states.stream;

  /// 配对成功并获得新 token；App 应持久化，下次传入 `token`。
  Stream<String> get onPaired => _paired.stream;

  /// 原生库日志（在创建客户端的 isolate 上投递）。没有监听者时丢弃。
  Stream<McpLogRecord> get logs => _logs.stream;

  /// 当前状态（同步查询原生库）。
  McpConnectionState get state {
    _ensureAlive();
    return using((arena) {
      final status = arena<Int32>();
      final retry = arena<Uint64>();
      final reason = arena<Pointer<Utf8>>();
      reason.value = nullptr;
      _rt.check(_b.am_client_state(_ptr, status, retry, reason));
      final r = _takeString(_b, reason.value);
      return stateFromNative(status.value, retry.value, r, code: _queryStateCode());
    });
  }

  /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 `cid` 对应；未连接时为 null。
  String? get connectionId {
    _ensureAlive();
    return _takeOut(_b.am_client_connection_id);
  }

  /// 当前状态的错误码（am_client_state_code）。
  String? _queryStateCode() => _takeOut(_b.am_client_state_code);

  /// 调用 `AmStatus f(client, char **out)` 形式的查询，取走输出字符串。
  String? _takeOut(int Function(Pointer<AmClient>, Pointer<Pointer<Utf8>>) f) => using((arena) {
        final out = arena<Pointer<Utf8>>();
        out.value = nullptr;
        _rt.check(f(_ptr, out));
        return _takeString(_b, out.value);
      });

  String get instanceId {
    _ensureAlive();
    return _takeString(_b, _b.am_client_instance_id(_ptr)) ?? '';
  }

  /// 当前 token（配置带入的或配对后获得的）。
  String? get token {
    _ensureAlive();
    return _takeString(_b, _b.am_client_token(_ptr));
  }

  /// 开始连接。重复调用无效果。
  void start() {
    _ensureAlive();
    _rt.check(_b.am_client_start(_ptr));
  }

  /// 停止：取消所有调用、断开连接、不再重连。
  void stop() {
    _ensureAlive();
    _rt.check(_b.am_client_stop(_ptr));
  }

  void setVisibility(AppVisibility visibility, {bool focused = true}) {
    _ensureAlive();
    _rt.check(_b.am_client_set_visibility(_ptr, visibilityToNative(visibility), focused));
  }

  // ---- 生命周期（spec/lifecycle.md 第 8 节） ----

  /// 处理操作系统激活参数 / URL：`app-mcp-wake:<token>`（Android extra、Windows 激活参数）、
  /// `<scheme>://app-mcp/wake?token=`（iOS / macOS `onOpenURL`）、`#app-mcp-wake=<token>`。
  /// 不是本 SDK 的唤醒返回 false。可以在 [start] 之前调用（冷启动唤醒）。
  bool handleWake(String args) {
    _ensureAlive();
    return using((arena) => _b.am_client_handle_wake(_ptr, args.toNativeUtf8(allocator: arena)));
  }

  /// App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。
  bool wake({WakeReason reason = WakeReason.app}) {
    _ensureAlive();
    return _rt.flag((out) => _b.am_client_wake_with_reason(_ptr, wakeReasonToNative(reason), out));
  }

  /// on-demand 模式下主动连接；尚未 [start] 时等同于 [start]。
  bool connectNow() {
    _ensureAlive();
    return _rt.flag((out) => _b.am_client_connect_now(_ptr, out));
  }

  /// App 主动请求休眠（不受空闲条件与持有影响）。返回是否有效果。
  bool sleep({SleepReason reason = SleepReason.app}) {
    _ensureAlive();
    return _rt.flag((out) => _b.am_client_sleep_with_reason(_ptr, sleepReasonToNative(reason), out));
  }

  /// 临时阻止自动休眠，直到返回的持有被释放。
  McpHold hold() {
    _ensureAlive();
    return _rt.hold((out) => _b.am_client_hold(_ptr, out));
  }

  /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
  String get toolsHash {
    _ensureAlive();
    final p = _b.am_client_tools_hash(_ptr);
    if (p == nullptr) throw _rt.error(AmStatus.internal);
    return _takeString(_b, p)!;
  }

  /// 已进入休眠且驻留策略（[Residency]）允许退出进程时触发；App 自行决定是否退出
  /// （例如先 [dispose] 再 `exit(0)`）。
  Stream<void> get onIdleExit => _idleExit.stream;

  /// 在根作用域注册工具。见 [McpScope.tool]。
  ToolHandle tool(
    String name, {
    required String description,
    Map<String, Object?>? inputSchema,
    Risk risk = Risk.write,
    Activation? activation,
    String? title,
    bool enabled = true,
    ToolAnnotations? annotations,
    Map<String, Object?>? outputSchema,
    required ToolHandler handler,
  }) =>
      _root.tool(name,
          description: description,
          inputSchema: inputSchema,
          risk: risk,
          activation: activation,
          title: title,
          enabled: enabled,
          annotations: annotations,
          outputSchema: outputSchema,
          handler: handler);

  /// 在根作用域注册资源。见 [McpScope.resource]。
  ResourceHandle resource(String name,
          {required String description,
          String? mimeType,
          bool realtime = false,
          required ResourceReader read}) =>
      _root.resource(name, description: description, mimeType: mimeType, realtime: realtime, read: read);

  /// 在根作用域下创建子作用域。
  McpScope scope(String name) => _root.scope(name);

  /// 停止并释放客户端。之后所有句柄失效。幂等。
  ///
  /// 会阻塞到原生后台线程结束（通常很快）。仍在执行的 handler 结果将被丢弃。
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _b.am_client_stop(_ptr);
    // 消费所有未完成的调用与读取，避免 AmCall / AmRead 泄漏。
    for (final call in _calls.toList()) {
      if (call.consumed) continue;
      call.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_call_fail(call.ptr, k, m));
      call.onCancel(CancelReason.stopped);
    }
    _calls.clear();
    for (final read in _reads.toList()) {
      if (read.consumed) continue;
      read.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_read_fail(read.ptr, k, m));
    }
    _reads.clear();
    _root._releaseTree();
    _b.am_client_free(_ptr);
    for (final id in _ownedIds) {
      _rt.targets.remove(id);
    }
    _ownedIds.clear();
    _rt.targets.remove(_userDataId);
    _states.close();
    _paired.close();
    _logs.close();
    _idleExit.close();
  }

  void _ensureAlive() {
    if (_disposed) throw AppMcpException(AppMcpErrorCode.stopped, '客户端已释放');
  }

  // ---- 原生回调（已在本 isolate 上） ----

  void _onNativeState(int status, int retryInMs, String? reason) {
    if (_disposed) return;
    // @why 状态回调签名没有 code（C ABI v6 只新增查询函数），投递到本 isolate 时再查询；
    // 回调异步投递，状态可能已再次变化，此时 code 反映更新后的状态（可能为 null）。
    String? code;
    if (statusHasCode(statusFromNative(status))) {
      try {
        code = _queryStateCode();
      } on AppMcpException {
        code = null; // @why 原生客户端已停止 / 释放中：状态照常送达，只缺 code
      }
    }
    _states.add(stateFromNative(status, retryInMs, reason, code: code));
  }

  void _onNativePaired(String token) {
    if (_disposed) return;
    _paired.add(token);
  }

  void _onNativeIdleExit() {
    if (_disposed) return;
    _idleExit.add(null);
  }

  void _onNativeLog(LogLevel level, String message) {
    if (_disposed || !_logs.hasListener) return;
    _logs.add(McpLogRecord(level, message));
  }

  void _dispatchCall(_ToolEntry entry, Pointer<AmCall> ptr) {
    final call = _PendingCall(
      _rt,
      ptr,
      _b.am_call_id(ptr).toDartString(),
      _b.am_call_tool_name(ptr).toDartString(),
    );
    final argsJson = _b.am_call_arguments_json(ptr).toDartString();
    if (_disposed) {
      call.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_call_fail(ptr, k, m));
      return;
    }
    _calls.add(call);
    final cancelId = _rt.register(call);
    call.cancelId = cancelId;
    final status = _b.am_call_set_cancel_callback(
        ptr, _rt.cancel.nativeFunction, Pointer<Void>.fromAddress(cancelId), _rt.free.nativeFunction);
    if (status != AmStatus.ok) _rt.targets.remove(cancelId);

    final handler = entry.handler;
    Future<Object?>.sync(() => handler(decodeArguments(argsJson), call)).then(
      (value) => _completeCall(call, value),
      onError: (Object e, StackTrace _) => _failCall(call, e),
    );
  }

  void _completeCall(_PendingCall call, Object? value) {
    if (call.consumed) return;
    final EncodedResult result;
    try {
      result = encodeResult(value);
    } catch (e) {
      _failCall(call, e);
      return;
    }
    final status = using((arena) {
      final data = result.dataJson.toNativeUtf8(allocator: arena);
      final hints = arena<Pointer<Utf8>>(result.stateHints.isEmpty ? 1 : result.stateHints.length);
      for (var i = 0; i < result.stateHints.length; i++) {
        hints[i] = result.stateHints[i].toNativeUtf8(allocator: arena);
      }
      if (!result.isStructured) return _b.am_call_complete(call.ptr, data, hints, result.stateHints.length);
      final r = arena<AmCallResult>();
      r.ref
        ..struct_size = sizeOf<AmCallResult>()
        ..data_json = data
        ..state_hints = hints
        ..state_hints_len = result.stateHints.length
        ..status = resultStatusToNative(result.status)
        ..state_resource = _optStr(result.stateResource, arena)
        ..summary = _optStr(result.summary, arena)
        ..annotations_json = _optStr(result.annotationsJson, arena);
      return _b.am_call_complete_ex(call.ptr, r);
    });
    if (status == AmStatus.invalidJson) {
      // 未被消费，改为失败完成。
      _failCall(call, ToolCallError(ErrorKind.handlerError, '返回值不是合法的 JSON'));
      return;
    }
    _finishCall(call);
  }

  void _failCall(_PendingCall call, Object error) {
    if (call.consumed) return;
    _failNative(call.ptr, error,
        fail: _b.am_call_fail,
        failWithDetails: _b.am_call_fail_with_details,
        failUserAction: _b.am_call_fail_user_action);
    _finishCall(call);
  }

  void _finishCall(_PendingCall call) {
    call.consumed = true;
    _calls.remove(call);
    _rt.targets.remove(call.cancelId);
  }

  void _dispatchRead(_ResourceEntry entry, Pointer<AmRead> ptr) {
    final read = _PendingRead(ptr);
    if (_disposed) {
      read.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_read_fail(ptr, k, m));
      return;
    }
    _reads.add(read);
    final reader = entry.reader;
    Future<Object?>.sync(reader).then((value) {
      if (read.consumed) return;
      final int status;
      try {
        final json = encodeJsonValue(value, '资源内容');
        status = using((arena) => _b.am_read_complete(ptr, json.toNativeUtf8(allocator: arena)));
      } catch (e) {
        _failRead(read, e);
        return;
      }
      if (status == AmStatus.invalidJson) {
        _failRead(read, ToolCallError(ErrorKind.handlerError, '资源内容不是合法的 JSON'));
        return;
      }
      read.consumed = true;
      _reads.remove(read);
    }, onError: (Object e, StackTrace _) => _failRead(read, e));
  }

  void _failRead(_PendingRead read, Object error) {
    if (read.consumed) return;
    _failNative(read.ptr, error,
        fail: _b.am_read_fail,
        failWithDetails: _b.am_read_fail_with_details,
        failUserAction: _b.am_read_fail_user_action);
    read.consumed = true;
    _reads.remove(read);
  }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// 作用域：注销时递归注销其下所有工具、资源与子作用域。
final class McpScope {
  McpScope._(this._client, this._ptr, this._parent, {this.isRoot = false});

  final AppMcp _client;
  final Pointer<AmScope> _ptr;
  final McpScope? _parent;
  final bool isRoot;
  bool _disposed = false;

  final Set<ToolHandle> _tools = {};
  final Set<ResourceHandle> _resources = {};
  final Set<McpScope> _children = {};

  AppMcp get client => _client;
  bool get isDisposed => _disposed || _client._disposed;

  /// 注册工具。名称重复、名称或 schema 非法时抛出 [AppMcpException]。
  ToolHandle tool(
    String name, {
    required String description,
    Map<String, Object?>? inputSchema,
    Risk risk = Risk.write,
    Activation? activation,
    String? title,
    bool enabled = true,
    ToolAnnotations? annotations,
    Map<String, Object?>? outputSchema,
    required ToolHandler handler,
  }) =>
      registerTool(
          ToolSpec(
              name: name,
              description: description,
              inputSchema: inputSchema,
              risk: risk,
              activation: activation,
              title: title,
              enabled: enabled,
              annotations: annotations,
              outputSchema: outputSchema),
          handler);

  /// 用 [ToolSpec] 注册工具。
  ToolHandle registerTool(ToolSpec spec, ToolHandler handler) {
    _ensureAlive();
    final rt = _client._rt;
    final entry = _ToolEntry(_client, handler);
    final id = rt.register(entry);
    final ptr = using((arena) {
      final s = _toolSpec(spec, arena);
      final options = _toolOptions(spec, arena);
      final out = arena<Pointer<AmTool>>();
      final status = rt.b.am_tool_register_ex(_ptr, s, options, rt.tool.nativeFunction,
          Pointer<Void>.fromAddress(id), rt.free.nativeFunction, out);
      if (status != AmStatus.ok) {
        final e = rt.error(status);
        rt.targets.remove(id);
        throw e;
      }
      return out.value;
    });
    _client._ownedIds.add(id);
    final handle = ToolHandle._(this, ptr, entry, spec);
    _tools.add(handle);
    return handle;
  }

  /// 注册资源。
  ///
  /// [realtime]：需实时推送（spec/lifecycle.md 第 13 节 B3）——被订阅时保持连接、休眠中变化时回连推送。
  /// 默认 false：订阅不阻止休眠，变化在下次连接时补发；只用于"模型在等待变化"的资源。
  ResourceHandle resource(String name,
      {required String description,
      String? mimeType,
      bool realtime = false,
      required ResourceReader read}) {
    _ensureAlive();
    final rt = _client._rt;
    final entry = _ResourceEntry(_client, read);
    final id = rt.register(entry);
    final ptr = using((arena) {
      final s = arena<AmResourceSpec>();
      s.ref
        ..name = name.toNativeUtf8(allocator: arena)
        ..description = description.toNativeUtf8(allocator: arena)
        ..mime_type = _optStr(mimeType, arena);
      final options = arena<AmResourceOptions>();
      options.ref
        ..struct_size = sizeOf<AmResourceOptions>()
        ..realtime = realtime;
      final out = arena<Pointer<AmResource>>();
      final status = rt.b.am_resource_register_ex(
          _ptr, s, options, rt.read.nativeFunction, Pointer<Void>.fromAddress(id), rt.free.nativeFunction, out);
      if (status != AmStatus.ok) {
        final e = rt.error(status);
        rt.targets.remove(id);
        throw e;
      }
      return out.value;
    });
    _client._ownedIds.add(id);
    final handle = ResourceHandle._(this, ptr, entry, name);
    _resources.add(handle);
    return handle;
  }

  /// 创建子作用域。
  McpScope scope(String name) {
    _ensureAlive();
    final rt = _client._rt;
    final ptr = using((arena) {
      final out = arena<Pointer<AmScope>>();
      rt.check(rt.b.am_scope_create(_ptr, name.toNativeUtf8(allocator: arena), out));
      return out.value;
    });
    final child = McpScope._(_client, ptr, this);
    _children.add(child);
    return child;
  }

  /// 注销该作用域下的全部工具、资源与子作用域并释放句柄。幂等。
  ///
  /// 对根作用域调用会注销全部工具与资源，但根作用域本身仍可继续注册。
  void dispose() {
    if (isDisposed) return;
    final rt = _client._rt;
    rt.b.am_scope_dispose(_ptr);
    if (isRoot) {
      for (final t in _tools.toList()) {
        t._release();
      }
      for (final r in _resources.toList()) {
        r._release();
      }
      for (final c in _children.toList()) {
        c._releaseTree();
      }
      return;
    }
    _releaseTree();
    _parent?._children.remove(this);
  }

  /// 释放本作用域及其后代的原生句柄（不再发送注销）。
  void _releaseTree() {
    if (_disposed) return;
    _disposed = true;
    for (final t in _tools.toList()) {
      t._release();
    }
    for (final r in _resources.toList()) {
      r._release();
    }
    for (final c in _children.toList()) {
      c._releaseTree();
    }
    _tools.clear();
    _resources.clear();
    _children.clear();
    _client._b.am_scope_free(_ptr);
  }

  void _ensureAlive() {
    _client._ensureAlive();
    if (_disposed) throw AppMcpException(AppMcpErrorCode.disposed, '作用域已注销');
  }
}

Pointer<AmToolSpec> _toolSpec(ToolSpec spec, Allocator arena) {
  final s = arena<AmToolSpec>();
  s.ref
    ..name = spec.name.toNativeUtf8(allocator: arena)
    ..description = spec.description.toNativeUtf8(allocator: arena)
    ..input_schema_json = _optStr(encodeSchema(spec.inputSchema), arena)
    ..risk = riskToNative(spec.risk)
    ..activation = activationToNative(spec.activation)
    ..title = _optStr(spec.title, arena)
    ..enabled = spec.enabled;
  return s;
}

/// v9：工具注解与 outputSchema（为 null 的字段不声明 / 清除）。
Pointer<AmToolOptions> _toolOptions(ToolSpec spec, Allocator arena) {
  final o = arena<AmToolOptions>();
  o.ref
    ..struct_size = sizeOf<AmToolOptions>()
    ..annotations_json = _optStr(encodeToolAnnotations(spec.annotations), arena)
    ..output_schema_json = _optStr(encodeSchema(spec.outputSchema), arena);
  return o;
}

// ---------------------------------------------------------------------------
// 句柄
// ---------------------------------------------------------------------------

/// [ToolHandle.update] 的缺省标记类型（区分“未提供”与 null）。
final class _KeepField {
  const _KeepField();
}

/// 已注册的工具。
final class ToolHandle {
  ToolHandle._(this._scope, this._ptr, this._entry, this._spec);

  final McpScope _scope;
  final Pointer<AmTool> _ptr;
  final _ToolEntry _entry;
  ToolSpec _spec;
  bool _released = false;

  String get name => _spec.name;
  ToolSpec get spec => _spec;
  bool get isDisposed => _released || _scope._client._disposed;

  /// 更新定义：未提供的字段保持不变；显式传 null 清除该声明（与 @app-mcp/web 一致）。
  ///
  /// 各参数类型同 [ToolSpec] 对应字段（`description` String、`inputSchema` / `outputSchema`
  /// `Map<String, Object?>`、`risk` [Risk]、`activation` [Activation]、`title` String、`enabled` bool、
  /// `annotations` [ToolAnnotations]）。null 的含义：`title` / `activation` / `annotations` /
  /// `outputSchema` 清除声明，`inputSchema` 为无参数，`risk` 恢复 [Risk.write]，`enabled` 恢复 true，
  /// `description` 保持不变。
  ///
  /// @error 类型不符时抛 [ArgumentError]，不产生协议消息。
  /// @compat 参数声明为 `Object?` 以区分“未提供”与 null；原有按类型传值的调用不受影响。
  void update({
    Object? description = _keep,
    Object? inputSchema = _keep,
    Object? risk = _keep,
    Object? activation = _keep,
    Object? title = _keep,
    Object? enabled = _keep,
    Object? annotations = _keep,
    Object? outputSchema = _keep,
  }) {
    final s = _spec;
    replace(ToolSpec(
        name: s.name,
        description: _patch<String?>(description, s.description, 'description') ?? s.description,
        inputSchema: _patch<Map<String, Object?>?>(inputSchema, s.inputSchema, 'inputSchema'),
        risk: _patch<Risk?>(risk, s.risk, 'risk') ?? Risk.write,
        activation: _patch<Activation?>(activation, s.activation, 'activation'),
        title: _patch<String?>(title, s.title, 'title'),
        enabled: _patch<bool?>(enabled, s.enabled, 'enabled') ?? true,
        annotations: _patch<ToolAnnotations?>(annotations, s.annotations, 'annotations'),
        outputSchema: _patch<Map<String, Object?>?>(outputSchema, s.outputSchema, 'outputSchema')));
  }

  /// [update] 参数缺省标记。
  static const Object _keep = _KeepField();

  /// @output 未提供（[_keep]）时为 [current]，否则为 [value]（null 或 [T]）。
  /// @error [value] 不是 [T] 时抛 [ArgumentError]。
  static T _patch<T>(Object? value, T current, String field) {
    if (identical(value, _keep)) return current;
    if (value is T) return value;
    throw ArgumentError.value(value, field, '类型应为 $T');
  }

  /// 用新定义整体替换（名称不可变，`spec.name` 被忽略；为 null 的 annotations / outputSchema 表示清除该声明）。
  /// 与当前定义相同时不做任何事。
  void replace(ToolSpec spec) {
    _ensureAlive();
    final next = ToolSpec(
        name: _spec.name,
        description: spec.description,
        inputSchema: spec.inputSchema,
        risk: spec.risk,
        activation: spec.activation,
        title: spec.title,
        enabled: spec.enabled,
        annotations: spec.annotations,
        outputSchema: spec.outputSchema);
    if (next == _spec) return;
    final rt = _scope._client._rt;
    using((arena) =>
        rt.check(rt.b.am_tool_update_ex(_ptr, _toolSpec(next, arena), _toolOptions(next, arena))));
    _spec = next;
  }

  void setEnabled(bool enabled) {
    _ensureAlive();
    if (enabled == _spec.enabled) return;
    final rt = _scope._client._rt;
    rt.check(rt.b.am_tool_set_enabled(_ptr, enabled));
    _spec = _spec.copyWith(enabled: enabled);
  }

  /// 替换 handler（不产生协议消息，供框架适配在重建时刷新闭包）。
  void setHandler(ToolHandler handler) {
    _entry.handler = handler;
  }

  /// 注销工具并释放句柄。幂等。
  void dispose() {
    if (isDisposed) return;
    _scope._client._b.am_tool_dispose(_ptr);
    _release();
  }

  void _release() {
    if (_released) return;
    _released = true;
    _entry.disposed = true;
    _scope._tools.remove(this);
    _scope._client._b.am_tool_free(_ptr);
  }

  void _ensureAlive() {
    if (isDisposed) throw AppMcpException(AppMcpErrorCode.disposed, '工具已注销');
  }
}

/// 已注册的资源。
final class ResourceHandle {
  ResourceHandle._(this._scope, this._ptr, this._entry, this.name);

  final McpScope _scope;
  final Pointer<AmResource> _ptr;
  final _ResourceEntry _entry;
  final String name;
  bool _released = false;

  bool get isDisposed => _released || _scope._client._disposed;

  /// 通知 Host 资源内容已变化。
  void notifyChanged() {
    if (isDisposed) throw AppMcpException(AppMcpErrorCode.disposed, '资源已注销');
    final rt = _scope._client._rt;
    rt.check(rt.b.am_resource_notify_changed(_ptr));
  }

  /// 替换读取函数（不产生协议消息）。
  void setReader(ResourceReader read) {
    _entry.reader = read;
  }

  /// 注销资源并释放句柄。幂等。
  void dispose() {
    if (isDisposed) return;
    _scope._client._b.am_resource_dispose(_ptr);
    _release();
  }

  void _release() {
    if (_released) return;
    _released = true;
    _entry.disposed = true;
    _scope._resources.remove(this);
    _scope._client._b.am_resource_free(_ptr);
  }
}
