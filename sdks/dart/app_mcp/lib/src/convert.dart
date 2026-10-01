// Dart 值与 C ABI 值之间的转换（纯函数，便于单元测试）。

import 'dart:async';
import 'dart:convert';

import 'bindings.dart';
import 'types.dart';

int riskToNative(Risk risk) => switch (risk) {
      Risk.read => AmRisk.read,
      Risk.write => AmRisk.write,
      Risk.destructive => AmRisk.destructive,
      Risk.payment => AmRisk.payment,
      Risk.osSensitive => AmRisk.osSensitive,
    };

int activationToNative(Activation? activation) => switch (activation) {
      null => AmActivation.none,
      Activation.headless => AmActivation.headless,
      Activation.background => AmActivation.background,
      Activation.foreground => AmActivation.foreground,
    };

int visibilityToNative(AppVisibility visibility) => switch (visibility) {
      AppVisibility.visible => AmVisibility.visible,
      AppVisibility.hidden => AmVisibility.hidden,
      AppVisibility.frozen => AmVisibility.frozen,
    };

int clientKindToNative(ClientKind kind) => switch (kind) {
      ClientKind.native => AmClientKind.native,
      ClientKind.hybrid => AmClientKind.hybrid,
    };

int lifecycleModeToNative(LifecycleMode m) => switch (m) {
      LifecycleMode.persistent => AmLifecycleMode.persistent,
      LifecycleMode.idle => AmLifecycleMode.idle,
      LifecycleMode.onDemand => AmLifecycleMode.onDemand,
    };

int residencyToNative(Residency r) => switch (r) {
      Residency.keep => AmResidency.keep,
      Residency.exitWhenIdle => AmResidency.exitWhenIdle,
      Residency.exitAlways => AmResidency.exitAlways,
    };

/// null → `AM_WAKE_UNSET`（不上报）。
int wakeKindToNative(WakeKind? k) => switch (k) {
      null => AmWakeKind.unset,
      WakeKind.none => AmWakeKind.none,
      WakeKind.uri => AmWakeKind.uri,
      WakeKind.aumid => AmWakeKind.aumid,
      WakeKind.appleEvent => AmWakeKind.appleEvent,
      WakeKind.dbus => AmWakeKind.dbus,
      WakeKind.androidIntent => AmWakeKind.androidIntent,
      WakeKind.webUrl => AmWakeKind.webUrl,
    };

int wakeReasonToNative(WakeReason r) => switch (r) {
      WakeReason.osActivation => AmWakeReason.osActivation,
      WakeReason.app => AmWakeReason.app,
      WakeReason.visible => AmWakeReason.visible,
      WakeReason.coldStart => AmWakeReason.coldStart,
    };

int sleepReasonToNative(SleepReason r) => switch (r) {
      SleepReason.idle => AmSleepReason.idle,
      SleepReason.grace => AmSleepReason.grace,
      SleepReason.background => AmSleepReason.background,
      SleepReason.app => AmSleepReason.app,
    };

/// Duration → 非负毫秒数。
int durationToMs(Duration d) => d.isNegative ? 0 : d.inMilliseconds;

/// 未知值按 [ConnectionStatus.idle] 处理（不应发生）。
ConnectionStatus statusFromNative(int status) => switch (status) {
      AmStateStatus.idle => ConnectionStatus.idle,
      AmStateStatus.connecting => ConnectionStatus.connecting,
      AmStateStatus.handshaking => ConnectionStatus.handshaking,
      AmStateStatus.pendingPairing => ConnectionStatus.pendingPairing,
      AmStateStatus.connected => ConnectionStatus.connected,
      AmStateStatus.backoff => ConnectionStatus.backoff,
      AmStateStatus.rejected => ConnectionStatus.rejected,
      AmStateStatus.stopped => ConnectionStatus.stopped,
      AmStateStatus.dormant => ConnectionStatus.dormant,
      AmStateStatus.waking => ConnectionStatus.waking,
      AmStateStatus.hostMismatch => ConnectionStatus.hostMismatch,
      _ => ConnectionStatus.idle,
    };

McpConnectionState stateFromNative(int status, int retryInMs, String? reason, {String? code}) {
  final s = statusFromNative(status);
  return McpConnectionState(
    s,
    retryIn: s == ConnectionStatus.backoff ? Duration(milliseconds: retryInMs) : null,
    reason: switch (s) {
      ConnectionStatus.rejected || ConnectionStatus.hostMismatch => reason ?? '',
      // C ABI v6 起 backoff 在连接失败等情况下也带原因。
      ConnectionStatus.backoff => reason,
      _ => null,
    },
    code: statusHasCode(s) ? code : null,
  );
}

/// 只有这些状态带错误码（spec/protocol.md 10.1）。
bool statusHasCode(ConnectionStatus s) =>
    s == ConnectionStatus.backoff || s == ConnectionStatus.rejected || s == ConnectionStatus.hostMismatch;

/// 未知值按 [LogLevel.info] 处理。
LogLevel logLevelFromNative(int level) => switch (level) {
      AmLogLevel.debug => LogLevel.debug,
      AmLogLevel.warn => LogLevel.warn,
      AmLogLevel.error => LogLevel.error,
      _ => LogLevel.info,
    };

/// 未知值按 [CancelReason.requested] 处理。
CancelReason cancelReasonFromNative(int reason) => switch (reason) {
      AmCancelReason.timeout => CancelReason.timeout,
      AmCancelReason.disconnected => CancelReason.disconnected,
      AmCancelReason.stopped => CancelReason.stopped,
      _ => CancelReason.requested,
    };

AppMcpErrorCode errorCodeFromStatus(int status) => switch (status) {
      AmStatus.invalidArgument => AppMcpErrorCode.invalidArgument,
      AmStatus.invalidName => AppMcpErrorCode.invalidName,
      AmStatus.invalidSchema => AppMcpErrorCode.invalidSchema,
      AmStatus.duplicateName => AppMcpErrorCode.duplicateName,
      AmStatus.invalidJson => AppMcpErrorCode.invalidJson,
      AmStatus.invalidConfig => AppMcpErrorCode.invalidConfig,
      AmStatus.alreadyCompleted => AppMcpErrorCode.alreadyCompleted,
      AmStatus.disposed => AppMcpErrorCode.disposed,
      AmStatus.stopped => AppMcpErrorCode.stopped,
      AmStatus.internal => AppMcpErrorCode.internal,
      AmStatus.panic => AppMcpErrorCode.panic,
      _ => AppMcpErrorCode.unknown,
    };

/// 解析调用参数。空文本或 `null` 视为无参数；非对象参数视为非法输入。
Map<String, dynamic> decodeArguments(String json) {
  if (json.trim().isEmpty) return <String, dynamic>{};
  final Object? value;
  try {
    value = jsonDecode(json);
  } on FormatException catch (e) {
    throw ToolCallError(ErrorKind.invalidInput, '参数不是合法的 JSON：${e.message}');
  }
  if (value == null) return <String, dynamic>{};
  if (value is Map<String, dynamic>) return value;
  throw ToolCallError(ErrorKind.invalidInput, '参数必须是 JSON 对象');
}

/// 编码 inputSchema；为 null 时返回 null（无参数）。
String? encodeSchema(Map<String, Object?>? schema) => schema == null ? null : jsonEncode(schema);

/// handler 返回值规范化后的结果。
final class EncodedResult {
  const EncodedResult(this.dataJson, this.stateHints);
  final String dataJson;
  final List<String> stateHints;
}

/// 把 handler 返回值编码为 JSON。无法编码时抛出 [ToolCallError]（HANDLER_ERROR）。
EncodedResult encodeResult(Object? value) {
  final data = value is ToolResult ? value.data : value;
  final hints = value is ToolResult ? value.stateHints : const <String>[];
  return EncodedResult(encodeJsonValue(data, '返回值'), hints);
}

/// 编码任意值为 JSON 文本；失败时抛出 [ToolCallError]。
String encodeJsonValue(Object? value, String what) {
  try {
    return jsonEncode(value);
  } on JsonUnsupportedObjectError catch (e) {
    throw ToolCallError(
        ErrorKind.handlerError, '$what无法编码为 JSON：${e.unsupportedObject.runtimeType}');
  }
}

/// 把 handler 抛出的错误映射为协议错误类别、说明与详情 JSON（没有或无法编码时为 null）。
({String kind, String message, String? detailsJson}) failureFromError(Object error) {
  if (error is ToolCallError) {
    String? details;
    if (error.details != null) {
      try {
        details = jsonEncode(error.details);
      } on JsonUnsupportedObjectError {
        details = null;
      }
    }
    return (kind: error.kind.wireName, message: error.message, detailsJson: details);
  }
  if (error is TimeoutException) {
    return (kind: ErrorKind.timeout.wireName, message: error.message ?? '处理超时', detailsJson: null);
  }
  return (kind: ErrorKind.handlerError.wireName, message: error.toString(), detailsJson: null);
}

