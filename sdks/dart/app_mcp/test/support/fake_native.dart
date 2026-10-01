// 编译并加载 test/fake_native/fake_app_mcp.c（Linux / macOS 用 cc；Windows 用 MSVC 生成工具）。
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// 编译假库，返回路径；没有可用的 C 编译器时返回 null。
String? buildFakeLibrary() {
  final src = File('test/fake_native/fake_app_mcp.c').absolute.path;
  final dir = Directory.systemTemp.createTempSync('app_mcp_fake_');
  if (Platform.isWindows) return _buildWithMsvc(src, dir);
  if (!Platform.isLinux && !Platform.isMacOS) return null;
  final out = '${dir.path}/${Platform.isMacOS ? 'libfake_app_mcp.dylib' : 'libfake_app_mcp.so'}';
  return _run('cc', ['-shared', '-fPIC', '-o', out, src, '-lpthread']) ? out : null;
}

/// 运行命令，成功时返回 stdout；失败时把输出写到 stderr 并返回 null。
String? _runOutput(String exe, List<String> args, {String? workingDirectory}) {
  try {
    final r = Process.runSync(exe, args, workingDirectory: workingDirectory);
    if (r.exitCode == 0) return r.stdout as String;
    stderr.writeln('编译假库失败（$exe ${args.join(' ')}）：${r.stdout}${r.stderr}');
  } on ProcessException {
    // 工具不存在。
  }
  return null;
}

bool _run(String exe, List<String> args) => _runOutput(exe, args) != null;

/// Windows：经 vswhere 找到 VS（含生成工具）的 vcvars64.bat，在其环境中用 cl / dumpbin / link 生成 DLL。
///
/// @why MSVC 不像 cc 那样默认导出全部非 static 函数，而 app_mcp.h 的声明不带 dllexport（定义处再加会
///      报 C2375）。因此先 `cl /c` 编译，从 `dumpbin /symbols` 取出已定义的 am_* / fake_* 外部函数生成 .def，再 `link /DLL`，
///      导出集合与 Linux 一致，源文件无需维护导出表。
String? _buildWithMsvc(String src, Directory dir) {
  final vcvars = _findVcvars64();
  if (vcvars == null) return null;
  // cl.exe / link.exe 依赖 vcvars 设置的 INCLUDE / LIB / PATH：每步写一个 .cmd，先 call 再执行。
  // @why 脚本名加前缀：cmd 先在当前目录查找命令，名为 link.cmd 会被其中的 link 递归调用。
  String? msvc(String name, String command) {
    final script = File('${dir.path}\\step_$name.cmd')
      ..writeAsStringSync('@echo off\r\ncall "$vcvars" >nul || exit /b 1\r\n$command\r\n');
    return _runOutput('cmd.exe', ['/c', script.path], workingDirectory: dir.path);
  }

  // /utf-8：源文件含中文注释（UTF-8 无 BOM）。
  if (msvc('compile', 'cl /nologo /c /utf-8 /W3 /Fo:fake_app_mcp.obj "$src"') == null) return null;
  final symbols = msvc('symbols', 'dumpbin /nologo /symbols fake_app_mcp.obj');
  if (symbols == null) return null;
  // 形如 `01A 00000000 SECT5  notype ()    External     | am_version`：已定义（SECTn）的外部函数。
  final exported = RegExp(r'^\S+ \S+ SECT\w+\s+notype \(\)\s+External\s+\| (\w+)\s*$', multiLine: true)
      .allMatches(symbols)
      .map((m) => m.group(1)!)
      // 只导出 C ABI（am_*）与测试驱动（fake_*）；跳过 CRT 头文件中的内联函数（snprintf 等）。
      .where((name) => name.startsWith('am_') || name.startsWith('fake_'))
      .toList();
  if (exported.isEmpty) {
    stderr.writeln('编译假库失败：dumpbin 未列出任何外部函数');
    return null;
  }
  File('${dir.path}\\fake_app_mcp.def').writeAsStringSync('EXPORTS\r\n${exported.join('\r\n')}\r\n');
  final out = '${dir.path}\\fake_app_mcp.dll';
  final linked =
      msvc('link', 'link /nologo /DLL /DEF:fake_app_mcp.def "/OUT:$out" fake_app_mcp.obj');
  return linked == null ? null : out;
}

/// 用 vswhere 找含 x64 C++ 工具的最新 VS 的 vcvars64.bat；找不到时返回 null。
String? _findVcvars64() {
  final programFiles = Platform.environment['ProgramFiles(x86)'] ?? r'C:\Program Files (x86)';
  final vswhere = '$programFiles\\Microsoft Visual Studio\\Installer\\vswhere.exe';
  if (!File(vswhere).existsSync()) return null;
  final found = _runOutput(vswhere, [
    '-latest', '-products', '*',
    '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    '-find', r'VC\Auxiliary\Build\vcvars64.bat',
  ]);
  final first = found?.trim().split(RegExp(r'\r?\n')).first.trim();
  return first == null || first.isEmpty || !File(first).existsSync() ? null : first;
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
  late final _resourceRealtime = lib.lookupFunction<Int32 Function(Pointer<Utf8>), int Function(Pointer<Utf8>)>(
      'fake_resource_realtime');

  /// 资源注册时的 realtime：1 / 0；找不到时 -1。
  int resourceRealtime(String name) => using((a) => _resourceRealtime(name.toNativeUtf8(allocator: a)));
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

  /// `mode|idle|hidden|grace|residency|wakeKind|target|background|connectTimeout|heartbeat|hostAbsentRetries|legacyTimers|mergeWindowMs|sleepOnBackground`。
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
