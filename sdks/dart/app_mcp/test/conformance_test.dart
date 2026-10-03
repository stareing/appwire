// 一致性用例 runner（Dart）：按 conformance/cases/*.json 的 app 部分注册工具与资源，连接 fake_host
// （--case 模式，核对在 fake_host 内完成），每个用例一个 test。格式与约定见 conformance/README.md，
// 结构对照 crates/native/tests/conformance.rs。
//
// 先构建：cargo build -p app-mcp-c；fake_host 由本测试用 cargo 构建（APP_MCP_FAKE_HOST 可跳过构建）。
// 只跑部分用例：APP_MCP_CONFORMANCE_CASES=handshake,errors dart test test/conformance_test.dart
@Tags(['integration'])
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:app_mcp/app_mcp.dart';
import 'package:test/test.dart';

const _sdk = 'dart';

/// 本 runner 支持的用例能力（`requires`），见 conformance/README.md 第 4 节。
const _features = {
  'toolOptions', 'mutate', 'lifecycle', 'wake', 'richResult', 'userAction', 'progress', 'resourceOptions', //
  'readFailure', 'surface', 'navigation', 'backgroundTool', 'backgroundNavigation', 'idempotencyKey',
  'callScheduling',
};

final String _repoRoot = Directory('${Directory.current.path}/../../..').absolute.path;

String get _targetDir => Platform.environment['CARGO_TARGET_DIR'] ?? '$_repoRoot/target';

String? _existing(String? path) => path != null && File(path).existsSync() ? path : null;

final String? _nativePath = _existing(Platform.environment['APP_MCP_NATIVE_PATH']) ??
    _existing('$_targetDir/debug/${defaultNativeLibraryName()}');

/// @why 与 integration_test.dart 相同：每次先构建 fake_host，避免用到旧版本（核对逻辑在 fake_host 内）。
String? _buildFakeHost() {
  final ProcessResult r;
  try {
    r = Process.runSync('cargo', ['build', '-q', '-p', 'app-mcp-native', '--example', 'fake_host'],
        workingDirectory: _repoRoot, environment: {'CARGO_TARGET_DIR': _targetDir});
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

final String? _fakeHostPath = _existing(Platform.environment['APP_MCP_FAKE_HOST']) ?? _buildFakeHost();

// ---- 用例字段 → SDK 类型（协议同名字符串） ----------------------------------------

T? _byWire<T extends Enum>(List<T> values, Object? wire, String Function(T) name) {
  for (final v in values) {
    if (name(v) == wire) return v;
  }
  return null;
}

Risk _risk(Object? v) => _byWire(Risk.values, v, (r) => r.wireName) ?? Risk.write;
Activation? _activation(Object? v) => _byWire(Activation.values, v, (a) => a.wireName);
ToolResultStatus _status(Object? v) => _byWire(ToolResultStatus.values, v, (s) => s.wireName) ?? ToolResultStatus.done;
ErrorKind _errorKind(Object? v) => _byWire(ErrorKind.values, v, (k) => k.wireName) ?? ErrorKind.handlerError;

LifecycleMode _mode(Object? v) => switch (v) {
      'idle' => LifecycleMode.idle,
      'on-demand' => LifecycleMode.onDemand,
      _ => LifecycleMode.persistent,
    };

Map<String, Object?>? _map(Object? v) => v is Map ? v.cast<String, Object?>() : null;

ToolAnnotations? _toolAnnotations(Object? v) {
  final m = _map(v);
  if (m == null) return null;
  return ToolAnnotations(
      title: m['title'] as String?,
      readOnlyHint: m['readOnlyHint'] as bool?,
      destructiveHint: m['destructiveHint'] as bool?,
      idempotentHint: m['idempotentHint'] as bool?,
      openWorldHint: m['openWorldHint'] as bool?);
}

ContentAnnotations? _contentAnnotations(Object? v) {
  final m = _map(v);
  if (m == null) return null;
  final audience = m['audience'] as List?;
  return ContentAnnotations(
      audience: audience == null
          ? null
          : [for (final a in audience) _byWire(ContentAudience.values, a, (c) => c.wireName)!],
      priority: (m['priority'] as num?)?.toDouble(),
      lastModified: m['lastModified'] as String?);
}

Duration? _ms(Object? v) => v is num ? Duration(milliseconds: v.toInt()) : null;

/// 一个用例的 App：客户端与已注册工具（mutate 用）。
final class _CaseApp {
  _CaseApp(this.client);

  final AppMcp client;
  final Map<String, ToolHandle> _tools = {};

  void registerTool(Map<String, Object?> decl) {
    final spec = _map(decl['handler']) ?? const {};
    var runs = 0;
    final name = decl['name'] as String;
    _tools[name] = client.tool(name,
        description: decl['description'] as String,
        inputSchema: _map(decl['inputSchema']),
        risk: _risk(decl['risk']),
        activation: _activation(decl['activation']),
        title: decl['title'] as String?,
        enabled: decl['enabled'] as bool? ?? true,
        annotations: _toolAnnotations(decl['annotations']),
        outputSchema: _map(decl['outputSchema']),
        surface: decl['surface'] == 'view' ? ToolSurface.view : ToolSurface.app,
        page: decl['page'] as String?,
        backgroundTool: decl['backgroundTool'] as String?,
        concurrency: (decl['concurrency'] as num?)?.toInt() ?? 0,
        exclusive: decl['exclusive'] as String?,
        handler: (args, ctx) => _runHandler(spec, ++runs, args, ctx));
  }

  void registerResource(Map<String, Object?> decl) {
    final spec = _map(decl['read']) ?? const {};
    client.resource(decl['name'] as String,
        description: decl['description'] as String,
        mimeType: decl['mimeType'] as String?,
        realtime: decl['realtime'] as bool? ?? false,
        annotations: _contentAnnotations(decl['annotations']),
        read: () => _read(spec));
  }

  /// 导航行为（conformance/README.md 2.4）。Dart 最自然的写法：正常返回 = 完成，抛 [NavigationDeniedError] = 拒绝，
  /// 抛 [UserActionRequiredError] = USER_ACTION_REQUIRED，其他异常 = 失败。
  void setNavigation(Map<String, Object?> pages) {
    client.setNavigationHandler((request) {
      final spec = _map(pages[request.page]);
      if (spec == null) throw StateError('未知页面：${request.page}');
      if (spec['throw'] case final String message) throw Exception(message);
      for (final op in (spec['mutate'] as List?) ?? const []) {
        _mutate(_map(op)!);
      }
      if (spec['deny'] case final String message) throw NavigationDeniedError(message);
      if (spec['fail'] case final String message) throw StateError(message);
      if (_map(spec['userAction']) case final u?) {
        throw UserActionRequiredError(u['message'] as String? ?? '', reason: u['reason'] as String?, uri: u['uri'] as String?);
      }
      if (spec['failParams'] == true) throw StateError(request.paramsJson ?? '');
    });
  }

  /// 按 handler 描述执行（顺序：progress → delayMs → mutate → 结果，见 conformance/README.md 2.1）。
  Future<Object?> _runHandler(Map<String, Object?> spec, int count, Map<String, dynamic> args, ToolContext ctx) async {
    for (final p in (spec['progress'] as List?) ?? const []) {
      final m = _map(p)!;
      ctx.progress((m['progress'] as num).toDouble(),
          total: (m['total'] as num?)?.toDouble(), message: m['message'] as String?);
    }
    if (spec['delayMs'] case final num ms) {
      // 被取消 / 超时后提前结束；之后的完成结果由 SDK 丢弃。
      await Future.any([Future<void>.delayed(Duration(milliseconds: ms.toInt())), ctx.cancelled]);
    }
    for (final op in (spec['mutate'] as List?) ?? const []) {
      _mutate(_map(op)!);
    }
    return _complete(spec, count, args, ctx);
  }

  /// Dart 最自然的写法：失败抛异常；无返回值即 handler 不返回（null）。
  static Object? _complete(Map<String, Object?> spec, int count, Map<String, dynamic> args, ToolContext ctx) {
    if (spec['throw'] case final String message) throw Exception(message);
    if (_map(spec['userAction']) case final u?) {
      throw UserActionRequiredError(u['message'] as String? ?? '', reason: u['reason'] as String?, uri: u['uri'] as String?);
    }
    if (_map(spec['result']) case final r?) {
      return ToolResult(r['data'],
          stateHints: [for (final h in (r['stateHints'] as List?) ?? const []) h as String],
          status: _status(r['status']),
          stateResource: r['stateResource'] as String?,
          summary: r['summary'] as String?,
          annotations: _contentAnnotations(r['annotations']));
    }
    if (spec.containsKey('return')) return spec['return'];
    if (spec['echo'] == true) return args;
    if (spec['returnIdempotencyKey'] == true) return {'idempotencyKey': ctx.idempotencyKey};
    if (spec['counter'] == true) return {'count': count};
    // returnNothing（以及未声明结果）：Dart 的"无返回值"即 handler 不返回值（null）。
    return null;
  }

  static Object? _read(Map<String, Object?> spec) {
    if (spec.containsKey('return')) return spec['return'];
    if (_map(spec['fail']) case final f?) {
      throw ToolCallError(_errorKind(f['kind']), f['message'] as String? ?? '', details: f['details']);
    }
    if (_map(spec['userAction']) case final u?) {
      throw UserActionRequiredError(u['message'] as String? ?? '', reason: u['reason'] as String?, uri: u['uri'] as String?);
    }
    throw Exception(spec['throw'] as String? ?? '读取失败');
  }

  /// `update` 的字段 → [ToolHandle.update] 的命名参数（补丁型 API：null 直接传，表示清除声明）。
  static Object? _updateValue(String field, Object? v) => switch (field) {
        'risk' => v == null ? null : _risk(v),
        'activation' => _activation(v),
        'annotations' => _toolAnnotations(v),
        'inputSchema' || 'outputSchema' => _map(v),
        'surface' => v == null ? null : (v == 'view' ? ToolSurface.view : ToolSurface.app),
        _ => v,
      };

  /// handler 的 `mutate` 操作（conformance/README.md 2.3）。
  void _mutate(Map<String, Object?> op) {
    final name = op['name'] as String? ?? '';
    switch (op['op']) {
      case 'register':
        registerTool(_map(op['tool'])!);
      case 'update':
        final set = _map(op['set']) ?? const {};
        Function.apply(_tools[name]!.update, const [], {
          for (final e in set.entries) Symbol(e.key): _updateValue(e.key, e.value),
        });
      case 'remove':
        _tools.remove(name)?.dispose();
      case 'enable' || 'disable':
        _tools[name]!.setEnabled(op['op'] == 'enable');
      default:
        throw StateError('未知的 mutate 操作 ${op['op']}');
    }
  }
}

AppMcp _client(String addr, Map<String, Object?> c) {
  final tcp = addr.contains(':') && !addr.startsWith('unix:') && !addr.startsWith('pipe:');
  final l = _map(c['lifecycle']) ?? const {};
  const defaults = LifecyclePolicy();
  final d = _map(c['callDedup']) ?? const {};
  const dedupDefaults = CallDedupPolicy();
  return AppMcp(
    appId: 'conf',
    appName: 'Conformance',
    hostUrl: tcp ? 'ws://$addr/app' : addr,
    libraryPath: _nativePath,
    maxConcurrentCalls: (c['maxConcurrentCalls'] as num?)?.toInt() ?? 1,
    maxQueuedCalls: (c['maxQueuedCalls'] as num?)?.toInt() ?? 64,
    lifecycle: LifecyclePolicy(
      mode: _mode(l['mode']),
      idleTimeout: _ms(l['idleTimeoutMs']) ?? defaults.idleTimeout,
      grace: _ms(l['graceMs']) ?? defaults.grace,
      mergeWindow: _ms(l['mergeWindowMs']) ?? defaults.mergeWindow,
    ),
    callDedup: CallDedupPolicy(
        ttl: _ms(d['ttlMs']) ?? dedupDefaults.ttl,
        maxEntries: (d['maxEntries'] as num?)?.toInt() ?? dedupDefaults.maxEntries),
    navigateInBackground: c['navigateInBackground'] as bool?,
  );
}

/// 跑一个用例，返回 fake_host 给出的结论行。
Future<Map<String, Object?>> _runCase(File path, String reportDir) async {
  final kase = jsonDecode(await path.readAsString()) as Map<String, Object?>;
  final missing = [for (final f in (kase['requires'] as List?) ?? const []) if (!_features.contains(f)) f as String];
  final process = await Process.start(_fakeHostPath!, [
    '--case', path.path, '--sdk', _sdk, '--report-dir', reportDir, //
    if (missing.isNotEmpty) ...['--skip', 'runner 不支持：${missing.join(', ')}'],
  ]);
  process.stderr.transform(utf8.decoder).listen((s) => stderr.write('[fake_host] $s'));

  final app = _map(kase['app']) ?? const {};
  _CaseApp? caseApp;
  Map<String, Object?> verdict = {'status': 'error', 'failures': ['fake_host 没有输出结论']};
  try {
    await for (final line in process.stdout.transform(utf8.decoder).transform(const LineSplitter())) {
      if (line.startsWith('LISTENING ')) {
        final a = _CaseApp(_client(line.substring('LISTENING '.length).trim(), _map(app['config']) ?? const {}));
        caseApp = a;
        for (final t in (app['tools'] as List?) ?? const []) {
          a.registerTool(_map(t)!);
        }
        for (final r in (app['resources'] as List?) ?? const []) {
          a.registerResource(_map(r)!);
        }
        if (_map(app['navigation']) case final pages?) a.setNavigation(pages);
        if (app['visibility'] case final String v) a.client.setVisibility(AppVisibility.values.byName(v), focused: false);
        a.client.start();
        continue;
      }
      if (!line.startsWith('{')) continue;
      final v = jsonDecode(line) as Map<String, Object?>;
      switch (v['type']) {
        case 'wake':
          caseApp?.client.handleWake(v['arg'] as String? ?? '');
        case 'verdict':
          verdict = v;
      }
    }
    final code = await process.exitCode;
    if (code != 0 && verdict['status'] != 'fail') {
      verdict = {'status': 'error', 'failures': ['fake_host 退出码 $code']};
    }
  } finally {
    process.kill();
    caseApp?.client.dispose();
  }
  return verdict;
}

void main() {
  final skip = _nativePath == null
      ? '找不到原生库（先 cargo build -p app-mcp-c）'
      : _fakeHostPath == null
          ? '找不到 fake_host（cargo build -p app-mcp-native --example fake_host 失败，见 stderr）'
          : false;
  final only = Platform.environment['APP_MCP_CONFORMANCE_CASES']?.split(',');
  final cases = Directory('$_repoRoot/conformance/cases')
      .listSync()
      .whereType<File>()
      .where((f) => f.path.endsWith('.json'))
      .where((f) => only == null || only.contains(f.uri.pathSegments.last.replaceAll('.json', '')))
      .toList()
    ..sort((a, b) => a.path.compareTo(b.path));
  final reportDir = '$_repoRoot/target/conformance';

  for (final path in cases) {
    final id = path.uri.pathSegments.last.replaceAll('.json', '');
    test('conformance $id', () async {
      final v = await _runCase(path, reportDir);
      final status = v['status'];
      stdout.writeln('[$_sdk] ${id.padRight(24)} $status');
      expect(status, isIn(['pass', 'xfail', 'xpass', 'skip']), reason: jsonEncode(v['failures']));
    }, skip: skip);
  }
}
