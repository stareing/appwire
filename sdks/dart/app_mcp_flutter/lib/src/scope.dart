import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'lifecycle.dart';

/// 向子树提供 [AppMcp] 客户端。
///
/// ```dart
/// final client = AppMcp(appId: 'shop', appName: '商店')..start();
/// runApp(AppMcpScope(client: client, child: const ShopApp()));
/// ```
///
/// - [trackLifecycle] 为 true（默认）时，用 [AppLifecycleListener] 按 `AppLifecycleState` 调用
///   [AppMcp.setVisibility]（见 [visibilityForLifecycle]）；生命周期模式为 `idle` / `onDemand` 时，
///   回到前台（`resumed`）还会以原因 `visible` 调用 [AppMcp.wake]（见 [wakeOnResume]）。
///   进入后台后的休眠由原生层按 `hiddenIdleTimeout` 完成（iOS 默认 0：进入后台即休眠）。
/// - [disposeClient] 为 true 时，本 widget 卸载时释放客户端；默认由创建者负责。
///
/// 客户端的 handler 已经在主 isolate 上执行（`NativeCallable.listener` 投递到事件循环），
/// 可以直接修改 widget 状态。
class AppMcpScope extends StatefulWidget {
  const AppMcpScope({
    super.key,
    required this.client,
    required this.child,
    this.trackLifecycle = true,
    this.disposeClient = false,
  });

  final AppMcp client;
  final Widget child;
  final bool trackLifecycle;
  final bool disposeClient;

  /// 最近的 [AppMcpScope] 提供的客户端；没有时返回 null。
  static AppMcp? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_AppMcpInherited>()?.client;

  /// 最近的 [AppMcpScope] 提供的客户端。
  static AppMcp of(BuildContext context) {
    final client = maybeOf(context);
    assert(client != null, '找不到 AppMcpScope：请在 widget 树上方放置 AppMcpScope');
    if (client == null) {
      throw FlutterError('找不到 AppMcpScope：请在 widget 树上方放置 AppMcpScope。');
    }
    return client;
  }

  /// 当前的注册作用域：最近的 [McpToolGroup] 的作用域，否则为客户端根作用域。
  static McpScope scopeOf(BuildContext context) {
    final group = context.dependOnInheritedWidgetOfExactType<_McpGroupInherited>();
    return group?.scope ?? of(context).root;
  }

  @override
  State<AppMcpScope> createState() => _AppMcpScopeState();
}

class _AppMcpScopeState extends State<AppMcpScope> {
  AppLifecycleListener? _listener;

  @override
  void initState() {
    super.initState();
    if (widget.trackLifecycle) _track();
  }

  void _track() {
    _listener = AppLifecycleListener(onStateChange: _report);
    final state = WidgetsBinding.instance.lifecycleState;
    if (state != null) _report(state);
  }

  void _untrack() {
    _listener?.dispose();
    _listener = null;
  }

  @override
  void didUpdateWidget(AppMcpScope oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.trackLifecycle != widget.trackLifecycle) {
      if (widget.trackLifecycle) {
        _track();
      } else {
        _untrack();
      }
    }
    if (oldWidget.client != widget.client && oldWidget.disposeClient) {
      oldWidget.client.dispose();
    }
  }

  void _report(AppLifecycleState state) {
    final client = widget.client;
    if (client.isDisposed) return;
    final v = visibilityForLifecycle(state, isMobile: _isMobile);
    try {
      client.setVisibility(v.visibility, focused: v.focused);
      if (state == AppLifecycleState.resumed && wakeOnResume(client.lifecycle)) {
        client.wake(reason: WakeReason.visible);
      }
    } on AppMcpException catch (e, st) {
      FlutterError.reportError(FlutterErrorDetails(
          exception: e, stack: st, library: 'app_mcp_flutter', context: ErrorDescription('上报可见性时')));
    }
  }

  static bool get _isMobile =>
      !kIsWeb &&
      (defaultTargetPlatform == TargetPlatform.android ||
          defaultTargetPlatform == TargetPlatform.iOS);

  @override
  void dispose() {
    _untrack();
    if (widget.disposeClient) widget.client.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      _AppMcpInherited(client: widget.client, child: widget.child);
}

class _AppMcpInherited extends InheritedWidget {
  const _AppMcpInherited({required this.client, required super.child});
  final AppMcp client;

  @override
  bool updateShouldNotify(_AppMcpInherited oldWidget) => client != oldWidget.client;
}

/// 为子树创建一个命名作用域：子树中的 [McpTool] / [McpResource] 注册在该作用域下，
/// 本 widget 卸载时一次性注销（例如一个页面的全部工具）。
class McpToolGroup extends StatefulWidget {
  const McpToolGroup({super.key, required this.name, required this.child});

  /// 作用域名（仅用于诊断）。
  final String name;
  final Widget child;

  @override
  State<McpToolGroup> createState() => _McpToolGroupState();
}

class _McpToolGroupState extends State<McpToolGroup> {
  McpScope? _parent;
  McpScope? _scope;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final parent = AppMcpScope.scopeOf(context);
    if (!identical(parent, _parent)) {
      _scope?.dispose();
      _parent = parent;
      _scope = _create(parent);
    }
  }

  @override
  void didUpdateWidget(McpToolGroup oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.name != widget.name && _parent != null) {
      _scope?.dispose();
      _scope = _create(_parent!);
    }
  }

  McpScope? _create(McpScope parent) {
    try {
      return parent.scope(widget.name);
    } on AppMcpException catch (e, st) {
      FlutterError.reportError(FlutterErrorDetails(
          exception: e,
          stack: st,
          library: 'app_mcp_flutter',
          context: ErrorDescription('创建作用域 ${widget.name} 时')));
      return null;
    }
  }

  @override
  void dispose() {
    _scope?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scope = _scope;
    if (scope == null) return widget.child;
    return _McpGroupInherited(scope: scope, child: widget.child);
  }
}

class _McpGroupInherited extends InheritedWidget {
  const _McpGroupInherited({required this.scope, required super.child});
  final McpScope scope;

  @override
  bool updateShouldNotify(_McpGroupInherited oldWidget) => !identical(scope, oldWidget.scope);
}
