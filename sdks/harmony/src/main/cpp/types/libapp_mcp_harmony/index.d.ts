/**
 * libapp_mcp_harmony.so（bindings/harmony）的导出。
 *
 * 与 bindings/node 编译自同一份 Rust 源码（napi-rs / napi-ohos），JS 形状逐字相同。
 * `ClientConfig.hostUrl` 缺省时按 spec/protocol.md 1.3 解析（鸿蒙沙箱为 `ws://127.0.0.1:7717/app`）。回调在创建 `NativeClient` 的 ArkTS 线程的事件循环上执行。
 * 所有方法抛出的 Error 带 `code`（`INVALID_ARG`、`DUPLICATE_NAME`、`ALREADY_COMPLETED`、`STOPPED` 等）。
 */

export interface OverviewInit {
  summary: string;
  body?: string;
  locale?: string;
}

export interface WakeInit {
  /** 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none' */
  kind: string;
  target?: string;
  background?: boolean;
}

export interface LifecycleInit {
  /** 'persistent' | 'idle' | 'on-demand' */
  mode?: string;
  idleTimeoutMs?: number;
  hiddenIdleTimeoutMs?: number;
  graceMs?: number;
  /** 'keep' | 'exit-when-idle' | 'exit-always' */
  residency?: string;
  wake?: WakeInit;
  /** `idle` / `on-demand` 下连续多少次"Host 不在"后转休眠；默认 3，0 = 一直重连。 */
  hostAbsentRetries?: number;
  /** 回退到 4e 之前的定时器行为。默认 false。 */
  legacyTimers?: boolean;
  /** 调用 / 资源读取后的合并窗口（毫秒）。默认 2000。 */
  mergeWindowMs?: number;
  /** `idle` / `on-demand` 下进入后台且空闲时立即休眠。默认 false。 */
  sleepOnBackground?: boolean;
}

export interface ClientConfig {
  appId: string;
  appName: string;
  instanceId?: string;
  /** 'native'（默认）| 'hybrid' | 'web' */
  clientKind?: string;
  hostUrl?: string;
  appVersion?: string;
  instanceTitle?: string;
  token?: string;
  launchToken?: string;
  maxConcurrentCalls?: number;
  overview?: OverviewInit;
  lifecycle?: LifecycleInit;
  connectTimeoutMs?: number;
  /** 'auto'（默认）| 'always' | 'off' */
  heartbeat?: string;
  /** 调用去重（spec/protocol.md 3.3）；缺省保留 5 分钟、最多 64 条。 */
  callDedup?: CallDedupInit;
}

/** 调用去重策略：未给出的字段取默认值；任一为 0 关闭。 */
export interface CallDedupInit {
  ttlMs?: number;
  maxEntries?: number;
}

export interface JsStateInfo {
  status: string;
  retryInMs?: number;
  reason?: string;
  code?: string;
}

/** `type`：'state'（带 state）、'paired'（带 token）、'log'（带 level、message）、'idle-exit'。 */
export interface ClientEvent {
  type: string;
  state?: JsStateInfo;
  token?: string;
  level?: string;
  message?: string;
}

export interface ToolSpecInit {
  name: string;
  description: string;
  inputSchemaJson?: string;
  risk?: string;
  activation?: string;
  title?: string;
  enabled?: boolean;
  /** 标准 MCP 工具注解。 */
  annotations?: ToolAnnotationsInit;
  /** 结果的 JSON Schema 文本（MCP `outputSchema`）。 */
  outputSchemaJson?: string;
  /** 'app'（缺省）| 'view'：对界面的依赖（spec/protocol.md 3.4）。 */
  surface?: string;
  /** 所在页面名；Hub 在该工具未注册时据此导航。 */
  page?: string;
  /** 后台替代（spec/protocol.md 3.4）：同一 App 中一个 `app` 工具的局部名；本工具因 App 在后台不可调用时 Hub 改调它。 */
  backgroundTool?: string;
}

export interface ToolAnnotationsInit {
  title?: string;
  readOnlyHint?: boolean;
  destructiveHint?: boolean;
  idempotentHint?: boolean;
  openWorldHint?: boolean;
}

export interface ContentAnnotationsInit {
  /** 'user' | 'assistant' */
  audience?: string[];
  priority?: number;
  lastModified?: string;
}

/** `Call.completeWith` 的参数；缺省 = 无返回值、`done`。 */
export interface CallResultInit {
  dataJson?: string;
  stateHints?: string[];
  /** 'done'（缺省）| 'pending' | 'partial' | 'noop' */
  status?: string;
  stateResource?: string;
  summary?: string;
  annotations?: ContentAnnotationsInit;
}

export interface ResourceSpecInit {
  name: string;
  description: string;
  mimeType?: string;
  /** 需实时推送：被订阅时保持连接、休眠中变化时回连推送。默认 false。 */
  realtime?: boolean;
  /** 资源内容的标注（MCP 内容注解）；`audience` 取值不合法时抛出 `INVALID_ARG`。 */
  annotations?: ContentAnnotationsInit;
}

export class Hold {
  release(): void;
}

export class Call {
  readonly callId: string;
  readonly toolName: string;
  readonly argumentsJson: string;
  /** Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 null / undefined。 */
  readonly idempotencyKey?: string | null;
  isCancelled(): boolean;
  setCancelListener(listener: (reason: string) => void): void;
  complete(dataJson?: string | null, stateHints?: string[] | null): void;
  /** 成功完成并附带业务状态、摘要与内容标注；取值不合法时抛出 `INVALID_ARG`（调用仍未完成）。 */
  completeWith(result: CallResultInit): void;
  fail(kind: string, message: string): void;
  failWithDetails(kind: string, message: string, detailsJson?: string | null): void;
  hold(): Hold;
  /** 报告进度（spec/protocol.md 3.3）；调用已结束时抛出 `ALREADY_COMPLETED`。 */
  reportProgress(progress: number, total?: number | null, message?: string | null): void;
}

export class Read {
  readonly resourceName: string;
  complete(contentsJson: string): void;
  fail(kind: string, message: string): void;
  /** 失败完成并附带详情（JSON 文本）；语义同 `Call.failWithDetails`。 */
  failWithDetails(kind: string, message: string, detailsJson?: string | null): void;
}

/** 一次导航请求（Host 的 `app/navigate`，spec/protocol.md 3.4）。完成只能一次，重复完成抛出 `ALREADY_COMPLETED`。 */
export class Navigate {
  readonly page: string;
  /** 页面参数 JSON 文本；Host 没有给出时为 undefined。 */
  readonly paramsJson?: string;
  complete(): void;
  /** 导航失败（`NAVIGATION_FAILED`）。 */
  fail(message: string): void;
  /** 拒绝导航（`NAVIGATION_DENIED`）。 */
  deny(message: string): void;
  /** 需要用户操作（`USER_ACTION_REQUIRED`，如 App 在后台、已发通知请用户点开）；`reason` / `uri` 缺省不出现。 */
  failUserAction(message: string, reason?: string | null, uri?: string | null): void;
}

export class Tool {
  readonly name: string;
  /** 整体替换定义；已声明的 `annotations` / `outputSchemaJson` 保持不变。 */
  update(spec: ToolSpecInit): void;
  /** 整体替换定义与选项：`annotations` / `outputSchemaJson` 缺省表示清除。 */
  updateWith(spec: ToolSpecInit): void;
  setEnabled(enabled: boolean): void;
  dispose(): void;
}

export class Resource {
  readonly name: string;
  notifyChanged(): void;
  dispose(): void;
}

export class Scope {
  registerTool(spec: ToolSpecInit, handler: (call: Call) => void): Tool;
  registerResource(spec: ResourceSpecInit, reader: (read: Read) => void): Resource;
  createScope(name: string): Scope;
  dispose(): void;
}

export class NativeClient {
  constructor(config: ClientConfig, listener?: (event: ClientEvent) => void);
  readonly instanceId: string;
  readonly state: JsStateInfo;
  readonly connectionId?: string;
  readonly token?: string;
  start(): void;
  stop(): void;
  handleWake(args: string): boolean;
  wake(reason?: string | null): boolean;
  connectNow(): boolean;
  sleep(reason?: string | null): boolean;
  hold(): Hold;
  toolsHash(): string;
  runtimeActive(): boolean;
  setVisibility(visibility: string, focused: boolean): void;
  /** 设置导航回调（spec/protocol.md 3.4）；null 清除。握手时声明能力，应在 `start()` 之前设置。 */
  setNavigationHandler(handler: ((navigate: Navigate) => void) | null): void;
  /**
   * 不可见时导航请求是否仍交给导航回调（spec/protocol.md 3.4）。缺省按平台：鸿蒙 `false`（直接以 `USER_ACTION_REQUIRED`
   * （`foreground`）回复）。随时生效，只影响之后到达的请求。
   */
  setNavigateInBackground(enabled: boolean): void;
  registerTool(spec: ToolSpecInit, handler: (call: Call) => void): Tool;
  registerResource(spec: ResourceSpecInit, reader: (read: Read) => void): Resource;
  createScope(name: string): Scope;
}
