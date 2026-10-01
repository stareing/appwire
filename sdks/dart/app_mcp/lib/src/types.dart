// 公开的基础类型（与协议一致，见 spec/protocol.md）。纯 Dart，不依赖 FFI。

import 'dart:async';
import 'dart:convert';

/// 工具风险等级。
enum Risk {
  read('read'),
  write('write'),
  destructive('destructive'),
  payment('payment'),
  osSensitive('os-sensitive');

  const Risk(this.wireName);

  /// 协议中的字符串形式。
  final String wireName;
}

/// 调用工具时 App 需要的激活程度。
enum Activation {
  headless('headless'),
  background('background'),
  foreground('foreground');

  const Activation(this.wireName);
  final String wireName;
}

/// App 可见性，见 `app/visibility`。
enum AppVisibility {
  visible('visible'),
  hidden('hidden'),
  frozen('frozen');

  const AppVisibility(this.wireName);
  final String wireName;
}

/// 客户端连接宿主的方式。
enum ClientKind { native, hybrid }

/// 协议错误类别，见 spec/protocol.md 第 4 节。
enum ErrorKind {
  toolNotFound('TOOL_NOT_FOUND'),
  toolDisabled('TOOL_DISABLED'),
  invalidInput('INVALID_INPUT'),
  userRejected('USER_REJECTED'),
  timeout('TIMEOUT'),
  handlerError('HANDLER_ERROR'),
  cancelled('CANCELLED'),
  appDisconnected('APP_DISCONNECTED'),
  appNotInstalled('APP_NOT_INSTALLED'),
  launchFailed('LAUNCH_FAILED'),
  appNotResponding('APP_NOT_RESPONDING'),
  instanceFrozen('INSTANCE_FROZEN'),
  resourceNotFound('RESOURCE_NOT_FOUND'),
  unauthorized('UNAUTHORIZED'),
  unsupportedProtocol('UNSUPPORTED_PROTOCOL');

  const ErrorKind(this.wireName);

  /// 协议中的字符串形式，如 `HANDLER_ERROR`。
  final String wireName;

  /// 解析协议字符串；未知值返回 [ErrorKind.handlerError]。
  static ErrorKind parse(String wireName) {
    for (final k in values) {
      if (k.wireName == wireName) return k;
    }
    return handlerError;
  }
}

/// 连接状态。
enum ConnectionStatus {
  idle,
  connecting,
  handshaking,
  pendingPairing,
  connected,
  backoff,
  rejected,
  stopped,

  /// 已休眠：无连接、无定时器，等待唤醒。
  dormant,

  /// 收到唤醒后正在回连。
  waking,

  /// 对端不是期望的 Host（不是 app-mcp，或属于其他用户；spec/protocol.md 1.6）。不再自动重连，
  /// `wake()` / `connectNow()` 时再试一次；原因见 [McpConnectionState.reason]。
  hostMismatch,
}

/// 连接状态快照。
final class McpConnectionState {
  const McpConnectionState(this.status, {this.retryIn, this.reason, this.code});

  final ConnectionStatus status;

  /// [ConnectionStatus.backoff] 时距下一次重连的时间。
  final Duration? retryIn;

  /// [ConnectionStatus.rejected] / [ConnectionStatus.hostMismatch] 时的原因；
  /// [ConnectionStatus.backoff] 时为连接失败 / 断开的原因（有的话）。
  final String? reason;

  /// 与 [reason] 对应的错误码（spec/protocol.md 10.1，如 `HOST_NOT_RUNNING`）：rejected / hostMismatch 时总有，
  /// backoff 时在连接失败等情况下有，其他状态为 null。
  final String? code;

  @override
  bool operator ==(Object other) =>
      other is McpConnectionState &&
      other.status == status &&
      other.retryIn == retryIn &&
      other.reason == reason &&
      other.code == code;

  @override
  int get hashCode => Object.hash(status, retryIn, reason, code);

  @override
  String toString() => switch (status) {
        ConnectionStatus.backoff => 'McpConnectionState(backoff, retryIn: $retryIn, code: $code)',
        ConnectionStatus.rejected ||
        ConnectionStatus.hostMismatch =>
          'McpConnectionState(${status.name}, code: $code, reason: $reason)',
        _ => 'McpConnectionState(${status.name})',
      };
}

/// 原生库日志级别。
enum LogLevel { debug, info, warn, error }

/// 原生库输出的一条日志。
final class McpLogRecord {
  const McpLogRecord(this.level, this.message);

  final LogLevel level;
  final String message;

  @override
  String toString() => '[${level.name}] $message';
}

/// 调用被取消的原因。
enum CancelReason { requested, timeout, disconnected, stopped }

/// handler 可以抛出此错误以指定错误类别；其他异常归为 [ErrorKind.handlerError]。
///
/// [details] 为结构化详情（可被 `jsonEncode` 编码）：对象的字段合并进协议错误的 `data`，
/// 其他值放在 `data.details`。无法编码时丢弃详情，只保留类别与说明。
class ToolCallError implements Exception {
  ToolCallError(this.kind, this.message, {this.details});

  final ErrorKind kind;
  final String message;
  final Object? details;

  @override
  String toString() => 'ToolCallError(${kind.wireName}): $message';
}

/// 原生库返回的错误码（对应 C 的 `AmStatus`）。
enum AppMcpErrorCode {
  invalidArgument,
  invalidName,
  invalidSchema,
  duplicateName,
  invalidJson,
  invalidConfig,
  alreadyCompleted,
  disposed,
  stopped,
  internal,
  panic,

  /// 头文件中未定义的状态码。
  unknown,
}

/// 调用原生库失败（注册重名、非法名称、客户端已停止等）。
class AppMcpException implements Exception {
  AppMcpException(this.code, this.message);

  final AppMcpErrorCode code;
  final String message;

  @override
  String toString() => 'AppMcpException(${code.name}): $message';
}

/// handler 可以直接返回数据，也可以返回 [ToolResult] 以附带 stateHints。
final class ToolResult {
  const ToolResult(this.data, {this.stateHints = const []});

  /// 可被 `jsonEncode` 编码的数据。
  final Object? data;

  /// 调用后可能变化的资源名（提示 Host 重新读取）。
  final List<String> stateHints;
}

/// 一次工具调用的上下文。
abstract interface class ToolContext {
  /// Host 生成的调用 ID。
  String get callId;

  /// 被调用的工具名。
  String get toolName;

  /// 调用是否已被取消（Host 取消、超时、断线、停止）。
  bool get isCancelled;

  /// 取消原因；未取消时为 null。
  CancelReason? get cancelReason;

  /// 调用被取消时完成；调用正常结束时永不完成。
  Future<CancelReason> get cancelled;

  /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的持有被释放。
  /// 必须在调用完成前获取；调用已结束时抛出 [AppMcpException]（`alreadyCompleted`）。
  McpHold hold();
}

/// 阻止自动休眠的持有（[ToolContext.hold]、`AppMcp.hold`）。[release] 幂等；
/// 对象被垃圾回收时也会自动释放，但请显式释放以免休眠被无限推迟。
abstract interface class McpHold {
  void release();
  bool get isReleased;
}

// ---------------------------------------------------------------------------
// 生命周期（spec/lifecycle.md）
// ---------------------------------------------------------------------------

/// 生命周期模式。
enum LifecycleMode {
  /// 不休眠（桌面端默认，兼容旧行为）。
  persistent,

  /// 启动即连接；空闲后休眠；唤醒后回连。
  idle,

  /// 启动时不连接；被唤醒或 `connectNow()` 时连接，任务完成后经过合并窗口休眠（移动端默认）。
  onDemand,
}

/// 心跳策略（spec/lifecycle.md 第 11 节 A3）。
enum HeartbeatMode {
  /// 按传输：本地 IPC / 桌面本机回环不发，远程发（默认）。
  auto,
  always,
  off,
}

/// 休眠后的进程驻留策略。
enum Residency {
  /// 只断开连接（默认；移动端进程交给系统回收）。
  keep,

  /// 仅当本进程由唤醒冷启动时，休眠后触发 `onIdleExit`，由 App 决定是否退出。
  exitWhenIdle,

  /// 每次休眠后都触发 `onIdleExit`（无界面的辅助进程）。
  exitAlways,
}

/// 唤醒方式（spec/lifecycle.md 第 5 节）。
enum WakeKind { none, uri, aumid, appleEvent, dbus, androidIntent, webUrl }

/// 回连原因。
enum WakeReason { osActivation, app, visible, coldStart }

/// 休眠原因。
enum SleepReason { idle, grace, background, app }

/// 本实例的唤醒描述，随 `app/sleep` 上报给 Host。
final class WakeDescriptor {
  const WakeDescriptor(this.kind, {this.target, this.background = false});

  /// iOS / macOS / 未打包 Windows：自定义 URL scheme（Host 打开 `<scheme>://app-mcp/wake?token=`）。
  const WakeDescriptor.uri(String scheme, {bool background = false})
      : this(WakeKind.uri, target: scheme, background: background);

  /// Android：显式广播到 `WakeReceiver`，[component] 为 `包名/接收器类名`。
  const WakeDescriptor.androidIntent(String component)
      : this(WakeKind.androidIntent, target: component, background: true);

  final WakeKind kind;

  /// scheme、AUMID、bundle id、D-Bus 名、组件名或 URL。
  final String? target;

  /// 能否不把窗口带到前台就唤醒。
  final bool background;

  @override
  bool operator ==(Object other) =>
      other is WakeDescriptor &&
      other.kind == kind &&
      other.target == target &&
      other.background == background;

  @override
  int get hashCode => Object.hash(kind, target, background);

  @override
  String toString() => 'WakeDescriptor(${kind.name}, $target, background: $background)';
}

/// 生命周期策略。
final class LifecyclePolicy {
  const LifecyclePolicy({
    this.mode = LifecycleMode.persistent,
    this.idleTimeout = const Duration(seconds: 60),
    this.hiddenIdleTimeout = const Duration(seconds: 15),
    this.grace = const Duration(seconds: 10),
    this.residency = Residency.keep,
    this.wake,
    this.hostAbsentRetries = 3,
    this.legacyTimers = false,
    this.mergeWindow = const Duration(seconds: 2),
    this.sleepOnBackground = false,
  });

  /// 按平台选择默认值（spec/lifecycle.md 第 13 节 B1"平台默认"）：Android / iOS 为 [LifecycleMode.onDemand]
  /// + [sleepOnBackground]、[Residency.keep]（iOS 另设 `hiddenIdleTimeout = 0`）；其他平台为 [LifecycleMode.persistent]。
  ///
  /// @why 桌面不默认 `idle`：本封装没有单实例重定向，休眠后经 URI / 清单 `launch` 唤醒会冷启动新进程而不是回连本实例。
  /// 有可靠唤醒入口（如 macOS URL scheme、自行实现的单实例转交）的桌面 App 显式传入 `idle`。
  factory LifecyclePolicy.platformDefault({
    required bool isAndroid,
    required bool isIOS,
    WakeDescriptor? wake,
  }) {
    if (isIOS) {
      return LifecyclePolicy(
          mode: LifecycleMode.onDemand, hiddenIdleTimeout: Duration.zero, sleepOnBackground: true, wake: wake);
    }
    if (isAndroid) return LifecyclePolicy(mode: LifecycleMode.onDemand, sleepOnBackground: true, wake: wake);
    return LifecyclePolicy(wake: wake);
  }

  final LifecycleMode mode;

  /// `idle` 模式下空闲多久进入休眠。
  final Duration idleTimeout;

  /// 可见性为 hidden / frozen 时使用的更短空闲时间。
  final Duration hiddenIdleTimeout;

  /// `onDemand` 模式下任务完成后保留连接的时间。
  final Duration grace;
  final Residency residency;

  /// 未设置时 Host 回退到清单 `launch`。
  final WakeDescriptor? wake;

  /// `idle` / `onDemand` 下连续多少次"Host 不在"后转休眠（第 11 节 A2）；0 = 一直重连。
  final int hostAbsentRetries;

  /// true：回退到 4e 之前的定时器行为（第 11、13 节）。
  final bool legacyTimers;

  /// 调用 / 资源读取后的合并窗口（第 13 节 B1）；[Duration.zero] = 不留窗口（调用后只看租约）。
  final Duration mergeWindow;

  /// true：`idle` / `onDemand` 下进入后台（可见 → 隐藏 / 冻结）且空闲时立即休眠，不等租约（第 13 节 B4）。
  final bool sleepOnBackground;

  LifecyclePolicy copyWith({
    LifecycleMode? mode,
    Duration? idleTimeout,
    Duration? hiddenIdleTimeout,
    Duration? grace,
    Residency? residency,
    WakeDescriptor? wake,
    int? hostAbsentRetries,
    bool? legacyTimers,
    Duration? mergeWindow,
    bool? sleepOnBackground,
  }) =>
      LifecyclePolicy(
        mode: mode ?? this.mode,
        idleTimeout: idleTimeout ?? this.idleTimeout,
        hiddenIdleTimeout: hiddenIdleTimeout ?? this.hiddenIdleTimeout,
        grace: grace ?? this.grace,
        residency: residency ?? this.residency,
        wake: wake ?? this.wake,
        hostAbsentRetries: hostAbsentRetries ?? this.hostAbsentRetries,
        legacyTimers: legacyTimers ?? this.legacyTimers,
        mergeWindow: mergeWindow ?? this.mergeWindow,
        sleepOnBackground: sleepOnBackground ?? this.sleepOnBackground,
      );

  @override
  bool operator ==(Object other) =>
      other is LifecyclePolicy &&
      other.mode == mode &&
      other.idleTimeout == idleTimeout &&
      other.hiddenIdleTimeout == hiddenIdleTimeout &&
      other.grace == grace &&
      other.residency == residency &&
      other.wake == wake &&
      other.hostAbsentRetries == hostAbsentRetries &&
      other.legacyTimers == legacyTimers &&
      other.mergeWindow == mergeWindow &&
      other.sleepOnBackground == sleepOnBackground;

  @override
  int get hashCode => Object.hash(mode, idleTimeout, hiddenIdleTimeout, grace, residency, wake,
      hostAbsentRetries, legacyTimers, mergeWindow, sleepOnBackground);

  @override
  String toString() => 'LifecyclePolicy(${mode.name}, idle: $idleTimeout, hidden: $hiddenIdleTimeout, '
      'grace: $grace, residency: ${residency.name}, wake: $wake, hostAbsentRetries: $hostAbsentRetries, '
      'legacyTimers: $legacyTimers, mergeWindow: $mergeWindow, sleepOnBackground: $sleepOnBackground)';
}

/// 工具 handler。参数已由 Host 按 inputSchema 校验。
typedef ToolHandler = FutureOr<Object?> Function(Map<String, dynamic> args, ToolContext ctx);

/// 资源读取函数，返回可被 `jsonEncode` 编码的内容。
typedef ResourceReader = FutureOr<Object?> Function();

/// 工具定义（不含 handler）。不可变。
final class ToolSpec {
  const ToolSpec({
    required this.name,
    required this.description,
    this.inputSchema,
    this.risk = Risk.write,
    this.activation,
    this.title,
    this.enabled = true,
  });

  /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
  final String name;
  final String description;

  /// JSON Schema，顶层 `type` 必须为 `object`；为 null 表示无参数。
  final Map<String, Object?>? inputSchema;
  final Risk risk;
  final Activation? activation;
  final String? title;
  final bool enabled;

  ToolSpec copyWith({
    String? description,
    Map<String, Object?>? inputSchema,
    Risk? risk,
    Activation? activation,
    String? title,
    bool? enabled,
  }) =>
      ToolSpec(
        name: name,
        description: description ?? this.description,
        inputSchema: inputSchema ?? this.inputSchema,
        risk: risk ?? this.risk,
        activation: activation ?? this.activation,
        title: title ?? this.title,
        enabled: enabled ?? this.enabled,
      );

  @override
  bool operator ==(Object other) =>
      other is ToolSpec &&
      other.name == name &&
      other.description == description &&
      other.risk == risk &&
      other.activation == activation &&
      other.title == title &&
      other.enabled == enabled &&
      _schemaText(other.inputSchema) == _schemaText(inputSchema);

  @override
  int get hashCode =>
      Object.hash(name, description, risk, activation, title, enabled, _schemaText(inputSchema));

  static String? _schemaText(Map<String, Object?>? schema) =>
      schema == null ? null : jsonEncode(schema);
}

/// App 总览：模型第一次接触该 App 时由 Host 附带（见 spec/protocol.md 第 7 节）。
final class AppOverview {
  const AppOverview({required this.summary, this.body, this.locale});

  /// 一句话简介（≤ 100 字符），出现在 MCP `instructions` 与 `apps.list` 中。
  final String summary;

  /// 总览正文（Markdown，≤ 2000 字符）。建议小节：适用场景、能力范围、典型流程、
  /// 前置条件、不支持的操作、风险说明。
  final String? body;

  /// 语言，如 `zh-CN`。
  final String? locale;
}

/// 资源定义。
final class ResourceSpec {
  const ResourceSpec({required this.name, required this.description, this.mimeType, this.realtime = false});

  final String name;
  final String description;

  /// 为 null 时为 `application/json`。
  final String? mimeType;

  /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被订阅时保持连接、休眠中变化时回连推送。
  final bool realtime;
}
