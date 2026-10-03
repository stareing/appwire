import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/widgets.dart';

import 'scope.dart';

/// 随 widget 声明用户正在操作（spec/protocol.md 5.3「用户正在操作」）：挂载且 [busy] 为 true 时持有一个
/// [AppMcp.beginBusy] 作用域，[busy] 变为 false 或卸载时归还。期间写调用按客户端的 [BusyPolicy] 拒绝或排队，只读调用不受影响。
///
/// ```dart
/// // 编辑表单打开期间，模型的写操作暂不执行
/// McpBusy(busy: _editing, child: OrderForm(...))
/// ```
///
/// 作用域按引用计数：多个 [McpBusy] 可嵌套或并存，全部解除（且 [AppMcp.setBusy] 的显式开关为 false）时才解除"正在操作"。
class McpBusy extends StatefulWidget {
  const McpBusy({super.key, this.busy = true, required this.child});

  /// 是否声明用户正在操作。
  final bool busy;
  final Widget child;

  @override
  State<McpBusy> createState() => _McpBusyState();
}

class _McpBusyState extends State<McpBusy> {
  /// 本 widget 持有的作用域及其客户端；未声明时为 null。
  McpBusyHold? _hold;
  AppMcp? _declaredOn;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _apply();
  }

  @override
  void didUpdateWidget(McpBusy oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.busy != widget.busy) _apply();
  }

  /// 让客户端状态与 [McpBusy.busy] 一致；客户端换了时先解除旧客户端上的声明。
  void _apply() {
    final client = AppMcpScope.of(context);
    final target = widget.busy ? client : null;
    if (identical(target, _declaredOn)) return;
    _release();
    if (target == null) return;
    try {
      _hold = target.beginBusy();
      _declaredOn = target;
    } on AppMcpException catch (e, st) {
      _report(e, st, '声明用户正在操作时');
    }
  }

  /// @error 客户端已释放时只记账（[McpBusyHold.release] 不调用原生库）；其他失败上报 FlutterError。
  void _release() {
    final hold = _hold;
    _hold = null;
    _declaredOn = null;
    if (hold == null) return;
    try {
      hold.release();
    } on AppMcpException catch (e, st) {
      _report(e, st, '解除用户正在操作时');
    }
  }

  static void _report(Object e, StackTrace st, String what) {
    FlutterError.reportError(FlutterErrorDetails(
        exception: e, stack: st, library: 'app_mcp_flutter', context: ErrorDescription(what)));
  }

  @override
  void dispose() {
    _release();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
