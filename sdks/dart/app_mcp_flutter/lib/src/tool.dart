import 'dart:convert' show jsonEncode;

import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/widgets.dart';

import 'scope.dart';
import 'view.dart';

void _report(Object e, StackTrace st, String what) {
  FlutterError.reportError(FlutterErrorDetails(
      exception: e, stack: st, library: 'app_mcp_flutter', context: ErrorDescription(what)));
}

/// 声明式注册一个工具：挂载时注册，卸载时注销。
///
/// 重建时（[State.didUpdateWidget]）：
/// - 总是用新的 [handler] 替换旧的（handler 通常是捕获了最新状态的闭包），不产生协议消息；
/// - 定义（描述、schema、风险等）变化时调用 [ToolHandle.replace]，相同则什么也不做；
/// - 只有 [name] 变化或所在作用域变化时才注销并重新注册。
///
/// [surface] 为 [ToolSurface.view] 时只在所在界面可见且处于最上层时启用（[McpViewGate.isActive]：最近的
/// [McpViewGate] / [McpRouteGate]，否则所在路由是否为栈顶），否则禁用；[page] 声明所在页面，Hub 据此导航；
/// [backgroundTool] 声明 App 在后台时 Hub 改调的同 App app 工具（spec/protocol.md 3.4「后台与前台」）。
/// [concurrency] / [exclusive] 为 SDK 内的调用调度声明（spec/protocol.md 5.3，见 [ToolSpec.concurrency]、[ToolSpec.exclusive]）。
/// [implements] 声明实现的标准意图（spec/intents.md，见 [ToolSpec.implements]）；[cache] 声明结果缓存（spec/protocol.md 3.6，
/// 见 [ToolSpec.cache]）；[deprecated] 声明弃用（spec/protocol.md 3.7，见 [ToolSpec.deprecated]）；[undoable] 声明结果可能带撤销信息
/// （spec/protocol.md 3.8，见 [ToolSpec.undoable]；handler 返回带 [ToolResult.undo] 的结果）。
///
/// ```dart
/// McpTool(
///   name: 'cart.add',
///   description: '把商品加入购物车',
///   inputSchema: const {'type': 'object', 'properties': {'id': {'type': 'string'}}},
///   handler: (args, ctx) { setState(() => cart.add(args['id'] as String)); return {'ok': true}; },
///   child: CartView(cart),
/// )
/// ```
class McpTool extends StatefulWidget {
  const McpTool({
    super.key,
    required this.name,
    required this.description,
    this.inputSchema,
    this.risk = Risk.write,
    this.activation,
    this.title,
    this.enabled = true,
    this.annotations,
    this.outputSchema,
    this.surface = ToolSurface.app,
    this.page,
    this.backgroundTool,
    this.concurrency = 0,
    this.exclusive,
    this.implements = const [],
    this.cache,
    this.deprecated,
    this.undoable = false,
    required this.handler,
    this.child,
  });

  final String name;
  final String description;
  final Map<String, Object?>? inputSchema;

  /// 旧写法：优先用 [annotations]。
  final Risk risk;
  final Activation? activation;
  final String? title;
  final bool enabled;

  /// 标准 MCP 工具注解；为 null 时不声明（Host 按 [risk] 推导）。
  final ToolAnnotations? annotations;

  /// 结果的 JSON Schema（MCP outputSchema）；为 null 时不声明。
  final Map<String, Object?>? outputSchema;

  /// 对界面的依赖（spec/protocol.md 3.4）。
  final ToolSurface surface;

  /// 所在页面名；为 null 时不声明。
  final String? page;

  /// 只对 [ToolSurface.view] 有意义：App 在后台时 Hub 改调的同 App app 工具本地名；为 null 时不声明。
  final String? backgroundTool;

  /// 本工具同时执行的调用上限；0 = 不单独限制。
  final int concurrency;

  /// 互斥组名：同组的工具同一时刻至多一个在执行；为 null 时不互斥。
  final String? exclusive;

  /// 实现的标准意图（如 `message.send@1`）；空表示不声明。
  final List<String> implements;

  /// 结果缓存声明（只对只读工具生效）；为 null 时不声明，变化时整体替换。
  final CachePolicy? cache;

  /// 弃用声明（照常列出与调用）；为 null 时不声明，变化时整体替换。
  final ToolDeprecation? deprecated;

  /// 成功结果可能带撤销信息（只用于展示）；变化时整体替换。
  final bool undoable;
  final ToolHandler handler;
  final Widget? child;

  ToolSpec get spec => ToolSpec(
        name: name,
        description: description,
        inputSchema: inputSchema,
        risk: risk,
        activation: activation,
        title: title,
        enabled: enabled,
        annotations: annotations,
        outputSchema: outputSchema,
        surface: surface,
        page: page,
        backgroundTool: backgroundTool,
        concurrency: concurrency,
        exclusive: exclusive,
        implements: implements,
        cache: cache,
        deprecated: deprecated,
        undoable: undoable,
      );

  @override
  State<McpTool> createState() => _McpToolState();
}

class _McpToolState extends State<McpTool> {
  McpScope? _scope;
  ToolHandle? _handle;

  /// view 工具所在界面是否可见且处于最上层（app 工具恒为 true）。
  bool _viewActive = true;

  ToolSpec get _effectiveSpec {
    final spec = widget.spec;
    return _viewActive ? spec : spec.copyWith(enabled: false);
  }

  // handler 通过这一层转发，保证在途调用也用最新的闭包。
  Future<Object?> _invoke(Map<String, dynamic> args, ToolContext ctx) async =>
      widget.handler(args, ctx);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final active = widget.surface == ToolSurface.app || McpViewGate.isActive(context);
    final scope = AppMcpScope.scopeOf(context);
    if (!identical(scope, _scope)) {
      _handle?.dispose();
      _scope = scope;
      _viewActive = active;
      _register();
      return;
    }
    if (active != _viewActive) {
      _viewActive = active;
      _sync();
    }
  }

  @override
  void didUpdateWidget(McpTool oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.surface != widget.surface) {
      _viewActive = widget.surface == ToolSurface.app || McpViewGate.isActive(context);
    }
    if (oldWidget.name != widget.name) {
      _handle?.dispose();
      _register();
      return;
    }
    _sync();
  }

  /// 按当前定义与界面状态更新（相同则无协议消息）。
  void _sync() {
    final handle = _handle;
    if (handle == null || handle.isDisposed) return;
    try {
      handle.replace(_effectiveSpec);
    } on AppMcpException catch (e, st) {
      _report(e, st, '更新工具 ${widget.name} 时');
    }
  }

  void _register() {
    _handle = null;
    final scope = _scope;
    if (scope == null || scope.isDisposed) return;
    try {
      _handle = scope.registerTool(_effectiveSpec, _invoke);
    } on AppMcpException catch (e, st) {
      _report(e, st, '注册工具 ${widget.name} 时');
    }
  }

  @override
  void dispose() {
    _handle?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child ?? const SizedBox.shrink();
}

/// 声明式注册一个资源：挂载时注册，卸载时注销。
///
/// [changeToken] 变化（`!=`）时调用 [ResourceHandle.notifyChanged]，
/// 例如传入购物车的版本号或不可变的列表本身。
class McpResource extends StatefulWidget {
  const McpResource({
    super.key,
    required this.name,
    required this.description,
    this.mimeType,
    this.realtime = false,
    this.annotations,
    this.cache,
    required this.read,
    this.changeToken,
    this.child,
  });

  final String name;
  final String description;
  final String? mimeType;

  /// 需实时推送（spec/lifecycle.md 第 13 节 B3），见 [McpScope.resource]。
  final bool realtime;

  /// 资源内容的标注（MCP 内容注解），见 [McpScope.resource]；内容变化时重新注册。
  final ContentAnnotations? annotations;

  /// 读取结果缓存声明（spec/protocol.md 3.6），见 [McpScope.resource]；变化时重新注册。
  final CachePolicy? cache;
  final ResourceReader read;
  final Object? changeToken;
  final Widget? child;

  @override
  State<McpResource> createState() => _McpResourceState();
}

class _McpResourceState extends State<McpResource> {
  McpScope? _scope;
  ResourceHandle? _handle;

  Future<Object?> _read() async => widget.read();

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final scope = AppMcpScope.scopeOf(context);
    if (!identical(scope, _scope)) {
      _handle?.dispose();
      _scope = scope;
      _register();
    }
  }

  @override
  void didUpdateWidget(McpResource oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.name != widget.name ||
        oldWidget.description != widget.description ||
        oldWidget.mimeType != widget.mimeType ||
        oldWidget.realtime != widget.realtime ||
        oldWidget.cache != widget.cache ||
        !_sameAnnotations(oldWidget.annotations, widget.annotations)) {
      _handle?.dispose();
      _register();
      return;
    }
    final handle = _handle;
    if (oldWidget.changeToken != widget.changeToken && handle != null && !handle.isDisposed) {
      try {
        handle.notifyChanged();
      } on AppMcpException catch (e, st) {
        _report(e, st, '通知资源 ${widget.name} 变化时');
      }
    }
  }

  void _register() {
    _handle = null;
    final scope = _scope;
    if (scope == null || scope.isDisposed) return;
    try {
      _handle = scope.resource(widget.name,
          description: widget.description,
          mimeType: widget.mimeType,
          realtime: widget.realtime,
          annotations: widget.annotations,
          cache: widget.cache,
          read: _read);
    } on AppMcpException catch (e, st) {
      _report(e, st, '注册资源 ${widget.name} 时');
    }
  }

  @override
  void dispose() {
    _handle?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child ?? const SizedBox.shrink();
}

/// `useMcpTool` 风格的辅助：在 [State.build] 中调用 [useMcpTool]。
///
/// - 第一次调用时注册，之后的调用只刷新 handler 与定义（定义相同则无协议消息）；
/// - 工具一直保留到 State 卸载（与 hook 不同，不会因某次 build 未调用而注销）；
///   需要暂时隐藏时传 `enabled: false`；
/// - State 卸载时注销全部工具。
///
/// ```dart
/// class _CartPageState extends State<CartPage> with McpToolsMixin {
///   @override
///   Widget build(BuildContext context) {
///     useMcpTool('cart.clear', description: '清空购物车',
///         handler: (args, ctx) { setState(cart.clear); return null; });
///     return ...;
///   }
/// }
/// ```
mixin McpToolsMixin<T extends StatefulWidget> on State<T> {
  final Map<String, ToolHandle> _mcpTools = {};
  final Map<String, ToolHandler> _mcpHandlers = {};
  McpScope? _mcpScope;

  /// 注册或刷新工具。只能在 [build] 中调用。
  ToolHandle? useMcpTool(
    String name, {
    required String description,
    Map<String, Object?>? inputSchema,
    Risk risk = Risk.write,
    Activation? activation,
    String? title,
    bool enabled = true,
    ToolAnnotations? annotations,
    Map<String, Object?>? outputSchema,
    ToolSurface surface = ToolSurface.app,
    String? page,
    String? backgroundTool,
    int concurrency = 0,
    String? exclusive,
    List<String> implements = const [],
    CachePolicy? cache,
    ToolDeprecation? deprecated,
    bool undoable = false,
    required ToolHandler handler,
  }) {
    final scope = AppMcpScope.scopeOf(context);
    // view 工具按所在界面门控（同 [McpTool]）；build 中依赖门控 / 路由，状态变化时会重新 build。
    final active = surface == ToolSurface.app || McpViewGate.isActive(context);
    if (!identical(scope, _mcpScope)) {
      for (final h in _mcpTools.values) {
        h.dispose();
      }
      _mcpTools.clear();
      _mcpScope = scope;
    }
    _mcpHandlers[name] = handler;
    final spec = ToolSpec(
        name: name,
        description: description,
        inputSchema: inputSchema,
        risk: risk,
        activation: activation,
        title: title,
        enabled: enabled && active,
        annotations: annotations,
        outputSchema: outputSchema,
        surface: surface,
        page: page,
        backgroundTool: backgroundTool,
        concurrency: concurrency,
        exclusive: exclusive,
        implements: implements,
        cache: cache,
        deprecated: deprecated,
        undoable: undoable);
    final existing = _mcpTools[name];
    try {
      if (existing != null && !existing.isDisposed) {
        existing.replace(spec);
        return existing;
      }
      if (scope.isDisposed) return null;
      final handle = scope.registerTool(spec, (args, ctx) async => _mcpHandlers[name]!(args, ctx));
      _mcpTools[name] = handle;
      return handle;
    } on AppMcpException catch (e, st) {
      _report(e, st, '注册工具 $name 时');
      return null;
    }
  }

  @override
  void dispose() {
    for (final h in _mcpTools.values) {
      h.dispose();
    }
    _mcpTools.clear();
    _mcpHandlers.clear();
    super.dispose();
  }
}

/// 按内容比较（[ContentAnnotations] 没有值相等）。
bool _sameAnnotations(ContentAnnotations? a, ContentAnnotations? b) =>
    identical(a, b) || (a != null && b != null && jsonEncode(a.toJson()) == jsonEncode(b.toJson()));
