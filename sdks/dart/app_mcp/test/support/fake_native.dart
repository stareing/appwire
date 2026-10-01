// 编译并加载 test/fake_native/fake_app_mcp.c（需要 cc）。
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// 编译假库，返回路径；没有 C 编译器时返回 null。
String? buildFakeLibrary() {
  if (!Platform.isLinux && !Platform.isMacOS) return null;
  final src = File('test/fake_native/fake_app_mcp.c').absolute.path;
  final dir = Directory.systemTemp.createTempSync('app_mcp_fake_');
  final out = '${dir.path}/${Platform.isMacOS ? 'libfake_app_mcp.dylib' : 'libfake_app_mcp.so'}';
  try {
    final r = Process.runSync('cc', ['-shared', '-fPIC', '-o', out, src, '-lpthread']);
    if (r.exitCode != 0) {
      stderr.writeln('编译假库失败：${r.stderr}');
      return null;
    }
  } on ProcessException {
    return null;
  }
  return out;
}

/// 假库额外导出的测试驱动函数。
final class FakeNative {
  FakeNative(String path) : lib = DynamicLibrary.open(path);
  final DynamicLibrary lib;

  late final _invoke = lib.lookupFunction<Int32 Function(Pointer<Utf8>, Pointer<Utf8>),
      int Function(Pointer<Utf8>, Pointer<Utf8>)>('fake_invoke');
  late final _read = lib.lookupFunction<Int32 Function(Pointer<Utf8>), int Function(Pointer<Utf8>)>(
      'fake_read');
  late final cancel =
      lib.lookupFunction<Int32 Function(Int32, Int32), int Function(int, int)>('fake_cancel');
  late final _result = lib.lookupFunction<Pointer<Utf8> Function(Int32), Pointer<Utf8> Function(int)>(
      'fake_result');
  late final _stringFree = lib
      .lookupFunction<Void Function(Pointer<Utf8>), void Function(Pointer<Utf8>)>('am_string_free');
  late final _emitState = lib.lookupFunction<Void Function(Int32, Uint64, Pointer<Utf8>),
      void Function(int, int, Pointer<Utf8>)>('fake_emit_state');
  late final _emitStateCode = lib.lookupFunction<
      Void Function(Int32, Uint64, Pointer<Utf8>, Pointer<Utf8>),
      void Function(int, int, Pointer<Utf8>, Pointer<Utf8>)>('fake_emit_state_code');
  late final _pair =
      lib.lookupFunction<Void Function(Pointer<Utf8>), void Function(Pointer<Utf8>)>('fake_pair');
  late final freeCount = lib.lookupFunction<Int32 Function(), int Function()>('fake_free_count');
  late final visibility = lib.lookupFunction<Int32 Function(), int Function()>('fake_visibility');
  late final focused = lib.lookupFunction<Int32 Function(), int Function()>('fake_focused');
  late final notifyCount = lib.lookupFunction<Int32 Function(), int Function()>('fake_notify_count');
  late final _toolEnabled = lib.lookupFunction<Int32 Function(Pointer<Utf8>), int Function(Pointer<Utf8>)>(
      'fake_tool_enabled');
  late final _toolDescription = lib.lookupFunction<Pointer<Utf8> Function(Pointer<Utf8>),
      Pointer<Utf8> Function(Pointer<Utf8>)>('fake_tool_description');

  late final _log = lib.lookupFunction<Void Function(Int32, Pointer<Utf8>),
      void Function(int, Pointer<Utf8>)>('fake_log');
  late final ownedOutstanding =
      lib.lookupFunction<Int32 Function(), int Function()>('fake_owned_outstanding');
  late final ownedTotal = lib.lookupFunction<Int32 Function(), int Function()>('fake_owned_total');

  late final idleExit = lib.lookupFunction<Void Function(), void Function()>('fake_idle_exit');
  late final holdCount = lib.lookupFunction<Int32 Function(), int Function()>('fake_hold_count');
  late final _lifecycle =
      lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('fake_lifecycle');
  late final _lastSleep =
      lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('fake_last_sleep');

  String? _take(Pointer<Utf8> p) {
    if (p == nullptr) return null;
    final s = p.toDartString();
    _stringFree(p);
    return s;
  }

  /// `mode|idle|hidden|grace|residency|wakeKind|target|background|connectTimeout`。
  String? lifecycle() => _take(_lifecycle());
  String? lastSleep() => _take(_lastSleep());

  void log(int level, String message) => using((a) => _log(level, message.toNativeUtf8(allocator: a)));

  late final sizeOf = lib.lookupFunction<Size Function(Int32), int Function(int)>('fake_sizeof');
  late final _overview =
      lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('fake_overview');

  String? overview() {
    final p = _overview();
    if (p == nullptr) return null;
    final s = p.toDartString();
    _stringFree(p);
    return s;
  }

  int invoke(String tool, String argsJson) =>
      using((a) => _invoke(tool.toNativeUtf8(allocator: a), argsJson.toNativeUtf8(allocator: a)));

  int read(String name) => using((a) => _read(name.toNativeUtf8(allocator: a)));

  String? result(int idx) {
    final p = _result(idx);
    if (p == nullptr) return null;
    final s = p.toDartString();
    _stringFree(p);
    return s;
  }

  /// 轮询等待结果。
  Future<String> waitResult(int idx, {Duration timeout = const Duration(seconds: 5)}) async {
    final deadline = DateTime.now().add(timeout);
    while (DateTime.now().isBefore(deadline)) {
      final r = result(idx);
      if (r != null) return r;
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }
    throw StateError('等待结果 $idx 超时');
  }

  void emitState(int status, int retry, String? reason) => using(
      (a) => _emitState(status, retry, reason == null ? nullptr : reason.toNativeUtf8(allocator: a)));

  /// 发出状态变化，并设置之后 am_client_state_code 返回的错误码。
  void emitStateCode(int status, int retry, String? reason, String? code) => using((a) => _emitStateCode(
      status,
      retry,
      reason == null ? nullptr : reason.toNativeUtf8(allocator: a),
      code == null ? nullptr : code.toNativeUtf8(allocator: a)));

  void pair(String token) => using((a) => _pair(token.toNativeUtf8(allocator: a)));

  int toolEnabled(String name) => using((a) => _toolEnabled(name.toNativeUtf8(allocator: a)));

  String? toolDescription(String name) {
    final p = using((a) => _toolDescription(name.toNativeUtf8(allocator: a)));
    if (p == nullptr) return null;
    final s = p.toDartString();
    _stringFree(p);
    return s;
  }
}

/// 等待条件成立。
Future<void> eventually(bool Function() cond, {Duration timeout = const Duration(seconds: 5)}) async {
  final deadline = DateTime.now().add(timeout);
  while (!cond()) {
    if (DateTime.now().isAfter(deadline)) throw StateError('条件未在 $timeout 内成立');
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}
