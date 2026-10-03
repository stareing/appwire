// 编译 app_mcp 的假原生库（../app_mcp/test/fake_native/*.c），供本包的 widget 测试使用。
import 'dart:io';

/// 编译假库，返回路径；非 Linux / macOS 或没有 cc 时返回 null（相关测试跳过）。
String? buildFakeLibrary() {
  if (!Platform.isLinux && !Platform.isMacOS) return null;
  final sources = Directory('../app_mcp/test/fake_native')
      .listSync()
      .whereType<File>()
      .where((f) => f.path.endsWith('.c'))
      .map((f) => f.absolute.path)
      .toList();
  final dir = Directory.systemTemp.createTempSync('app_mcp_flutter_fake_');
  final out = '${dir.path}/libfake_app_mcp${Platform.isMacOS ? '.dylib' : '.so'}';
  try {
    final r = Process.runSync('cc', ['-shared', '-fPIC', '-o', out, ...sources, '-lpthread']);
    return r.exitCode == 0 ? out : null;
  } on ProcessException {
    return null;
  }
}
