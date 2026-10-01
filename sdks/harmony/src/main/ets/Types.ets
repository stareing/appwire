/**
 * @app-mcp/harmony 公开类型。
 *
 * 取值与 @app-mcp/node / @app-mcp/web 一致（spec/protocol.md、spec/lifecycle.md）；写法遵守 ArkTS 约束
 * （无 any / unknown、无结构类型对象字面量、无工具类型），因此个别形状与 TS 版不同：
 * - 工具输入 schema 用 JSON 文本（`inputSchema: string`）；
 * - 连接状态是单一接口（`status` + 可选字段），而不是联合类型；
 * - 取消用 `ToolContext.onCancel` / `isCancelled`（ArkTS 运行时没有 AbortSignal）；
 * - 带 stateHints 的结果用 `ToolResult` 类表示。
 */

export type Risk = 'read' | 'write' | 'destructive' | 'payment' | 'os-sensitive';
export type Activation = 'headless' | 'background' | 'foreground';
export type Visibility = 'visible' | 'hidden' | 'frozen';

/** 协议错误类别，见 spec/protocol.md 第 4 节。 */
export type ErrorKind =
  'TOOL_NOT_FOUND' | 'TOOL_DISABLED' | 'INVALID_INPUT' | 'USER_REJECTED' | 'TIMEOUT' | 'HANDLER_ERROR' |
  'CANCELLED' | 'APP_DISCONNECTED' | 'APP_NOT_INSTALLED' | 'LAUNCH_FAILED' | 'APP_NOT_RESPONDING' |
  'INSTANCE_FROZEN' | 'RESOURCE_NOT_FOUND' | 'UNAUTHORIZED' | 'UNSUPPORTED_PROTOCOL';

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

/** 本实例的唤醒描述（spec/lifecycle.md 第 5 节）。鸿蒙用 `uri`：`<scheme>://app-mcp/wake`。 */
export interface WakeDescriptor {
  kind: WakeKind;
  target?: string;
  background?: boolean;
}

export interface LifecycleOptions {
  /** 缺省 `persistent`；`HarmonyAppMcp.create` 缺省 `idle`。 */
  mode?: LifecycleMode;
  idleTimeoutMs?: number;
  hiddenIdleTimeoutMs?: number;
  graceMs?: number;
  residency?: Residency;
  wake?: WakeDescriptor;
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
}

/** 带状态提示的结果；handler 也可以直接返回数据。 */
export class ToolResult<O> {
  readonly data: O;
  readonly stateHints: string[];

  constructor(data: O, stateHints: string[]) {
    this.data = data;
    this.stateHints = stateHints;
  }
}

/** 工具 handler：`input` 为 `JSON.parse(参数)` 的结果（已由 Host 按 inputSchema 校验）。 */
export type ToolHandler<I, O> = (input: I, context: ToolContext) => O | ToolResult<O> | Promise<O | ToolResult<O>>;

export interface ToolDefinition<I, O> {
  description: string;
  title?: string;
  /** 输入的 JSON Schema 文本（顶层 `type` 为 `"object"`）；缺省为无参数。 */
  inputSchema?: string;
  /** 缺省 `write`。 */
  risk?: Risk;
  activation?: Activation;
  /** 缺省 true。 */
  enabled?: boolean;
  handler: ToolHandler<I, O>;
}

/** `ToolHandle.update` 的变更；未提供的字段保持不变。 */
export interface ToolChanges {
  description?: string;
  title?: string;
  inputSchema?: string;
  risk?: Risk;
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
  /** 已休眠且 `lifecycle.residency` 允许退出进程时调用；SDK 不会自行退出（可在此 `terminateSelf()`）。 */
  onIdleExit?: () => void;
}
