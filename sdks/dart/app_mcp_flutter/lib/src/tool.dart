import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/widgets.dart';

import 'scope.dart';

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
      );

  @override
  State<McpTool> createState() => _McpToolState();
}

class _McpToolState extends State<McpTool> {
  McpScope? _scope;
  ToolHandle? _handle;

  // handler 通过这一层转发，保证在途调用也用最新的闭包。
  Future<Object?> _invoke(Map<String, dynamic> args, ToolContext ctx) async =>
      widget.handler(args, ctx);

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
  void didUpdateWidget(McpTool oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.name != widget.name) {
      _handle?.dispose();
      _register();
      return;
    }
    final handle = _handle;
    if (handle == null || handle.isDisposed) return;
    try {
      handle.replace(widget.spec);
    } on AppMcpException catch (e, st) {
      _report(e, st, '更新工具 ${widget.name} 时');
    }
  }

  void _register() {
    _handle = null;
    final scope = _scope;
    if (scope == null || scope.isDisposed) return;
    try {
      _handle = scope.registerTool(widget.spec, _invoke);
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
    required this.read,
    this.changeToken,
    this.child,
  });

  final String name;
  final String description;
  final String? mimeType;

  /// 需实时推送（spec/lifecycle.md 第 13 节 B3），见 [McpScope.resource]。
  final bool realtime;
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
        oldWidget.realtime != widget.realtime) {
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
    required ToolHandler handler,
  }) {
    final scope = AppMcpScope.scopeOf(context);
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
        enabled: enabled,
        annotations: annotations,
        outputSchema: outputSchema);
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
