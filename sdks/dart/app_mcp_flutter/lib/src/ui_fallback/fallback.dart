// 进程内控件兜底的注册与门控（spec/ui-fallback.md，第 4c 项 H）。

import 'dart:async';

import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';

import '../view.dart' show mcpNextFrame;
import 'inspector.dart';
import 'semantics_tree.dart';

final RegExp _refPattern = RegExp(r'^e[1-9]\d*$');

/// 可见的应用生命周期状态（有可见窗口）。`null`（尚未上报）按可见处理。
const Set<AppLifecycleState?> _visibleStates = {null, AppLifecycleState.resumed, AppLifecycleState.inactive};

/// 进程内控件兜底：开发者显式开启后注册 `<prefix>.outline` / `click` / `fill` / `press` / `scroll` / `read`
/// （spec/ui-fallback.md）。
///
/// - 工具以 `surface: view` 注册，只在 App 有可见窗口（`AppLifecycleState` 为 resumed / inactive）时启用；
/// - 语义树（`SemanticsBinding.ensureSemantics()`）只在启用且已连接 Host 时开启，其余时间不产生开销；
/// - 密码类文本框（`obscureText`）的值只显示 `••••`，拒绝填写与按键；
/// - 用 [McpDeclared] 包住已有声明工具的控件，大纲会标出 `[已声明：…]`。
///
/// ```dart
/// final fallback = McpUiFallback.enable(client);   // 只在开发环境或用户明确开启时
/// // fallback.dispose() 注销全部兜底工具
/// ```
final class McpUiFallback {
  McpUiFallback._(this._scope, this.inspector, this._prefix, this._connected);

  /// 注册兜底工具（在 [scope]（缺省为客户端根作用域）下建子作用域 `<prefix>-fallback`）。必须在主 isolate 上调用。
  ///
  /// [maxItems] 为 `outline` 缺省列出的控件数；[inspector] 供测试替换（如注入 `tester.pump`）。
  static McpUiFallback enable(AppMcp client,
      {McpScope? scope, String prefix = 'ui', int maxItems = 60, McpUiInspector? inspector}) {
    final engine = inspector ?? McpUiInspector(prefix: prefix, maxItems: maxItems);
    final fallback = McpUiFallback._((scope ?? client.root).scope('$prefix-fallback'), engine, prefix,
        client.state.status == ConnectionStatus.connected);
    fallback._register();
    fallback._states = client.states.listen((s) => fallback._setConnected(s.status == ConnectionStatus.connected));
    fallback._lifecycle = AppLifecycleListener(onStateChange: fallback._setLifecycle);
    fallback._setLifecycle(WidgetsBinding.instance.lifecycleState);
    return fallback;
  }

  final McpScope _scope;
  final String _prefix;

  /// 执行引擎。
  final McpUiInspector inspector;

  final List<ToolHandle> _tools = [];
  StreamSubscription<McpConnectionState>? _states;
  AppLifecycleListener? _lifecycle;
  SemanticsHandle? _semantics;
  bool _visible = false;
  bool _connected;
  bool _disposed = false;

  /// 兜底工具当前是否启用（App 有可见窗口）。
  bool get enabled => !_disposed && _visible;

  /// 当前是否持有语义树句柄。
  bool get semanticsActive => _semantics != null;

  void _setLifecycle(AppLifecycleState? state) {
    final visible = _visibleStates.contains(state);
    if (_disposed || visible == _visible) return;
    _visible = visible;
    for (final t in _tools) {
      if (!t.isDisposed) t.setEnabled(visible);
    }
    if (!visible) inspector.clear();
    _syncSemantics();
  }

  void _setConnected(bool connected) {
    _connected = connected;
    _syncSemantics();
  }

  void _syncSemantics() {
    final want = enabled && _connected;
    if (want && _semantics == null) {
      _semantics = SemanticsBinding.instance.ensureSemantics();
    } else if (!want && _semantics != null) {
      _semantics?.dispose();
      _semantics = null;
    }
  }

  /// 调用到达时语义树尚未开启（状态事件晚于调用）：现在开启并等一帧让树建立。
  Future<void> _ready() async {
    if (_semantics != null) return;
    _semantics = SemanticsBinding.instance.ensureSemantics();
    await mcpNextFrame();
  }

  ToolHandler _guard(FutureOr<Object?> Function(Map<String, dynamic> args) run) => (args, ctx) async {
        if (!enabled) throw ToolCallError(ErrorKind.toolDisabled, '应用当前没有可见窗口，兜底工具不可用');
        await _ready();
        return run(args);
      };

  void _register() {
    ToolHandle add(String name, String title, String description, Map<String, Object?> properties,
        {List<String> required = const [], required bool readOnly, required ToolHandler handler}) {
      return _scope.registerTool(
          ToolSpec(
            name: '$_prefix.$name',
            title: title,
            description: description,
            inputSchema: {
              'type': 'object',
              'properties': properties,
              if (required.isNotEmpty) 'required': required,
              'additionalProperties': false,
            },
            risk: readOnly ? Risk.read : Risk.write,
            annotations: ToolAnnotations(title: title, readOnlyHint: readOnly),
            surface: ToolSurface.view,
            enabled: false,
          ),
          handler);
    }

    const refProp = {'type': 'string', 'description': '控件引用，如 "e12"（来自 outline）'};
    _tools.addAll([
      add(
        'outline',
        '界面控件大纲',
        '兜底能力：列出当前界面可见的可交互控件（按钮、输入框、复选框等），每行一个，带引用 eN，按对话框 / 区域分组。'
            '界面已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / scroll / read。',
        {
          'query': {'type': 'string', 'description': '按名称模糊过滤（空格分隔多个词，全部匹配）'},
          'within': {'type': 'string', 'description': '只列出该引用分组的子树，如 "e40"'},
          'limit': {
            'type': 'integer',
            'minimum': 1,
            'maximum': uiLimitMax,
            'description': '最多列出的控件数，默认 ${inspector.maxItems}'
          },
        },
        readOnly: true,
        handler: _guard((a) => inspector
            .outline(query: _string(a, 'query'), within: _ref(a, 'within', required: false), limit: _int(a, 'limit'))
            .toJson()),
      ),
      add('click', '点击控件', '兜底能力：激活 $_prefix.outline 中的控件（点击按钮、切换复选框、选中选项），返回界面变化摘要。',
          {'ref': refProp},
          required: ['ref'], readOnly: false, handler: _guard((a) => inspector.click(_ref(a, 'ref')!))),
      add(
        'fill',
        '填写控件',
        '兜底能力：填写文本框；复选框 / 开关 / 单选框传 true / false。密码类控件不支持。返回界面变化摘要。',
        {
          'ref': refProp,
          'value': {
            'description': '文本、数字或布尔（复选框 / 开关 / 单选框）',
            'anyOf': [
              {'type': 'string'},
              {'type': 'number'},
              {'type': 'boolean'},
            ],
          },
        },
        required: ['ref', 'value'],
        readOnly: false,
        handler: _guard((a) {
          if (!a.containsKey('value')) throw ToolCallError(ErrorKind.invalidInput, '缺少参数 value');
          return inspector.fill(_ref(a, 'ref')!, a['value']);
        }),
      ),
      add(
        'press',
        '按键',
        '兜底能力：按键（ref 缺省为当前焦点控件）：Enter（提交文本框 / 激活）、Escape（关闭对话框）、Tab / Shift+Tab（移动焦点）、Space（激活）。'
            '输入文本请用 fill。返回界面变化摘要。',
        {
          'ref': {...refProp, 'description': '目标控件引用；缺省为当前焦点控件'},
          'key': {'type': 'string', 'description': '按键，如 "Enter"、"Escape"、"Tab"、"Shift+Tab"'},
        },
        required: ['key'],
        readOnly: false,
        handler: _guard((a) {
          final key = _string(a, 'key');
          if (key == null) throw ToolCallError(ErrorKind.invalidInput, '缺少参数 key');
          return inspector.press(_ref(a, 'ref', required: false), key);
        }),
      ),
      add(
        'scroll',
        '滚动',
        '兜底能力：无 direction 时把控件滚动到可见；direction 为 up / down / left / right 时滚动该控件所在的滚动区一页'
            '（down = 向下翻看更多内容）。返回界面变化摘要。',
        {
          'ref': refProp,
          'direction': {
            'type': 'string',
            'enum': uiScrollActions.keys.toList(),
            'description': '查看方向；缺省为滚动到该控件可见'
          },
        },
        required: ['ref'],
        readOnly: false,
        handler: _guard((a) => inspector.scroll(_ref(a, 'ref')!, direction: _string(a, 'direction'))),
      ),
      add(
        'read',
        '读取控件文本',
        '兜底能力：读取控件的可见文本（折叠空白，默认最多 $uiReadDefault 字）；ref 缺省为整个界面。',
        {
          'ref': {...refProp, 'description': '控件引用；缺省为整个界面'},
          'maxChars': {'type': 'integer', 'minimum': 1, 'maximum': uiReadMax, 'description': '默认 $uiReadDefault'},
        },
        readOnly: true,
        handler: _guard((a) => inspector.read(ref: _ref(a, 'ref', required: false), maxChars: _int(a, 'maxChars'))),
      ),
    ]);
  }

  /// 注销全部兜底工具、释放语义树句柄。幂等。
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    unawaited(_states?.cancel());
    _lifecycle?.dispose();
    _semantics?.dispose();
    _semantics = null;
    inspector.clear();
    _scope.dispose();
  }
}

String? _string(Map<String, dynamic> a, String key) {
  final v = a[key];
  if (v == null) return null;
  if (v is! String) throw ToolCallError(ErrorKind.invalidInput, '$key 应为字符串');
  return v;
}

int? _int(Map<String, dynamic> a, String key) {
  final v = a[key];
  if (v == null) return null;
  if (v is! num || !v.isFinite) throw ToolCallError(ErrorKind.invalidInput, '$key 应为正整数');
  return v.floor();
}

String? _ref(Map<String, dynamic> a, String key, {bool required = true}) {
  final v = a[key];
  if (v == null || v == '') {
    if (required) throw ToolCallError(ErrorKind.invalidInput, '缺少参数 $key（控件引用，如 "e12"）');
    return null;
  }
  if (v is! String || !_refPattern.hasMatch(v.trim())) {
    throw ToolCallError(ErrorKind.invalidInput, '$key 应为控件引用，如 "e12"');
  }
  return v.trim();
}

/// 标出控件已有声明的工具：兜底大纲中显示 `[已声明：<tool>]`，提示模型优先调用该工具（spec/ui-fallback.md 8.1）。
///
/// 写入语义 identifier `mcp:<tool>`；不影响其他语义属性。
///
/// ```dart
/// McpDeclared(tool: 'cart.clear', child: TextButton(onPressed: clear, child: const Text('清空')))
/// ```
class McpDeclared extends StatelessWidget {
  const McpDeclared({super.key, required this.tool, required this.child});

  final String tool;
  final Widget child;

  @override
  Widget build(BuildContext context) => Semantics(identifier: '$uiDeclaredPrefix$tool', child: child);
}
