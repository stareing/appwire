// 公开的基础类型（与协议一致，见 spec/protocol.md）。纯 Dart，不依赖 FFI。

import 'dart:async';
import 'dart:convert';

export 'lifecycle_types.dart';

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
  unsupportedProtocol('UNSUPPORTED_PROTOCOL'),

  /// Host 侧限流（由 Host 产生，App 一般不用）。
  rateLimited('RATE_LIMITED'),

  /// 调用参数、结果或资源内容超过 Host 的大小上限（由 Host 产生，App 一般不用）。
  payloadTooLarge('PAYLOAD_TOO_LARGE'),

  /// 调用被用户 / 厂商的策略规则拒绝（由 Host 产生，App 一般不用）。
  policyDenied('POLICY_DENIED'),

  /// 需要用户本人操作后才能继续（登录过期、系统权限未授予、需切到前台、需在 App 内确认等）；
  /// 用 [UserActionRequiredError] 抛出可附带 reason / uri。
  userActionRequired('USER_ACTION_REQUIRED'),

  /// 导航没有完成（不支持 / 出错 / 超时 / 导航后工具未出现，spec/protocol.md 3.4）。
  navigationFailed('NAVIGATION_FAILED'),

  /// 导航被拒绝（App 拒绝或页面不可由 Agent 导航）。
  navigationDenied('NAVIGATION_DENIED'),

  /// 对象锁冲突（由 Host 产生，App 一般不用；spec/hub-api.md 3.6「对象锁」）。
  locked('LOCKED');

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

/// `USER_ACTION_REQUIRED` 的 `data.reason` 建议取值（spec/protocol.md 第 4 节；也可用其他字符串）。
abstract final class UserActionReason {
  /// 登录已过期 / 未登录。
  static const login = 'login';

  /// 系统权限未授予（相机、位置、通知等）。
  static const permission = 'permission';

  /// 需要把 App 切到前台。
  static const foreground = 'foreground';

  /// 需要用户在 App 内确认。
  static const confirm = 'confirm';
}

/// handler 抛出此错误以 `USER_ACTION_REQUIRED` 失败（app_mcp.h v11）：需要用户本人操作后才能继续。
///
/// [message] 面向用户（Agent 转告用户）；[reason]（见 [UserActionReason]）与 [uri]（App 内入口，如深链接）可选，
/// 为 null 时不出现在协议错误的 `data` 中。
class UserActionRequiredError extends ToolCallError {
  UserActionRequiredError(String message, {this.reason, this.uri})
      : super(ErrorKind.userActionRequired, message);

  final String? reason;
  final String? uri;
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

/// 调用结果的业务状态（spec/protocol.md 3.2）。
enum ToolResultStatus {
  /// 已完成（缺省）。
  done('done'),

  /// 已受理、尚未完成（等待用户在 App 内确认或异步处理）；后续状态见 [ToolResult.stateResource]。
  pending('pending'),

  /// 只完成了一部分，说明见 [ToolResult.summary]。
  partial('partial'),

  /// 没有做任何改动（目标状态已满足或无事可做）。
  noop('noop');

  const ToolResultStatus(this.wireName);
  final String wireName;
}

/// 内容面向谁（MCP 内容注解 audience）。
enum ContentAudience {
  user('user'),
  assistant('assistant');

  const ContentAudience(this.wireName);
  final String wireName;
}

/// 结果内容的标注（MCP 内容注解），Host 原样转发；为 null 的字段不声明。
final class ContentAnnotations {
  const ContentAnnotations({this.audience, this.priority, this.lastModified});

  final List<ContentAudience>? audience;

  /// 重要程度，0（可选）到 1（必需）。
  final double? priority;

  /// 最后修改时刻（ISO 8601）。
  final String? lastModified;

  /// 协议 JSON 对象（省略 null 字段）。
  Map<String, Object> toJson() => {
        if (audience != null) 'audience': [for (final a in audience!) a.wireName],
        if (priority != null) 'priority': priority!,
        if (lastModified != null) 'lastModified': lastModified!,
      };
}

/// 标准 MCP 工具注解（spec/protocol.md 第 3 节）。本库不据此做判断，只原样转发；为 null 的字段不声明。
final class ToolAnnotations {
  const ToolAnnotations({
    this.title,
    this.readOnlyHint,
    this.destructiveHint,
    this.idempotentHint,
    this.openWorldHint,
  });

  /// 给人看的工具标题。
  final String? title;

  /// 不修改任何状态。
  final bool? readOnlyHint;

  /// 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。
  final bool? destructiveHint;

  /// 以相同参数重复调用没有额外效果（只在非只读时有意义）。
  final bool? idempotentHint;

  /// 会与外部世界交互（网络、第三方、其他用户可见）。
  final bool? openWorldHint;

  /// 协议 JSON 对象（省略 null 字段）。
  Map<String, Object> toJson() => {
        if (title != null) 'title': title!,
        if (readOnlyHint != null) 'readOnlyHint': readOnlyHint!,
        if (destructiveHint != null) 'destructiveHint': destructiveHint!,
        if (idempotentHint != null) 'idempotentHint': idempotentHint!,
        if (openWorldHint != null) 'openWorldHint': openWorldHint!,
      };

  @override
  bool operator ==(Object other) =>
      other is ToolAnnotations &&
      other.title == title &&
      other.readOnlyHint == readOnlyHint &&
      other.destructiveHint == destructiveHint &&
      other.idempotentHint == idempotentHint &&
      other.openWorldHint == openWorldHint;

  @override
  int get hashCode => Object.hash(title, readOnlyHint, destructiveHint, idempotentHint, openWorldHint);
}

/// handler 可以直接返回数据，也可以返回 [ToolResult] 以附带 stateHints、业务状态、摘要与内容注解。
/// 直接返回普通值 = 只有 [data] 的 done 结果。
final class ToolResult {
  const ToolResult(
    this.data, {
    this.stateHints = const [],
    this.status = ToolResultStatus.done,
    this.stateResource,
    this.summary,
    this.annotations,
  });

  /// 可被 `jsonEncode` 编码的数据；null 表示无返回值（Host 对模型输出"已完成"）。
  final Object? data;

  /// 调用后可能变化的资源名（提示 Host 重新读取）。
  final List<String> stateHints;

  /// 业务状态，缺省 [ToolResultStatus.done]。
  final ToolResultStatus status;

  /// [ToolResultStatus.pending] 时可读取后续状态的资源名。
  final String? stateResource;

  /// 一句面向模型 / 用户的结论（partial 时说明完成了哪部分）。
  final String? summary;

  /// 结果内容的标注（MCP 内容注解）。
  final ContentAnnotations? annotations;
}

/// 一次工具调用的上下文。
abstract interface class ToolContext {
  /// Host 生成的调用 ID。
  String get callId;

  /// 被调用的工具名。
  String get toolName;

  /// Agent 给出的幂等键（原样，spec/protocol.md 3.3「idempotencyKey」）；没有时为 null。
  /// App 自行决定如何使用（如作为业务去重键、传给后端）；同一工具同一键的重复调用已由核心按首次结果重放。
  String? get idempotencyKey;

  /// 调用是否已被取消（Host 取消、超时、断线、停止）。
  bool get isCancelled;

  /// 取消原因；未取消时为 null。
  CancelReason? get cancelReason;

  /// 调用被取消时完成；调用正常结束时永不完成。
  Future<CancelReason> get cancelled;

  /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的持有被释放。
  /// 必须在调用完成前获取；调用已结束时抛出 [AppMcpException]（`alreadyCompleted`）。
  McpHold hold();

  /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。
  /// [progress] 应递增（不递增的值被 Host 丢弃），[total] 未知时省略。调用已结束、已取消或未连接时无副作用。
  void progress(double progress, {double? total, String? message});
}

/// 阻止自动休眠的持有（[ToolContext.hold]、`AppMcp.hold`）。[release] 幂等；
/// 对象被垃圾回收时也会自动释放，但请显式释放以免休眠被无限推迟。
abstract interface class McpHold {
  void release();
  bool get isReleased;
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
    this.annotations,
    this.outputSchema,
    this.surface = ToolSurface.app,
    this.page,
    this.backgroundTool,
    this.concurrency = 0,
    this.exclusive,
  });

  /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
  final String name;
  final String description;

  /// JSON Schema，顶层 `type` 必须为 `object`；为 null 表示无参数。
  final Map<String, Object?>? inputSchema;

  /// 旧写法：优先用 [annotations]。两者同时声明时注解中的字段优先，缺少的按 risk 推导。
  final Risk risk;
  final Activation? activation;
  final String? title;
  final bool enabled;

  /// 标准 MCP 工具注解，原样转发给 Agent；为 null 时不声明（Host 按 [risk] 推导）。
  final ToolAnnotations? annotations;

  /// 结果的 JSON Schema（MCP outputSchema）；为 null 时不声明。
  final Map<String, Object?>? outputSchema;

  /// 对界面的依赖（spec/protocol.md 3.4）：[ToolSurface.view] 的工具只在所在界面可见且处于最上层时注册 / 启用
  /// （Flutter 可用 `app_mcp_flutter` 的 `McpTool` + `McpRouteObserver` 按路由门控）。
  final ToolSurface surface;

  /// 所在页面名（`[a-zA-Z0-9_.-]{1,64}`）；为 null 时不声明。Hub 在该工具未注册时据此导航（[AppMcp.setNavigationHandler]）。
  final String? page;

  /// 只对 [ToolSurface.view] 有意义：App 在后台、本工具不可调用时 Hub 改调的同 App app 工具本地名；为 null 时不声明
  /// （spec/protocol.md 3.4「后台与前台」）。
  final String? backgroundTool;

  /// 本工具同时执行的调用上限；0 = 不单独限制，只受 `maxConcurrentCalls` 约束。只在 SDK 内调度，不同步给 Host
  /// （spec/protocol.md 5.3）。
  final int concurrency;

  /// 互斥组名（`[a-zA-Z0-9_.-]{1,64}`）：同组的工具同一时刻至多一个在执行；为 null 时不互斥（spec/protocol.md 5.3）。
  final String? exclusive;

  ToolSpec copyWith({
    String? description,
    Map<String, Object?>? inputSchema,
    Risk? risk,
    Activation? activation,
    String? title,
    bool? enabled,
    ToolAnnotations? annotations,
    Map<String, Object?>? outputSchema,
    ToolSurface? surface,
    String? page,
    String? backgroundTool,
    int? concurrency,
    String? exclusive,
  }) =>
      ToolSpec(
        name: name,
        description: description ?? this.description,
        inputSchema: inputSchema ?? this.inputSchema,
        risk: risk ?? this.risk,
        activation: activation ?? this.activation,
        title: title ?? this.title,
        enabled: enabled ?? this.enabled,
        annotations: annotations ?? this.annotations,
        outputSchema: outputSchema ?? this.outputSchema,
        surface: surface ?? this.surface,
        page: page ?? this.page,
        backgroundTool: backgroundTool ?? this.backgroundTool,
        concurrency: concurrency ?? this.concurrency,
        exclusive: exclusive ?? this.exclusive,
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
      other.annotations == annotations &&
      other.surface == surface &&
      other.page == page &&
      other.backgroundTool == backgroundTool &&
      other.concurrency == concurrency &&
      other.exclusive == exclusive &&
      _schemaText(other.inputSchema) == _schemaText(inputSchema) &&
      _schemaText(other.outputSchema) == _schemaText(outputSchema);

  @override
  int get hashCode => Object.hash(name, description, risk, activation, title, enabled, annotations,
      _schemaText(inputSchema), _schemaText(outputSchema), surface, page, backgroundTool, concurrency, exclusive);

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
  const ResourceSpec(
      {required this.name, required this.description, this.mimeType, this.realtime = false, this.annotations});

  final String name;
  final String description;

  /// 为 null 时为 `application/json`。
  final String? mimeType;

  /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被订阅时保持连接、休眠中变化时回连推送。
  final bool realtime;

  /// 资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上；null 表示不声明。
  final ContentAnnotations? annotations;
}

/// 工具对界面的依赖（spec/protocol.md 3.4）。
enum ToolSurface {
  /// 不依赖界面：后台可调、可唤醒（缺省）。
  app,

  /// 依赖界面：只在所在界面可见且处于最上层时注册。
  view,
}

/// 一次导航请求（Host 的 `app/navigate`，spec/protocol.md 3.4）。
final class NavigationRequest {
  const NavigationRequest(this.page, this.paramsJson);

  /// 目标页面名（清单 `pages[].name` 或工具的 [ToolSpec.page]）。
  final String page;

  /// 页面参数 JSON 文本；Host 没有给出时为 null。
  final String? paramsJson;

  /// 页面参数（解码后的 JSON）；Host 没有给出时为 null。
  Object? get params => paramsJson == null ? null : jsonDecode(paramsJson!);

  @override
  String toString() => 'NavigationRequest($page, $paramsJson)';
}

/// 在导航回调中抛出：拒绝本次导航（`NAVIGATION_DENIED`，如用户正在输入、页面需要登录）。[message] 面向模型 / 用户。
final class NavigationDeniedError implements Exception {
  const NavigationDeniedError(this.message);
  final String message;

  @override
  String toString() => 'NavigationDeniedError: $message';
}

/// 导航回调（[AppMcp.setNavigationHandler]）：切换到 [NavigationRequest.page] 后返回（最好在新页面的工具注册之后）。
/// 抛 [NavigationDeniedError] = 拒绝；其他异常 = 失败（`NAVIGATION_FAILED`）。
typedef NavigationHandler = FutureOr<void> Function(NavigationRequest request);
