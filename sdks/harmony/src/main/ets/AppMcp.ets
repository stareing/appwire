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
  ClientConfig, ClientEvent, JsStateInfo, NativeClient, Navigate,
} from 'libapp_mcp_harmony.so';
import { nativeErrorCode } from './Errors';
import {
  AppMcpOptions, ConnectionState, ConnectionStatus, HoldHandle, NavigationHandler, NavigationRequest, Visibility, WakeReason,
} from './Types';
import {
  ConsoleLogger, NativeRegistrar, NoopHold, OnceHold, errorText, lifecycleInit, submitNavigationFailure,
} from './AppMcpSupport';
import { RegistrarBase } from './AppMcpEntries';

/** 原生客户端工厂：`(config, listener) => new native.NativeClient(config, listener)`。 */
export type NativeClientFactory = (config: ClientConfig, listener: (event: ClientEvent) => void) => NativeClient;

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
    config.maxQueuedCalls = options.maxQueuedCalls;
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
