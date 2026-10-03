import 'dart:convert' show jsonEncode;

import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/widgets.dart';

import 'scope.dart';

/// 随 widget 声明一个可发出的事件（spec/protocol.md 3.5）：挂载时 [AppMcp.declareEvent]，卸载时 [AppMcp.removeEvent]；
/// [description] / [payloadSchema] 变化时重新声明（同名替换），[name] 或客户端变化时先撤销旧声明。
/// 发出用 `AppMcpScope.of(context).emitEvent(name, payload)`：已连接时发送并返回 true，未连接时丢弃并返回 false
/// （不缓存、不唤醒 Host）。整个 App 都会发出的事件直接在创建客户端后 `declareEvent` 即可。
///
/// ```dart
/// McpEvent(
///   name: 'message.received',
///   description: '聊天页收到新消息',
///   payloadSchema: const {'type': 'object', 'properties': {'from': {'type': 'string'}}},
///   child: ChatView(...),
/// )
/// ```
///
/// @invariant 同一客户端上同名事件只由一个 [McpEvent] 声明：任一实例卸载都会撤销该名称。
class McpEvent extends StatefulWidget {
  const McpEvent({super.key, required this.name, required this.description, this.payloadSchema, required this.child});

  final String name;
  final String description;

  /// 载荷的 JSON Schema（JSON 对象；描述用，Hub 不校验）。
  final Map<String, Object?>? payloadSchema;
  final Widget child;

  @override
  State<McpEvent> createState() => _McpEventState();
}

class _McpEventState extends State<McpEvent> {
  /// 已声明的客户端、名称与声明内容（描述 + schema 的 JSON，用于判断是否需要重新声明）；未声明时为 null。
  AppMcp? _client;
  String? _name;
  String? _declared;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _apply();
  }

  @override
  void didUpdateWidget(McpEvent oldWidget) {
    super.didUpdateWidget(oldWidget);
    _apply();
  }

  void _apply() {
    final client = AppMcpScope.of(context);
    final declared = jsonEncode([widget.description, widget.payloadSchema]);
    if (identical(client, _client) && widget.name == _name && declared == _declared) return;
    if (!identical(client, _client) || widget.name != _name) _remove();
    try {
      client.declareEvent(widget.name, widget.description, payloadSchema: widget.payloadSchema);
      _client = client;
      _name = widget.name;
      _declared = declared;
    } on AppMcpException catch (e, st) {
      _report(e, st, '声明事件 ${widget.name} 时');
    }
  }

  /// @error 客户端已释放时忽略（声明随客户端一起失效）；其他失败上报 FlutterError。
  void _remove() {
    final (client, name) = (_client, _name);
    _client = null;
    _name = null;
    _declared = null;
    if (client == null || name == null) return;
    try {
      client.removeEvent(name);
    } on AppMcpException catch (e, st) {
      if (e.code != AppMcpErrorCode.stopped) _report(e, st, '撤销事件 $name 时');
    }
  }

  static void _report(Object e, StackTrace st, String what) {
    FlutterError.reportError(FlutterErrorDetails(
        exception: e, stack: st, library: 'app_mcp_flutter', context: ErrorDescription(what)));
  }

  @override
  void dispose() {
    _remove();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
