/**
 * @app-mcp/harmony 公开类型。
 *
 * 取值与 @app-mcp/node / @app-mcp/web 一致（spec/protocol.md、spec/lifecycle.md）；写法遵守 ArkTS 约束
 * （无 any / unknown、无结构类型对象字面量、无工具类型），因此个别形状与 TS 版不同：
 * - 工具输入 schema 用 JSON 文本（`inputSchema: string`）；
 * - 连接状态是单一接口（`status` + 可选字段），而不是联合类型；
 * - 取消用 `ToolContext.onCancel` / `isCancelled`（ArkTS 运行时没有 AbortSignal）；
 * - 工具输出 schema 同样用 JSON 文本（`outputSchema: string`）；
 * - 结构化结果（stateHints、status、summary 等）用 `ToolResult` 类表示。
 */

export type Risk = 'read' | 'write' | 'destructive' | 'payment' | 'os-sensitive';
export type Activation = 'headless' | 'background' | 'foreground';
export type Visibility = 'visible' | 'hidden' | 'frozen';

/** 协议错误类别，见 spec/protocol.md 第 4 节。 */
export type ErrorKind =
  'TOOL_NOT_FOUND' | 'TOOL_DISABLED' | 'INVALID_INPUT' | 'USER_REJECTED' | 'TIMEOUT' | 'HANDLER_ERROR' |
  'CANCELLED' | 'APP_DISCONNECTED' | 'APP_NOT_INSTALLED' | 'LAUNCH_FAILED' | 'APP_NOT_RESPONDING' |
  'INSTANCE_FROZEN' | 'RESOURCE_NOT_FOUND' | 'UNAUTHORIZED' | 'UNSUPPORTED_PROTOCOL' |
  // Host 产生（限流 / 超过大小上限 / 策略拒绝），App 一般不抛
  'RATE_LIMITED' | 'PAYLOAD_TOO_LARGE' | 'POLICY_DENIED' |
  // 需要用户本人操作后才能继续，由 handler 抛出（见 ToolCallError.userActionRequired）
  'USER_ACTION_REQUIRED';

/** `ToolCallError.userActionRequired` 的可选项；未给出的字段不出现在错误详情中。 */
export interface UserActionRequiredOptions {
  /** 类别，建议取 `login`（登录过期）/ `permission`（系统权限未授予）/ `foreground`（需切到前台）/ `confirm`（需在 App 内确认），也可以是其他字符串。 */
  reason?: string;
  /** App 内入口（如 deep link），供 Agent / 用户打开。 */
  uri?: string;
}

/**
 * 标准 MCP 工具注解（spec/protocol.md 第 3 节），原样转发给 Agent，由 Agent 决定是否确认 / 放行。
 * 与旧写法 `risk` 同时给出时，声明的字段逐个优先，缺少的按 `risk` 推导。
 */
export interface ToolAnnotations {
  /** 给人看的工具标题。 */
  title?: string;
  /** 不修改任何状态。 */
  readOnlyHint?: boolean;
  /** 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。 */
  destructiveHint?: boolean;
  /** 以相同参数重复调用没有额外效果（只在非只读时有意义）。 */
  idempotentHint?: boolean;
  /** 会与外部世界交互（网络、第三方、其他用户可见）。 */
  openWorldHint?: boolean;
}

/** 内容的接收方（MCP `Role`）。 */
export type Audience = 'user' | 'assistant';

/** 标准 MCP 内容注解：对结果内容的标注，Hub 原样转发。 */
export interface ContentAnnotations {
  audience?: Audience[];
  /** 重要程度，0（可选）到 1（必需）。 */
  priority?: number;
  /** 最后修改时刻（ISO 8601）。 */
  lastModified?: string;
}

/**
 * 调用结果的业务状态（spec/protocol.md 3.2）：`done`（缺省）已完成；`pending` 已受理、待用户在 App 内确认或异步完成
 * （后续状态见 `stateResource`）；`partial` 只完成了一部分（说明见 `summary`）；`noop` 没有做任何改动。
 */
export type ResultStatus = 'done' | 'pending' | 'partial' | 'noop';

export type ConnectionStatus =
  'disabled' | 'idle' | 'connecting' | 'handshaking' | 'pending-pairing' | 'connected' | 'backoff' |
  'rejected' | 'stopped' | 'dormant' | 'waking' | 'host-mismatch';

/**
 * 连接状态。`backoff` 时 `retryAt` 为下次重连的时间戳（毫秒）；`backoff` / `rejected` / `host-mismatch` 时
 * `reason` 为中文原因、`code` 为错误码（spec/protocol.md 10.1）。
 */
export interface ConnectionState {
  status: ConnectionStatus;
  retryAt?: number;
  reason?: string;
  code?: string;
}

// ---- 生命周期（spec/lifecycle.md）---------------------------------------------

export type LifecycleMode = 'persistent' | 'idle' | 'on-demand';
export type Residency = 'keep' | 'exit-when-idle' | 'exit-always';
export type WakeKind = 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none';
export type WakeReason = 'app' | 'visible';
export type CancelReason = 'requested' | 'timeout' | 'disconnected' | 'stopped';
/** 心跳策略（spec/lifecycle.md 第 11 节 A3）：`auto` 按传输决定（鸿蒙上回环地址按远程，发心跳）。 */
export type HeartbeatMode = 'auto' | 'always' | 'off';

/** 本实例的唤醒描述（spec/lifecycle.md 第 5 节）。鸿蒙用 `uri`：`<scheme>://app-mcp/wake`。 */
export interface WakeDescriptor {
  kind: WakeKind;
  target?: string;
  background?: boolean;
}

/** 未提供的字段取原生层默认值（spec/lifecycle.md 第 3 节）；`HarmonyAppMcp.create` 未给出时用 `HarmonyAppMcp.defaultLifecycle`。 */
export interface LifecycleOptions {
  /** 缺省 `persistent`；`HarmonyAppMcp.defaultLifecycle` 为 `on-demand`。 */
  mode?: LifecycleMode;
  idleTimeoutMs?: number;
  hiddenIdleTimeoutMs?: number;
  graceMs?: number;
  residency?: Residency;
  wake?: WakeDescriptor;
  /** `idle` / `on-demand` 下连续多少次"Host 不在"后转 `dormant`；缺省 3，0 = 一直重连（第 11 节 A2）。 */
  hostAbsentRetries?: number;
  /** 回退到 4e 之前的定时器行为（第 11、13 节）；缺省 false。 */
  legacyTimers?: boolean;
  /** 调用 / 资源读取后的合并窗口（毫秒，第 13 节 B1）；缺省 2000。 */
  mergeWindowMs?: number;
  /** 进入后台且空闲时立即休眠，不等租约（第 13 节 B4）；缺省 false，`HarmonyAppMcp.defaultLifecycle` 为 true。 */
  sleepOnBackground?: boolean;
}

export interface AppOverview {
  /** 一句话简介，≤ 100 字符。 */
  summary: string;
  /** 总览正文（Markdown），≤ 2000 字符。 */
  body?: string;
  locale?: string;
}

/** 日志输出。缺省写 hilog（见 Harmony.ets）或 console。 */
export interface Logger {
  debug(message: string): void;
  warn(message: string): void;
  error(message: string): void;
}

/** 阻止自动休眠的持有；`release()` 幂等。 */
export interface HoldHandle {
  release(): void;
}

// ---- 工具与资源 ----------------------------------------------------------------

export interface ToolContext {
  /** Host 生成的调用 ID。 */
  readonly callId: string;
  /** 调用是否已被取消（Host 取消、超时、断线、停止）。 */
  isCancelled(): boolean;
  /** 取消时回调（已取消时也会经事件循环回调一次）。 */
  onCancel(listener: (reason: CancelReason) => void): void;
  /** handler 返回后仍阻止自动休眠，直到释放。调用已结束时抛出（`code` 为 `ALREADY_COMPLETED`）。 */
  hold(): HoldHandle;
  /**
   * 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。`progress` 应递增
   * （不递增的值被 Host 丢弃），`total` 未知时省略。调用已结束、已取消或未连接时无副作用（不抛出）。
   */
  progress(progress: number, total?: number, message?: string): void;
}

/** {@link ToolResult} 的可选字段。 */
export interface ToolResultOptions {
  /** 业务状态，缺省 `done`。 */
  status?: ResultStatus;
  /** `pending` 时可读取后续状态的资源名（局部名）。 */
  stateResource?: string;
  /** 一句面向模型 / 用户的结论（`partial` 时说明完成了哪部分）。 */
  summary?: string;
  /** 结果内容的标注。 */
  annotations?: ContentAnnotations;
}

/**
 * 结构化结果（状态提示、业务状态、摘要、内容标注）；handler 也可以直接返回数据。
 * `data` 为 undefined / null 表示无返回值（Hub 对模型输出"已完成"）。
 */
export class ToolResult<O> {
  readonly data: O;
  readonly stateHints: string[];
  readonly status?: ResultStatus;
  readonly stateResource?: string;
  readonly summary?: string;
  readonly annotations?: ContentAnnotations;

  constructor(data: O, stateHints: string[], options?: ToolResultOptions) {
    this.data = data;
    this.stateHints = stateHints;
    if (options !== undefined) {
      this.status = options.status;
      this.stateResource = options.stateResource;
      this.summary = options.summary;
      this.annotations = options.annotations;
    }
  }
}

/** 工具 handler：`input` 为 `JSON.parse(参数)` 的结果（已由 Host 按 inputSchema 校验）。 */
export type ToolHandler<I, O> = (input: I, context: ToolContext) => O | ToolResult<O> | Promise<O | ToolResult<O>>;

export interface ToolDefinition<I, O> {
  description: string;
  title?: string;
  /** 输入的 JSON Schema 文本（顶层 `type` 为 `"object"`）；缺省为无参数。 */
  inputSchema?: string;
  /** 旧写法：缺省 `write`。新代码优先用 `annotations`；两者同时给出时 `annotations` 声明的字段优先。 */
  risk?: Risk;
  /** 标准 MCP 工具注解（只读、破坏性、幂等、开放世界）。 */
  annotations?: ToolAnnotations;
  /** 结果的 JSON Schema 文本（MCP `outputSchema`，根类型不限）；缺省不声明。 */
  outputSchema?: string;
  activation?: Activation;
  /** 缺省 true。 */
  enabled?: boolean;
  handler: ToolHandler<I, O>;
}

/** `ToolHandle.update` 的变更；未提供的字段保持不变（`annotations` / `outputSchema` 整体替换，不支持清除）。 */
export interface ToolChanges {
  description?: string;
  title?: string;
  inputSchema?: string;
  risk?: Risk;
  annotations?: ToolAnnotations;
  outputSchema?: string;
  activation?: Activation;
  enabled?: boolean;
}

export interface ToolHandle {
  readonly name: string;
  update(changes: ToolChanges): void;
  dispose(): void;
}

export interface ResourceDefinition<T> {
  description: string;
  /** 缺省 `application/json`。 */
  mimeType?: string;
  /** 模型在等待其变化：被订阅时保持连接、休眠中变化时回连推送（spec/lifecycle.md 第 13 节 B3）。缺省 false。 */
  realtime?: boolean;
  read: () => T | Promise<T>;
}

export interface ResourceHandle {
  readonly name: string;
  /** 内容已变化；只有 Host 订阅时才会发送通知，且会节流。 */
  notifyChanged(): void;
  dispose(): void;
}

export interface Registrar {
  tool<I, O>(name: string, definition: ToolDefinition<I, O>): ToolHandle;
  resource<T>(name: string, definition: ResourceDefinition<T>): ResourceHandle;
  scope(name: string): Scope;
}

export interface Scope extends Registrar {
  readonly name: string;
  /** 注销该 scope 下的全部工具、资源与子 scope。 */
  dispose(): void;
}

// ---- 创建选项 ------------------------------------------------------------------

export interface AppMcpOptions {
  /** `[a-z][a-z0-9-]{0,62}` */
  appId: string;
  appName: string;
  appVersion?: string;
  /** 为 false 时不加载原生模块、不连接，注册调用为空操作。默认 true。 */
  enabled?: boolean;
  /** Host 端点；缺省由原生层按 spec/protocol.md 1.3 解析，鸿蒙上为 `ws://127.0.0.1:7717/app`（开发机用 `hdc rport tcp:7717 tcp:7717`）。 */
  hostUrl?: string;
  maxConcurrentCalls?: number;
  overview?: AppOverview;
  logger?: Logger;
  instanceId?: string;
  instanceTitle?: string;
  /** 之前配对得到的 token（由 `onPaired` 持久化，如 preferences）。 */
  token?: string;
  /** Host 唤醒时提供的一次性令牌；通常经 `handleWant` 传入，不在此设置。 */
  launchToken?: string;
  onPaired?: (token: string) => void;
  /** 创建后立即连接。默认 true。 */
  autoStart?: boolean;
  lifecycle?: LifecycleOptions;
  connectTimeoutMs?: number;
  /** 缺省 `auto`。在设备上嵌入 Hub 时可设 `off`。 */
  heartbeat?: HeartbeatMode;
  /** 已休眠且 `lifecycle.residency` 允许退出进程时调用；SDK 不会自行退出（可在此 `terminateSelf()`）。 */
  onIdleExit?: () => void;
}
