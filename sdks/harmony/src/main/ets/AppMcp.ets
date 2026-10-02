/**
 * AppMcp：在原生客户端（libapp_mcp_harmony.so，bindings/harmony）之上实现与 @app-mcp/node 同义的 ArkTS API。
 *
 * 原生模块通过 {@link NativeClientFactory} 注入：Index.ets / Harmony.ets 传入真实模块，测试传入假实现，
 * 因此本文件不加载 `.so`、不依赖 Kit。
 *
 * @invariant 所有原生回调都在创建客户端的 ArkTS 线程上执行（Node-API ThreadsafeFunction）；
 *   在 UIAbility / AbilityStage 中创建即为主线程，handler 可直接操作 UI 状态，耗时工作请交给 taskpool。
 */

import type {
  Call, CallResultInit, ClientConfig, ClientEvent, ContentAnnotationsInit, Hold, JsStateInfo, LifecycleInit,
  NativeClient, Navigate, Read, Resource, ResourceSpecInit, Scope as NativeScope, Tool, ToolAnnotationsInit, ToolSpecInit,
} from 'libapp_mcp_harmony.so';
import { Failure, ToolCallError, nativeErrorCode, toFailure } from './Errors';
import {
  Activation, AppMcpOptions, CancelReason, ConnectionState, ConnectionStatus, ContentAnnotations, HoldHandle,
  LifecycleOptions, Logger, NavigationHandler, NavigationRequest, Registrar, ResourceDefinition, ResourceHandle, Risk, Scope, ToolAnnotations,
  ToolChanges, ToolContext, ToolDefinition, ToolHandle, ToolResult, ToolSurface, Visibility, WakeReason,
} from './Types';

/** 原生客户端工厂：`(config, listener) => new native.NativeClient(config, listener)`。 */
export type NativeClientFactory = (config: ClientConfig, listener: (event: ClientEvent) => void) => NativeClient;

/** 原生注册入口（客户端或 scope）。 */
interface NativeRegistrar {
  registerTool(spec: ToolSpecInit, handler: (call: Call) => void): Tool;
  registerResource(spec: ResourceSpecInit, reader: (read: Read) => void): Resource;
  createScope(name: string): NativeScope;
}

/** 父级（scope / 客户端）在原生侧批量注销后，子项只更新本地状态。 */
interface Child {
  detach(): void;
}

class ConsoleLogger implements Logger {
  debug(message: string): void {
  }

  warn(message: string): void {
    console.warn(message);
  }

  error(message: string): void {
    console.error(message);
  }
}

function errorText(error: Object | null | undefined): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

const KNOWN_STATUSES: string[] = [
  'idle', 'connecting', 'handshaking', 'pending-pairing', 'connected', 'backoff', 'rejected', 'stopped', 'dormant',
  'waking', 'host-mismatch',
];

/** 原生状态 → {@link ConnectionState}。 */
export function mapState(info: JsStateInfo, now: number): ConnectionState {
  const status: ConnectionStatus = KNOWN_STATUSES.includes(info.status) ? info.status as ConnectionStatus : 'idle';
  const state: ConnectionState = { status: status };
  if (status === 'backoff') {
    state.retryAt = now + (info.retryInMs ?? 0);
    if (info.reason !== undefined && info.reason !== null) {
      state.reason = info.reason;
    }
    if (info.code !== undefined && info.code !== null) {
      state.code = info.code;
    }
  } else if (status === 'rejected') {
    state.reason = info.reason ?? '';
    state.code = info.code ?? 'REJECTED';
  } else if (status === 'host-mismatch') {
    state.reason = info.reason ?? '';
    state.code = info.code ?? 'HOST_NOT_APP_MCP';
  }
  return state;
}

/** `JSON.stringify`，`undefined` 记为 `null`。 */
function stringifyJson(value: Object | null | undefined): string {
  if (value === undefined) {
    return 'null';
  }
  const text: string | undefined = JSON.stringify(value);
  return text === undefined ? 'null' : text;
}

/** 生命周期选项 → 原生字段。 */
function lifecycleInit(options: LifecycleOptions): LifecycleInit {
  const init: LifecycleInit = {};
  init.mode = options.mode;
  init.idleTimeoutMs = options.idleTimeoutMs;
  init.hiddenIdleTimeoutMs = options.hiddenIdleTimeoutMs;
  init.graceMs = options.graceMs;
  init.residency = options.residency;
  if (options.wake !== undefined) {
    init.wake = { kind: options.wake.kind, target: options.wake.target, background: options.wake.background };
  }
  init.hostAbsentRetries = options.hostAbsentRetries;
  init.legacyTimers = options.legacyTimers;
  init.mergeWindowMs = options.mergeWindowMs;
  init.sleepOnBackground = options.sleepOnBackground;
  return init;
}

class NoopHold implements HoldHandle {
  release(): void {
  }
}

/** 包装原生持有：`release` 幂等。 */
class OnceHold implements HoldHandle {
  private readonly inner: Hold;
  private released: boolean = false;

  constructor(inner: Hold) {
    this.inner = inner;
  }

  release(): void {
    if (this.released) {
      return;
    }
    this.released = true;
    this.inner.release();
  }
}

// ---------------------------------------------------------------------------
// 调用上下文
// ---------------------------------------------------------------------------

class CallContext implements ToolContext {
  readonly callId: string;
  readonly idempotencyKey?: string;
  private readonly call: Call;
  private cancelled: boolean = false;
  private cancelReason: CancelReason | undefined = undefined;
  private readonly listeners: ((reason: CancelReason) => void)[] = [];
  private readonly logger: Logger;

  constructor(call: Call, logger: Logger) {
    this.call = call;
    this.callId = call.callId;
    const key = call.idempotencyKey;
    if (key !== undefined && key !== null) {
      this.idempotencyKey = key;
    }
    this.logger = logger;
  }

  isCancelled(): boolean {
    return this.cancelled;
  }

  onCancel(listener: (reason: CancelReason) => void): void {
    const reason = this.cancelReason;
    if (reason !== undefined) {
      Promise.resolve().then(() => this.notify(listener, reason));
      return;
    }
    this.listeners.push(listener);
  }

  hold(): HoldHandle {
    return new OnceHold(this.call.hold());
  }

  progress(progress: number, total?: number, message?: string): void {
    if (this.cancelled) {
      return;
    }
    try {
      this.call.reportProgress(progress, total ?? null, message ?? null);
    } catch (error) {
      // 调用刚结束时原生层报 ALREADY_COMPLETED：进度只是提示，不影响结果。
      this.logger.debug(`[app-mcp] 进度未送达：${String(error)}`);
    }
  }

  /** 原生侧通知取消。 */
  cancel(reason: CancelReason): void {
    if (this.cancelled) {
      return;
    }
    this.cancelled = true;
    this.cancelReason = reason;
    const listeners = this.listeners.splice(0, this.listeners.length);
    for (const listener of listeners) {
      this.notify(listener, reason);
    }
  }

  private notify(listener: (reason: CancelReason) => void, reason: CancelReason): void {
    try {
      listener(reason);
    } catch (error) {
      this.logger.error(`[app-mcp] onCancel 监听器抛出异常：${errorText(error as Object)}`);
    }
  }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

function toolAnnotationsInit(a: ToolAnnotations): ToolAnnotationsInit {
  const init: ToolAnnotationsInit = {};
  init.title = a.title;
  init.readOnlyHint = a.readOnlyHint;
  init.destructiveHint = a.destructiveHint;
  init.idempotentHint = a.idempotentHint;
  init.openWorldHint = a.openWorldHint;
  return init;
}

/**
 * 以失败完成调用或读取（原生 `Call` / `Read`）：带详情（如 `USER_ACTION_REQUIRED` 的 `reason` / `uri`）时用 `failWithDetails`。
 * @why 以函数传入而非接口：ArkTS 不支持结构化类型，`Call` / `Read` 不能直接当作同一接口。
 */
function submitFailure(failure: Failure, fail: (kind: string, message: string) => void,
  failWithDetails: (kind: string, message: string, detailsJson: string) => void): void {
  const detailsJson = failure.detailsJson;
  if (detailsJson !== undefined) {
    failWithDetails(failure.kind, failure.message, detailsJson);
  } else {
    fail(failure.kind, failure.message);
  }
}

/**
 * 导航回调的失败 → 原生导航句柄：`NAVIGATION_DENIED` 拒绝；`USER_ACTION_REQUIRED`（`ToolCallError.userActionRequired`）
 * 带详情中的 `reason` / `uri`；其他按失败。
 */
function submitNavigationFailure(navigate: Navigate, error: Object | null | undefined): void {
  const failure: Failure = toFailure(error);
  if (failure.kind === 'NAVIGATION_DENIED') {
    navigate.deny(failure.message);
    return;
  }
  if (failure.kind === 'USER_ACTION_REQUIRED' && error instanceof ToolCallError) {
    navigate.failUserAction(failure.message, detailText(error, 'reason'), detailText(error, 'uri'));
    return;
  }
  navigate.fail(failure.message);
}

/** 错误详情中的字符串字段；没有或不是字符串时为 undefined。 */
function detailText(error: ToolCallError, key: string): string | undefined {
  const details = error.details;
  if (details === undefined) {
    return undefined;
  }
  const value: Object | undefined = details[key];
  return typeof value === 'string' ? value as string : undefined;
}

function contentAnnotationsInit(a: ContentAnnotations): ContentAnnotationsInit {
  const init: ContentAnnotationsInit = {};
  if (a.audience !== undefined) {
    init.audience = a.audience.map((role: string): string => role);
  }
  init.priority = a.priority;
  init.lastModified = a.lastModified;
  return init;
}

/** 是否带有 data / stateHints 以外的字段（需要 `completeWith`）。 */
function hasResultExtras<O>(r: ToolResult<O>): boolean {
  return r.status !== undefined || r.stateResource !== undefined || r.summary !== undefined ||
    r.annotations !== undefined;
}

function callResultInit<O>(json: string, r: ToolResult<O>): CallResultInit {
  const init: CallResultInit = { dataJson: json, stateHints: r.stateHints };
  init.status = r.status;
  init.stateResource = r.stateResource;
  init.summary = r.summary;
  if (r.annotations !== undefined) {
    init.annotations = contentAnnotationsInit(r.annotations);
  }
  return init;
}

/**
 * `ToolHandle.update` 单个字段的合并规则。
 * @output `undefined`（未提供）→ 当前值；`null` → `undefined`（清除，回到缺省）；其他 → 新值。
 */
function patchField<T>(change: T | null | undefined, current: T | undefined): T | undefined {
  if (change === undefined) {
    return current;
  }
  return change === null ? undefined : change;
}

class ToolEntry<I, O> implements ToolHandle, Child {
  readonly name: string;
  private def: ToolDefinition<I, O>;
  private readonly native: Tool | null;
  private readonly owner: Set<Child> | null;
  private readonly logger: Logger;
  private disposed: boolean = false;

  constructor(registrar: NativeRegistrar | null, owner: Set<Child> | null, name: string,
    definition: ToolDefinition<I, O>, logger: Logger) {
    this.name = name;
    this.def = definition;
    this.owner = owner;
    this.logger = logger;
    if (registrar === null) {
      this.native = null;
      return;
    }
    // 同步注册：重名、非法名称、非法 schema 直接抛给调用方。
    this.native = registrar.registerTool(this.spec(), (call: Call): void => this.onCall(call));
    if (owner !== null) {
      owner.add(this);
    }
  }

  private spec(): ToolSpecInit {
    const d = this.def;
    const spec: ToolSpecInit = { name: this.name, description: d.description, enabled: d.enabled ?? true };
    spec.inputSchemaJson = d.inputSchema;
    spec.risk = d.risk;
    spec.activation = d.activation;
    spec.title = d.title;
    if (d.annotations !== undefined) {
      spec.annotations = toolAnnotationsInit(d.annotations);
    }
    spec.outputSchemaJson = d.outputSchema;
    spec.surface = d.surface;
    spec.page = d.page;
    spec.backgroundTool = d.backgroundTool;
    return spec;
  }

  update(changes: ToolChanges): void {
    if (this.disposed) {
      return;
    }
    const d = this.def;
    const next: ToolDefinition<I, O> = {
      description: changes.description ?? d.description,
      title: patchField<string>(changes.title, d.title),
      inputSchema: patchField<string>(changes.inputSchema, d.inputSchema),
      risk: patchField<Risk>(changes.risk, d.risk),
      annotations: patchField<ToolAnnotations>(changes.annotations, d.annotations),
      outputSchema: patchField<string>(changes.outputSchema, d.outputSchema),
      activation: patchField<Activation>(changes.activation, d.activation),
      enabled: patchField<boolean>(changes.enabled, d.enabled),
      surface: patchField<ToolSurface>(changes.surface, d.surface),
      page: patchField<string>(changes.page, d.page),
      backgroundTool: patchField<string>(changes.backgroundTool, d.backgroundTool),
      handler: d.handler,
    };
    this.def = next;
    if (this.native !== null) {
      // 定义始终完整：用 updateWith 一并替换注解与输出 schema
      this.native.updateWith(this.spec());
    }
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.owner !== null) {
      this.owner.delete(this);
    }
    if (this.native !== null) {
      this.native.dispose();
    }
  }

  detach(): void {
    this.disposed = true;
  }

  private onCall(call: Call): void {
    const context = new CallContext(call, this.logger);
    let settled = false;
    const finish = (action: () => void): void => {
      if (settled) {
        return;
      }
      settled = true;
      try {
        action();
      } catch (error) {
        // 已取消 / 已完成：原生侧拒绝重复完成，属正常情况。
        if (nativeErrorCode(error as Object) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交工具 ${this.name} 的结果失败：${errorText(error as Object)}`);
        }
      }
    };
    const fail = (failure: Failure): void => {
      finish(() => submitFailure(failure, (k: string, m: string): void => call.fail(k, m),
        (k: string, m: string, d: string): void => call.failWithDetails(k, m, d)));
    };

    try {
      call.setCancelListener((reason: string): void => {
        settled = true;
        context.cancel(reason as CancelReason);
      });
    } catch (error) {
      this.logger.warn(`[app-mcp] 设置取消监听失败：${errorText(error as Object)}`);
    }

    let input: I;
    try {
      const text = call.argumentsJson;
      input = (text !== '' ? JSON.parse(text) : JSON.parse('{}')) as I;
    } catch (error) {
      fail(toFailure(new ToolCallError('INVALID_INPUT', '参数不是合法的 JSON')));
      return;
    }

    const handler = this.def.handler;
    let pending: Promise<O | ToolResult<O>>;
    try {
      pending = Promise.resolve(handler(input, context));
    } catch (error) {
      fail(toFailure(error as Object));
      return;
    }
    pending.then((result: O | ToolResult<O>): void => {
      if (context.isCancelled()) {
        return;
      }
      const structured: ToolResult<O> | null = result instanceof ToolResult ? result : null;
      const data: O = structured !== null ? structured.data : result as O;
      const hints: string[] = structured !== null ? structured.stateHints : [];
      let json: string;
      try {
        json = stringifyJson(data as Object);
      } catch (error) {
        fail(toFailure(new ToolCallError('HANDLER_ERROR', `返回值无法序列化为 JSON：${errorText(error as Object)}`)));
        return;
      }
      if (structured === null || !hasResultExtras(structured)) {
        finish(() => call.complete(json, hints));
        return;
      }
      try {
        call.completeWith(callResultInit(json, structured));
        finish(() => {});
      } catch (error) {
        if (nativeErrorCode(error as Object) === 'ALREADY_COMPLETED') {
          finish(() => {});
          return;
        }
        // 取值不合法（如未知的 status / audience）：失败完成，不让调用挂起到超时
        fail(toFailure(new ToolCallError('HANDLER_ERROR', `返回的结构化结果无效：${errorText(error as Object)}`)));
      }
    }, (error: Object): void => {
      if (context.isCancelled()) {
        return;
      }
      fail(toFailure(error));
    });
  }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

class ResourceEntry<T> implements ResourceHandle, Child {
  readonly name: string;
  private readonly def: ResourceDefinition<T>;
  private readonly native: Resource | null;
  private readonly owner: Set<Child> | null;
  private readonly logger: Logger;
  private disposed: boolean = false;

  constructor(registrar: NativeRegistrar | null, owner: Set<Child> | null, name: string,
    definition: ResourceDefinition<T>, logger: Logger) {
    this.name = name;
    this.def = definition;
    this.owner = owner;
    this.logger = logger;
    if (registrar === null) {
      this.native = null;
      return;
    }
    const spec: ResourceSpecInit = {
      name: name,
      description: definition.description,
      // 未声明时不发送（toolsHash 输入与其他 SDK 一致；Host 按 application/json 处理）
      mimeType: definition.mimeType,
      realtime: definition.realtime,
    };
    if (definition.annotations !== undefined) {
      spec.annotations = contentAnnotationsInit(definition.annotations);
    }
    this.native = registrar.registerResource(spec, (read: Read): void => this.onRead(read));
    if (owner !== null) {
      owner.add(this);
    }
  }

  notifyChanged(): void {
    if (this.disposed || this.native === null) {
      return;
    }
    try {
      this.native.notifyChanged();
    } catch (error) {
      this.logger.warn(`[app-mcp] 资源 ${this.name} 变更通知失败：${errorText(error as Object)}`);
    }
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.owner !== null) {
      this.owner.delete(this);
    }
    if (this.native !== null) {
      this.native.dispose();
    }
  }

  detach(): void {
    this.disposed = true;
  }

  private failRead(read: Read, failure: Failure): void {
    submitFailure(failure, (k: string, m: string): void => read.fail(k, m),
      (k: string, m: string, d: string): void => read.failWithDetails(k, m, d));
  }

  private onRead(read: Read): void {
    const submit = (action: () => void): void => {
      try {
        action();
      } catch (error) {
        if (nativeErrorCode(error as Object) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交资源 ${this.name} 的内容失败：${errorText(error as Object)}`);
        }
      }
    };
    let pending: Promise<T>;
    try {
      pending = Promise.resolve(this.def.read());
    } catch (error) {
      const failure = toFailure(error as Object);
      submit(() => this.failRead(read, failure));
      return;
    }
    pending.then((value: T): void => {
      let json: string;
      try {
        json = stringifyJson(value as Object);
      } catch (error) {
        submit(() => read.fail('HANDLER_ERROR', `资源内容无法序列化为 JSON：${errorText(error as Object)}`));
        return;
      }
      submit(() => read.complete(json));
    }, (error: Object): void => {
      const failure = toFailure(error);
      submit(() => this.failRead(read, failure));
    });
  }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

abstract class RegistrarBase implements Registrar {
  protected readonly children: Set<Child> = new Set<Child>();
  protected readonly logger: Logger;

  constructor(logger: Logger) {
    this.logger = logger;
  }

  /** 原生注册入口；为 null 时（未启用 / 已注销）注册为空操作。 */
  protected abstract registrar(): NativeRegistrar | null;

  tool<I, O>(name: string, definition: ToolDefinition<I, O>): ToolHandle {
    return new ToolEntry<I, O>(this.registrar(), this.children, name, definition, this.logger);
  }

  resource<T>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return new ResourceEntry<T>(this.registrar(), this.children, name, definition, this.logger);
  }

  scope(name: string): Scope {
    const registrar = this.registrar();
    const scope = new ScopeEntry(registrar !== null ? registrar.createScope(name) : null, this.children, name,
      this.logger);
    if (registrar !== null) {
      this.children.add(scope);
    }
    return scope;
  }

  /** 本地标记全部子项已注销（原生侧已由父级递归注销）。 */
  protected detachChildren(): void {
    this.children.forEach((child: Child) => child.detach());
    this.children.clear();
  }
}

class ScopeEntry extends RegistrarBase implements Scope, Child {
  readonly name: string;
  private readonly native: NativeScope | null;
  private readonly owner: Set<Child>;
  private disposed: boolean = false;

  constructor(native: NativeScope | null, owner: Set<Child>, name: string, logger: Logger) {
    super(logger);
    this.native = native;
    this.owner = owner;
    this.name = name;
  }

  protected registrar(): NativeRegistrar | null {
    return this.disposed ? null : this.native;
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.owner.delete(this);
    this.detachChildren();
    if (this.native !== null) {
      this.native.dispose();
    }
  }

  detach(): void {
    this.disposed = true;
    this.detachChildren();
  }
}

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

export class AppMcp extends RegistrarBase {
  readonly options: AppMcpOptions;
  private readonly client: NativeClient | null;
  private currentState: ConnectionState;
  private readonly stateListeners: Set<(state: ConnectionState) => void> = new Set<(state: ConnectionState) => void>();
  private readonly idleExitListeners: Set<() => void> = new Set<() => void>();
  private disposed: boolean = false;

  /**
   * @param factory 原生客户端工厂；`options.enabled === false` 时不调用。
   * @error 配置非法（`INVALID_ARG` / `INVALID_CONFIG`）时抛出原生错误。
   */
  constructor(options: AppMcpOptions, factory: NativeClientFactory) {
    super(options.logger ?? new ConsoleLogger());
    this.options = options;
    if (options.enabled === false) {
      this.client = null;
      this.currentState = { status: 'disabled' };
      return;
    }
    const config: ClientConfig = {
      appId: options.appId,
      appName: options.appName,
      clientKind: 'native',
    };
    // @why 未指定时不传：原生层按 spec/protocol.md 1.3 解析默认端点（鸿蒙沙箱为 ws://127.0.0.1:7717/app）。
    config.hostUrl = options.hostUrl;
    config.instanceId = options.instanceId;
    config.appVersion = options.appVersion;
    config.instanceTitle = options.instanceTitle;
    config.token = options.token;
    config.launchToken = options.launchToken;
    config.maxConcurrentCalls = options.maxConcurrentCalls;
    config.connectTimeoutMs = options.connectTimeoutMs;
    config.heartbeat = options.heartbeat;
    if (options.callDedup !== undefined) {
      config.callDedup = { ttlMs: options.callDedup.ttlMs, maxEntries: options.callDedup.maxEntries };
    }
    if (options.overview !== undefined) {
      config.overview = { summary: options.overview.summary, body: options.overview.body, locale: options.overview.locale };
    }
    if (options.lifecycle !== undefined) {
      config.lifecycle = lifecycleInit(options.lifecycle);
    }
    this.client = factory(config, (event: ClientEvent): void => this.onEvent(event));
    this.currentState = mapState(this.client.state, Date.now());
    if (options.onNavigate !== undefined) {
      this.setNavigationHandler(options.onNavigate);
    }
    if (options.navigateInBackground !== undefined) {
      this.setNavigateInBackground(options.navigateInBackground);
    }
    if (options.autoStart !== false) {
      this.start();
    }
  }

  protected registrar(): NativeRegistrar | null {
    return this.disposed ? null : this.client;
  }

  get instanceId(): string {
    return this.client !== null ? this.client.instanceId : '';
  }

  get state(): ConnectionState {
    return this.currentState;
  }

  /** Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 `cid` 对应；未连接时为 undefined。 */
  get connectionId(): string | undefined {
    const client = this.live();
    return client !== null ? (client.connectionId ?? undefined) : undefined;
  }

  /** 当前 token（配置带入的或配对后获得的）；未启用时为 null。 */
  get token(): string | null {
    const client = this.live();
    return client !== null ? (client.token ?? null) : null;
  }

  get isDisposed(): boolean {
    return this.disposed;
  }

  /** 开始连接（`autoStart: false` 时使用；重复调用无效果）。 */
  start(): void {
    const client = this.live();
    if (client !== null) {
      client.start();
    }
  }

  /** 报告可见性（前后台）。`HarmonyAppMcp.create` 已自动跟随应用前后台。 */
  setVisibility(visibility: Visibility, focused: boolean = true): void {
    const client = this.live();
    if (client !== null) {
      client.setVisibility(visibility, focused);
    }
  }

  /**
   * 设置 / 替换导航回调（见 `AppMcpOptions.onNavigate`）；null 清除。能力在握手时声明：连接后才从无到有设置（或清除）
   * 的回调在下次连接时生效。
   */
  setNavigationHandler(handler: NavigationHandler | null): void {
    const client = this.live();
    if (client === null) {
      return;
    }
    if (handler === null) {
      client.setNavigationHandler(null);
      return;
    }
    client.setNavigationHandler((navigate: Navigate): void => this.onNavigate(handler, navigate));
  }

  /** 修改 `AppMcpOptions.navigateInBackground`；只影响之后到达的导航请求。 */
  setNavigateInBackground(enabled: boolean): void {
    const client = this.live();
    if (client !== null) {
      client.setNavigateInBackground(enabled);
    }
  }

  /**
   * 执行导航回调并提交结果：正常返回 → 完成；`NAVIGATION_DENIED` → 拒绝；`USER_ACTION_REQUIRED` → 需要用户操作
   * （带 `reason` / `uri`）；其他异常 → 失败。
   */
  private onNavigate(handler: NavigationHandler, navigate: Navigate): void {
    const page = navigate.page;
    const submit = (action: () => void): void => {
      try {
        action();
      } catch (error) {
        if (nativeErrorCode(error as Object) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交导航（页面 ${page}）的结果失败：${errorText(error as Object)}`);
        }
      }
    };
    let params: Record<string, Object> | undefined = undefined;
    const text = navigate.paramsJson;
    if (text !== undefined && text !== null) {
      try {
        params = JSON.parse(text) as Record<string, Object>;
      } catch (error) {
        submit(() => navigate.fail('页面参数不是合法的 JSON'));
        return;
      }
    }
    const onError = (error: Object | null | undefined): void => {
      submit(() => submitNavigationFailure(navigate, error));
    };
    let pending: void | Promise<void>;
    try {
      const request: NavigationRequest = { page: page, params: params };
      pending = handler(request);
    } catch (error) {
      onError(error as Object);
      return;
    }
    Promise.resolve(pending).then(
      (): void => submit(() => navigate.complete()),
      (error: Object): void => onError(error),
    );
  }

  // ---- 生命周期（spec/lifecycle.md 第 8 节）------------------------------

  /**
   * 处理激活参数（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、`#app-mcp-wake=`）。
   * 识别到时回连并返回 true（尚未 `start()` 时记录，`start()` 时连接）。鸿蒙上通常用 `handleWant`。
   */
  handleWake(args: string | string[]): boolean {
    const client = this.live();
    if (client === null) {
      return false;
    }
    const list: string[] = typeof args === 'string' ? [args] : args;
    for (const arg of list) {
      if (arg !== '' && client.handleWake(arg)) {
        return true;
      }
    }
    return false;
  }

  /** App 主动回连。`reason` 缺省 `app`（回到前台用 `visible`）。返回是否因此发起了回连。 */
  wake(reason: WakeReason = 'app'): boolean {
    const client = this.live();
    return client !== null ? client.wake(reason) : false;
  }

  /** `on-demand` 模式下主动连接；尚未 `start()` 时等同于 `start()`。 */
  connectNow(): boolean {
    const client = this.live();
    return client !== null ? client.connectNow() : false;
  }

  /** 主动请求休眠（原因 `app`）。返回是否有效果。 */
  sleep(): boolean {
    const client = this.live();
    return client !== null ? client.sleep() : false;
  }

  /** 临时阻止自动休眠，直到返回的句柄被释放。 */
  hold(): HoldHandle {
    const client = this.live();
    return client !== null ? new OnceHold(client.hold()) : new NoopHold();
  }

  /** 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）；未启用时为空字符串。 */
  toolsHash(): string {
    const client = this.live();
    return client !== null ? client.toolsHash() : '';
  }

  /** 订阅 idle-exit，返回取消订阅函数。 */
  onIdleExit(listener: () => void): () => void {
    this.idleExitListeners.add(listener);
    return () => {
      this.idleExitListeners.delete(listener);
    };
  }

  /** 订阅连接状态变化，返回取消订阅函数。 */
  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.stateListeners.add(listener);
    return () => {
      this.stateListeners.delete(listener);
    };
  }

  /** 断开连接并注销全部工具与资源。 */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.detachChildren();
    if (this.client !== null) {
      try {
        this.client.stop();
      } catch (error) {
        this.logger.warn(`[app-mcp] 停止原生客户端失败：${errorText(error as Object)}`);
      }
      this.setState({ status: 'stopped' });
    }
    this.stateListeners.clear();
    this.idleExitListeners.clear();
  }

  /** 未启用或已注销时为 null。 */
  private live(): NativeClient | null {
    return this.disposed ? null : this.client;
  }

  private setState(state: ConnectionState): void {
    this.currentState = state;
    const listeners = Array.from(this.stateListeners);
    for (const listener of listeners) {
      try {
        listener(state);
      } catch (error) {
        this.logger.error(`[app-mcp] onStateChange 监听器抛出异常：${errorText(error as Object)}`);
      }
    }
  }

  private emitIdleExit(): void {
    const callbacks: (() => void)[] = [];
    const own = this.options.onIdleExit;
    if (own !== undefined) {
      callbacks.push(own);
    }
    this.idleExitListeners.forEach((cb: () => void) => callbacks.push(cb));
    for (const cb of callbacks) {
      try {
        cb();
      } catch (error) {
        this.logger.error(`[app-mcp] onIdleExit 回调抛出异常：${errorText(error as Object)}`);
      }
    }
  }

  private onEvent(event: ClientEvent): void {
    if (this.disposed) {
      return;
    }
    if (event.type === 'state' && event.state !== undefined && event.state !== null) {
      this.setState(mapState(event.state, Date.now()));
    } else if (event.type === 'paired' && event.token !== undefined && event.token !== null) {
      const onPaired = this.options.onPaired;
      if (onPaired !== undefined) {
        try {
          onPaired(event.token);
        } catch (error) {
          this.logger.error(`[app-mcp] onPaired 回调抛出异常：${errorText(error as Object)}`);
        }
      }
    } else if (event.type === 'log') {
      const message = `[app-mcp] ${event.message ?? ''}`;
      if (event.level === 'error') {
        this.logger.error(message);
      } else if (event.level === 'warn') {
        this.logger.warn(message);
      } else {
        this.logger.debug(message);
      }
    } else if (event.type === 'idle-exit') {
      this.emitIdleExit();
    }
  }
}
