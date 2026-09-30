import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// 把平台侧收到的唤醒参数转交给 [AppMcp.handleWake]。
///
/// Flutter 应用的唤醒参数来自平台通道：
/// - Android：Kotlin 侧 `WakeReceiver`（后续由 Kotlin SDK 提供）收到 Host 的显式广播
///   `dev.appmcp.action.WAKE` 后，经 MethodChannel [channelName] 调用 `handleWake`，参数为
///   `app-mcp-wake:<token>`；
/// - iOS：`onOpenURL` / `application(_:open:options:)` 收到 `<scheme>://app-mcp/wake?token=`，
///   可以同样经本通道转发，或用 `app_links` 等包在 Dart 侧直接调用 [handleLink]。
///
/// ```dart
/// final wake = AppMcpWakeChannel(client)..attach();
/// appLinks.uriLinkStream.listen(wake.handleLink);   // iOS / Android App Links
/// ```
class AppMcpWakeChannel {
  AppMcpWakeChannel(this.client, {MethodChannel? channel})
      : channel = channel ?? const MethodChannel(channelName);

  /// 默认通道名；平台侧调用方法 `handleWake`，参数为激活字符串，返回 bool。
  static const String channelName = 'dev.appmcp/wake';

  final AppMcp client;
  final MethodChannel channel;

  /// 开始接收平台侧的 `handleWake` 调用。
  void attach() => channel.setMethodCallHandler(_onCall);

  /// 停止接收。
  void detach() => channel.setMethodCallHandler(null);

  /// 处理一个链接 / 激活参数（`app_links`、`getInitialLink` 等）。不是唤醒或客户端已释放时返回 false。
  bool handleLink(Object? link) {
    if (link == null || client.isDisposed) return false;
    try {
      return client.handleWake(link.toString());
    } on AppMcpException catch (e, st) {
      FlutterError.reportError(FlutterErrorDetails(
          exception: e, stack: st, library: 'app_mcp_flutter', context: ErrorDescription('处理唤醒参数时')));
      return false;
    }
  }

  Future<Object?> _onCall(MethodCall call) async {
    switch (call.method) {
      case 'handleWake':
        return handleLink(call.arguments);
      default:
        throw MissingPluginException('未知方法 ${call.method}');
    }
  }
}
