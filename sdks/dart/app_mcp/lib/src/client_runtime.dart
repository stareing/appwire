// 运行时：共享回调、注册表与字符串辅助（client.dart 的 part）。

part of 'client.dart';

// ---------------------------------------------------------------------------
// 运行时：共享回调与注册表
// ---------------------------------------------------------------------------

final class _Runtime {
  _Runtime(this.b) {
    for (final c in <NativeCallable<Function>>[tool, read, cancel, state, paired, log, free, idleExit, navigate]) {
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
  late final NativeCallable<AmNavigateFnNative> navigate =
      NativeCallable<AmNavigateFnNative>.listener(_onNavigate);

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

  /// user_data 为客户端自身的 ID（导航回调存在 Dart 侧，不随 user_data 释放）。
  void _onNavigate(Pointer<Void> userData, Pointer<AmNavigate> navigate) {
    final target = targets[userData.address];
    if (target is AppMcp) {
      target._dispatchNavigate(navigate);
    } else {
      using((arena) => b.am_navigate_fail(navigate, '客户端已释放'.toNativeUtf8(allocator: arena)));
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

/// 读取库持有的字符串（不释放）；NULL 为 null。
String? _borrowedString(Pointer<Utf8> p) => p == nullptr ? null : p.toDartString();

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
  _PendingCall(this.rt, this.ptr, this.callId, this.toolName, this.idempotencyKey);

  final _Runtime rt;
  AppMcpBindings get b => rt.b;
  Pointer<AmCall> ptr;
  @override
  final String callId;
  @override
  final String toolName;
  @override
  final String? idempotencyKey;
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

final class _PendingNavigate {
  _PendingNavigate(this.ptr);
  final Pointer<AmNavigate> ptr;
  bool consumed = false;
}
