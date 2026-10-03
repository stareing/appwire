/**
 * createAppMcp：在原生客户端（bindings/node）之上实现与 @app-mcp/web 同形的 JS API。
 *
 * 实现按职责拆分：client/shared.ts（共用辅助函数）、client/entries.ts（工具 / 资源条目）；本文件为 scope 与实例。
 */

import { ToolCallError, nativeErrorCode, toFailure } from './errors.js'
import {
  loadNativeBinding,
  type NativeClientConfig,
  type NativeClient,
  type NativeClientEvent,
  type NativeNavigate,
  type NativeRegistrar,
  type NativeScope,
} from './native.js'
import type {
  AppMcp,
  BusyHandle,
  BusyPolicy,
  ConnectionState,
  EventDefinition,
  EventPayload,
  HoldHandle,
  LazyToolDefinition,
  Logger,
  NavigationHandler,
  NodeAppMcpOptions,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ToolDefinition,
  ToolHandle,
  Visibility,
} from './types.js'
import {
  type Child,
  defaultLogger,
  mapState,
  NOOP_HOLD,
  type Owner,
  submitNavigationFailure,
  wrapHold,
} from './client/shared.js'
import { BusyState } from './client/busy.js'
import { encodePayload, toNativeEventSpec } from './client/events.js'
import { ResourceEntry, ToolEntry } from './client/entries.js'

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

abstract class RegistrarBase implements Owner {
  readonly children = new Set<Child>()

  protected constructor(protected readonly logger: Logger) {}

  /** 原生注册入口；为 null 时（未启用 / 已注销）注册为空操作。 */
  protected abstract registrar(): NativeRegistrar | null

  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle {
    return new ToolEntry(
      this.registrar(),
      this,
      name,
      definition as ToolDefinition<any, any> | LazyToolDefinition<any, any>,
      this.logger,
    )
  }

  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return new ResourceEntry(this.registrar(), this, name, definition, this.logger)
  }

  scope(name: string): Scope {
    const registrar = this.registrar()
    const scope = new ScopeEntry(registrar ? registrar.createScope(name) : null, this, name, this.logger)
    if (registrar) this.children.add(scope)
    return scope
  }

  /** 本地标记全部子项已注销（原生侧已由父级递归注销）。 */
  protected detachChildren(): void {
    for (const child of this.children) child.detach()
    this.children.clear()
  }
}

/** {@link BusyPolicy} 的合法值（运行时校验 `setBusyPolicy` 的参数）。 */
const BUSY_POLICIES: readonly BusyPolicy[] = ['reject', 'queue']

class ScopeEntry extends RegistrarBase implements Scope, Child {
  private disposed = false

  constructor(
    private readonly native: NativeScope | null,
    private readonly owner: Owner,
    readonly name: string,
    logger: Logger,
  ) {
    super(logger)
  }

  protected registrar(): NativeRegistrar | null {
    return this.disposed ? null : this.native
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.owner.children.delete(this)
    this.detachChildren()
    this.native?.dispose()
  }

  detach(): void {
    this.disposed = true
    this.detachChildren()
  }
}

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

class NodeAppMcp extends RegistrarBase implements AppMcp {
  readonly options: Readonly<NodeAppMcpOptions>
  private readonly client: NativeClient | null
  private currentState: ConnectionState
  private readonly listeners = new Set<(state: ConnectionState) => void>()
  private readonly idleExitListeners = new Set<() => void>()
  private keepAliveTimer: ReturnType<typeof setInterval> | undefined
  private disposed = false
  /** 用户正在操作的有效值（显式开关 OR 作用域）；变化时转给原生客户端。 */
  private readonly busyState = new BusyState((busy) => this.lifecycleClient()?.setBusy?.(busy))

  constructor(options: NodeAppMcpOptions) {
    super(options.logger ?? defaultLogger)
    this.options = Object.freeze({ ...options })
    if (options.enabled === false) {
      this.client = null
      this.currentState = { status: 'disabled' }
      return
    }
    const binding = options.binding ?? loadNativeBinding()
    const config: NativeClientConfig = {
      appId: options.appId,
      appName: options.appName,
      clientKind: options.clientKind ?? 'native',
      ...(options.instanceId !== undefined && { instanceId: options.instanceId }),
      ...(options.hostUrl !== undefined && { hostUrl: options.hostUrl }),
      ...(options.appVersion !== undefined && { appVersion: options.appVersion }),
      ...(options.instanceTitle !== undefined && { instanceTitle: options.instanceTitle }),
      ...(options.token !== undefined && { token: options.token }),
      ...(options.launchToken !== undefined && { launchToken: options.launchToken }),
      ...(options.maxConcurrentCalls !== undefined && { maxConcurrentCalls: options.maxConcurrentCalls }),
      ...(options.maxQueuedCalls !== undefined && { maxQueuedCalls: options.maxQueuedCalls }),
      ...(options.busyPolicy !== undefined && { busyPolicy: options.busyPolicy }),
      ...(options.overview !== undefined && { overview: options.overview }),
      ...(options.lifecycle !== undefined && { lifecycle: { ...options.lifecycle } }),
      ...(options.connectTimeoutMs !== undefined && { connectTimeoutMs: options.connectTimeoutMs }),
      ...(options.heartbeat !== undefined && { heartbeat: options.heartbeat }),
      ...(options.callDedup !== undefined && { callDedup: { ...options.callDedup } }),
      ...(options.registerName !== undefined && { registerName: options.registerName }),
      ...(options.nameInstance !== undefined && { nameInstance: options.nameInstance }),
    }
    this.client = new binding.NativeClient(config, (event) => this.onEvent(event))
    this.currentState = mapState(this.client.state)
    if (options.onNavigate) this.setNavigationHandler(options.onNavigate)
    if (options.navigateInBackground !== undefined) this.setNavigateInBackground(options.navigateInBackground)
    if (options.autoStart !== false) this.start()
  }

  protected registrar(): NativeRegistrar | null {
    return this.disposed ? null : this.client
  }

  get instanceId(): string {
    return this.client?.instanceId ?? ''
  }

  get state(): ConnectionState {
    return this.currentState
  }

  get connectionId(): string | undefined {
    return this.client?.connectionId ?? undefined
  }

  get token(): string | null {
    if (!this.client || this.disposed) return null
    return this.client.token ?? null
  }

  start(): void {
    if (!this.client || this.disposed) return
    this.client.start()
    this.ensureKeepAlive()
  }

  setVisibility(visibility: Visibility, focused = true): void {
    if (!this.client || this.disposed) return
    this.client.setVisibility(visibility, focused)
  }

  setNavigationHandler(handler: NavigationHandler | null): void {
    const client = this.lifecycleClient()
    if (!client) return
    if (!client.setNavigationHandler) {
      if (handler) this.logger.warn('[app-mcp] 原生模块版本过旧，不支持导航回调（spec/protocol.md 3.4）')
      return
    }
    client.setNavigationHandler(handler ? (navigate) => this.onNavigate(handler, navigate) : null)
  }

  setNavigateInBackground(enabled: boolean): void {
    const client = this.lifecycleClient()
    if (!client) return
    if (!client.setNavigateInBackground) {
      this.logger.warn('[app-mcp] 原生模块版本过旧，不支持 navigateInBackground（spec/protocol.md 3.4）')
      return
    }
    client.setNavigateInBackground(enabled)
  }

  setBusy(busy: boolean): void {
    if (this.busyClient()) this.busyState.set(busy)
  }

  beginBusy(): BusyHandle {
    return this.busyClient() ? this.busyState.begin() : NOOP_HOLD
  }

  isBusy(): boolean {
    return this.client?.setBusy !== undefined && this.busyState.busy
  }

  setBusyPolicy(policy: BusyPolicy): void {
    if (!BUSY_POLICIES.includes(policy)) throw new Error(`无效的 busyPolicy：${JSON.stringify(policy)}`)
    this.busyClient()?.setBusyPolicy(policy)
  }

  /** 支持用户正在操作的原生客户端；已停止时为 null，旧版原生模块记一条警告后为 null。 */
  private busyClient(): Required<Pick<NativeClient, 'setBusy' | 'setBusyPolicy'>> | null {
    const client = this.lifecycleClient()
    if (!client) return null
    if (!client.setBusy || !client.setBusyPolicy) {
      this.logger.warn('[app-mcp] 原生模块版本过旧，不支持 setBusy / busyPolicy（spec/protocol.md 5.3）')
      return null
    }
    return client as Required<Pick<NativeClient, 'setBusy' | 'setBusyPolicy'>>
  }

  declareEvent(event: EventDefinition): void {
    const spec = toNativeEventSpec(event)
    this.eventClient()?.declareEvent(spec)
  }

  removeEvent(name: string): boolean {
    return this.eventClient()?.removeEvent(name) ?? false
  }

  emitEvent(name: string, payload?: EventPayload): boolean {
    const payloadJson = encodePayload(name, payload)
    return this.eventClient()?.emitEvent(name, payloadJson) ?? false
  }

  /** 支持事件的原生客户端；已停止时为 null，旧版原生模块记一条警告后为 null。 */
  private eventClient(): Required<Pick<NativeClient, 'declareEvent' | 'removeEvent' | 'emitEvent'>> | null {
    const client = this.lifecycleClient()
    if (!client) return null
    if (!client.declareEvent || !client.removeEvent || !client.emitEvent) {
      this.logger.warn('[app-mcp] 原生模块版本过旧，不支持事件（spec/protocol.md 3.5）')
      return null
    }
    return client as Required<Pick<NativeClient, 'declareEvent' | 'removeEvent' | 'emitEvent'>>
  }

  /**
   * 执行导航回调并提交结果：正常返回 → 完成；`NAVIGATION_DENIED` 类别的 ToolCallError → 拒绝；
   * `USER_ACTION_REQUIRED`（{@link ToolCallError.userActionRequired}）→ 需要用户操作；其他异常 → 失败。
   * @error 参数 JSON 无法解析时以失败完成（不调用回调）。
   */
  private onNavigate(handler: NavigationHandler, navigate: NativeNavigate): void {
    const submit = (action: () => void) => {
      try {
        action()
      } catch (error) {
        if (nativeErrorCode(error) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交导航（页面 ${navigate.page}）的结果失败`, error)
        }
      }
    }
    let params: Record<string, unknown> | undefined
    try {
      params = navigate.paramsJson != null ? (JSON.parse(navigate.paramsJson) as Record<string, unknown>) : undefined
    } catch {
      submit(() => navigate.fail('页面参数不是合法的 JSON'))
      return
    }
    Promise.resolve()
      .then(() => handler({ page: navigate.page, params }))
      .then(
        () => submit(() => navigate.complete()),
        (error: unknown) => {
          submit(() => submitNavigationFailure(navigate, toFailure(error)))
        },
      )
  }

  // ---- 生命周期 ----------------------------------------------------------

  handleWake(args: string | readonly string[]): boolean {
    const client = this.lifecycleClient()
    if (!client?.handleWake) return false
    const list = typeof args === 'string' ? [args] : args
    for (const arg of list) {
      if (typeof arg === 'string' && arg && client.handleWake(arg)) return true
    }
    return false
  }

  wake(reason: 'app' | 'visible' = 'app'): boolean {
    const client = this.lifecycleClient()
    return client?.wake ? client.wake(reason) : false
  }

  connectNow(): boolean {
    const client = this.lifecycleClient()
    if (!client) return false
    if (!client.connectNow) {
      this.start()
      return true
    }
    this.ensureKeepAlive()
    return client.connectNow()
  }

  sleep(): boolean {
    const client = this.lifecycleClient()
    return client?.sleep ? client.sleep() : false
  }

  hold(): HoldHandle {
    const client = this.lifecycleClient()
    return client?.hold ? wrapHold(client.hold()) : NOOP_HOLD
  }

  toolsHash(): string {
    const client = this.lifecycleClient()
    return client?.toolsHash ? client.toolsHash() : ''
  }

  onIdleExit(listener: () => void): () => void {
    this.idleExitListeners.add(listener)
    return () => {
      this.idleExitListeners.delete(listener)
    }
  }

  /** 未启用或已注销时为 null。 */
  private lifecycleClient(): NativeClient | null {
    return this.client && !this.disposed ? this.client : null
  }

  private ensureKeepAlive(): void {
    if (this.options.keepAlive !== false && this.keepAliveTimer === undefined) {
      // 原生回调使用 weak ThreadsafeFunction，不会让进程保持运行；这里用一个 ref 的定时器代替。
      this.keepAliveTimer = setInterval(() => {}, 0x7fffffff)
    }
  }

  private emitIdleExit(): void {
    const callbacks = [...(this.options.onIdleExit ? [this.options.onIdleExit] : []), ...this.idleExitListeners]
    for (const cb of callbacks) {
      try {
        cb()
      } catch (error) {
        this.logger.error('[app-mcp] onIdleExit 回调抛出异常', error)
      }
    }
  }

  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    if (this.keepAliveTimer !== undefined) {
      clearInterval(this.keepAliveTimer)
      this.keepAliveTimer = undefined
    }
    this.detachChildren()
    if (this.client) {
      try {
        this.client.stop()
      } catch (error) {
        this.logger.warn('[app-mcp] 停止原生客户端失败', error)
      }
      this.setState({ status: 'stopped' })
    }
    this.listeners.clear()
    this.idleExitListeners.clear()
  }

  private setState(state: ConnectionState): void {
    this.currentState = state
    for (const listener of [...this.listeners]) {
      try {
        listener(state)
      } catch (error) {
        this.logger.error('[app-mcp] onStateChange 监听器抛出异常', error)
      }
    }
  }

  private onEvent(event: NativeClientEvent): void {
    if (this.disposed) return
    switch (event.type) {
      case 'state':
        this.setState(mapState(event.state))
        break
      case 'paired':
        try {
          this.options.onPaired?.(event.token)
        } catch (error) {
          this.logger.error('[app-mcp] onPaired 回调抛出异常', error)
        }
        break
      case 'log':
        if (event.level === 'error') this.logger.error(`[app-mcp] ${event.message}`)
        else if (event.level === 'warn') this.logger.warn(`[app-mcp] ${event.message}`)
        else this.logger.debug(`[app-mcp] ${event.message}`)
        break
      case 'idle-exit':
        this.emitIdleExit()
        break
    }
  }
}

/** 创建 Node 端 app-mcp 客户端。API 与 @app-mcp/web 的 `createAppMcp` 同形。 */
export function createAppMcp(options: NodeAppMcpOptions): AppMcp {
  return new NodeAppMcp(options)
}