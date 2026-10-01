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
}

export interface ResourceSpecInit {
  name: string;
  description: string;
  mimeType?: string;
  /** 需实时推送：被订阅时保持连接、休眠中变化时回连推送。默认 false。 */
  realtime?: boolean;
}

export class Hold {
  release(): void;
}

export class Call {
  readonly callId: string;
  readonly toolName: string;
  readonly argumentsJson: string;
  isCancelled(): boolean;
  setCancelListener(listener: (reason: string) => void): void;
  complete(dataJson?: string | null, stateHints?: string[] | null): void;
  fail(kind: string, message: string): void;
  failWithDetails(kind: string, message: string, detailsJson?: string | null): void;
  hold(): Hold;
}

export class Read {
  readonly resourceName: string;
  complete(contentsJson: string): void;
  fail(kind: string, message: string): void;
}

export class Tool {
  readonly name: string;
  update(spec: ToolSpecInit): void;
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
  registerTool(spec: ToolSpecInit, handler: (call: Call) => void): Tool;
  registerResource(spec: ResourceSpecInit, reader: (read: Read) => void): Resource;
  createScope(name: string): Scope;
}
