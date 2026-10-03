/**
 * Electron 渲染进程模式：页面经 preload 暴露的桥接对象把工具 / 资源登记到主进程，
 * 由主进程的 @app-mcp/node 客户端代为连接 Host。此模式下不加载 WASM、不建 WebSocket。
 *
 * 消息格式是 @app-mcp/electron 的 op/event 协议（主进程实现见 packages/electron/src/main.ts），
 * 该协议消息类型的唯一定义在 electron-bridge/protocol.ts，经本文件导出，@app-mcp/electron 从这里重新导出。
 *
 * - 页面 → 主进程：`bridge.request(op)`（preload 内为 `ipcRenderer.invoke('app-mcp:op', op)`），返回 {@link OpReply}。
 * - 主进程 → 页面：`bridge.onMessage(listener)` 收到 {@link MainEvent}。
 *
 * `createAppMcp` 检测到桥接（{@link findElectronBridge}）时自动走这条路径；页面代码无需修改。
 * 身份与连接由主进程负责：`appId` / `appName` / `hostUrl` 等选项在页面中被忽略，
 * `instanceId`、`state` 与 `connectionId` 取自主进程客户端（首次 hello 完成前 `instanceId` 为空字符串）。
 *
 * 实现按职责拆在 `electron-bridge/` 下：protocol.ts（协议消息类型，唯一定义）、client.ts（桥接客户端与辅助函数）、
 * entries.ts（工具 / 资源条目）；本文件为 scope 与实例。
 */

import { attachBridgeNavigation, type BridgeNavigation } from './bridge-navigation'
import { BusyState } from './busy'
import { EventDeclarations } from './events'
import { noopHold } from './noop'
import { checkPageName } from './view'
import type {
  AppMcp,
  AppMcpOptions,
  BusyHandle,
  ConnectionState,
  EventDefinition,
  EventPayload,
  HoldHandle,
  LazyToolDefinition,
  NavigationHandler,
  NavigationOptions,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ScopeOptions,
  ToolDefinition,
  ToolHandle,
} from './types'
import type { AppMcpBridge, EventOp, HelloReply, MainEvent } from './electron-bridge/protocol'
import { type AnyDef, callError, Client, defaultLogger, type Detachable, type Owner, scopeField } from './electron-bridge/client'
import { ResourceEntry, ToolEntry } from './electron-bridge/entries'

export {
  type AppMcpBridge,
  BRIDGE_VERSION,
  DEFAULT_BRIDGE_KEY,
  type EventMessage,
  type EventOp,
  findElectronBridge,
  type HelloReply,
  type MainEvent,
  type NavigateEvent,
  type NavigationOp,
  type OpReply,
  type Outcome,
  type RendererOp,
  type ToolSpecMessage,
} from './electron-bridge/protocol'

abstract class RegistrarBase implements Owner {
  readonly children = new Set<Detachable>()
  abstract readonly scopeId: number | undefined
  abstract readonly viewOptions: ScopeOptions | undefined
  abstract readonly parentOwner: Owner | undefined
  protected abstract isActive(): boolean

  protected constructor(protected readonly client: Client) {}

  private get target(): Client {
    // 已注销的 scope / 实例：返回无桥接的客户端，注册为空操作。
    return this.isActive() ? this.client : new Client(null, this.client.logger)
  }

  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle {
    return new ToolEntry(this.target, this, name, definition as AnyDef)
  }

  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return new ResourceEntry(this.target, this, name, definition, definition.read)
  }

  scope(name: string, options?: ScopeOptions): Scope {
    if (options?.page !== undefined) checkPageName(`scope ${name}`, options.page)
    const scope = new ScopeEntry(this.target, this, name, options)
    if (this.isActive() && this.client.bridge) this.children.add(scope)
    return scope
  }

  protected detachChildren(): void {
    for (const child of this.children) child.detach()
    this.children.clear()
  }
}

class ScopeEntry extends RegistrarBase implements Scope, Detachable {
  readonly scopeId: number
  readonly viewOptions: ScopeOptions | undefined
  private disposed = false

  constructor(
    client: Client,
    private readonly owner: Owner,
    readonly name: string,
    options: ScopeOptions | undefined,
  ) {
    super(client)
    this.viewOptions = options === undefined ? undefined : { ...options }
    this.scopeId = client.id()
    client.enqueue(() => ({ op: 'scope.create', id: this.scopeId, ...scopeField(owner), name }))
  }

  get parentOwner(): Owner {
    return this.owner
  }

  protected isActive(): boolean {
    return !this.disposed
  }

  dispose(): void {
    if (this.disposed) return
    this.detach()
    this.owner.children.delete(this)
    this.client.enqueue(() => ({ op: 'scope.dispose', id: this.scopeId }))
  }

  detach(): void {
    this.disposed = true
    this.detachChildren()
  }
}

class BridgeAppMcp extends RegistrarBase implements AppMcp {
  readonly scopeId = undefined
  readonly viewOptions = undefined
  readonly parentOwner = undefined
  /** 当前的导航接入（`setNavigationHandler`）。 */
  private navigation: BridgeNavigation | undefined
  readonly options: Readonly<AppMcpOptions>
  instanceId = ''
  private currentConnectionId: string | undefined
  private currentState: ConnectionState
  private readonly listeners = new Set<(state: ConnectionState) => void>()
  private readonly unsubscribe: () => void
  private disposed = false
  /** 本页"用户正在操作"的有效值；变化时经桥接发送 `busy.set`。 */
  private readonly busyState = new BusyState((busy) => this.sendBusy(busy))
  /** 本页声明的事件（本地校验用；主进程 / Rust 侧按页面记录）。 */
  private readonly events = new EventDeclarations()

  constructor(options: AppMcpOptions, bridge: AppMcpBridge | null) {
    super(new Client(bridge, options.logger ?? defaultLogger))
    this.options = Object.freeze({ ...options })
    if (!bridge) {
      this.currentState = { status: 'disabled' }
      this.unsubscribe = () => {}
      return
    }
    this.currentState = { status: 'connecting' }
    this.unsubscribe = bridge.onMessage((event) => this.onEvent(event))
    // 通知主进程：新页面开始登记（丢弃同一 webContents 上旧页面的登记）。
    void this.client.enqueue(() => ({ op: 'hello' })).then((reply) => {
      if (!reply?.ok || this.disposed) return
      const hello = reply.value as HelloReply
      this.instanceId = hello.instanceId
      this.setState(hello.state, hello.connectionId)
    })
  }

  protected isActive(): boolean {
    return !this.disposed
  }

  get state(): ConnectionState {
    return this.currentState
  }

  get connectionId(): string | undefined {
    return this.currentConnectionId
  }

  // 生命周期由主进程的 @app-mcp/node 客户端负责（模式、空闲时间在主进程配置）；
  // 页面侧的 wake / sleep / connectNow / hold 经 IPC 转发，状态（含 dormant / waking）由主进程原样转发。
  wake(): void {
    this.lifecycle('lifecycle.wake')
  }
  sleep(): void {
    this.lifecycle('lifecycle.sleep')
  }
  connectNow(): void {
    this.lifecycle('lifecycle.connectNow')
  }
  hold(): HoldHandle {
    return this.disposed ? noopHold() : this.client.hold()
  }

  /**
   * 导航回调（spec/protocol.md 3.4）：经桥接请主进程 / Rust 侧把 Host 的导航请求转给本页（对方需开启导航转发，
   * 如 `attachAppMcp({ navigation: true })` / `Builder::page_navigation(true)`；未开启时记录警告）。
   */
  setNavigationHandler(handler: NavigationHandler | null, options: NavigationOptions = {}): void {
    this.navigation?.dispose()
    this.navigation = undefined
    const bridge = this.client.bridge
    if (!handler || !bridge || this.disposed) return
    const navigation = attachBridgeNavigation(bridge, handler, options)
    this.navigation = navigation
    navigation.ready.catch((error: unknown) => {
      if (this.navigation === navigation) {
        this.client.logger.warn(`[app-mcp] 主进程未接受本页处理导航：${error instanceof Error ? error.message : String(error)}`)
      }
    })
  }

  /**
   * 用户正在操作（spec/protocol.md 5.3）：经桥接声明本页的状态（`busy.set`），对方按页面汇总（各页之或）后设置客户端；
   * 页面刷新 / 关闭时随登记一起失效。写调用的处理方式（`busyPolicy`）由主进程 / Rust 侧配置。
   */
  setBusy(busy: boolean): void {
    if (this.disposed) return
    this.busyState.set(busy)
  }

  beginBusy(): BusyHandle {
    return this.disposed ? noopHold() : this.busyState.begin()
  }

  isBusy(): boolean {
    return this.busyState.busy
  }

  private sendBusy(busy: boolean): void {
    if (this.disposed || !this.client.bridge) return
    void this.client.enqueue(() => ({ op: 'busy.set', busy })).then((reply) => {
      if (reply && !reply.ok) this.client.logger.warn(`[app-mcp] 主进程未接受用户正在操作的声明：${reply.message}`)
    })
  }

  /** 事件（spec/protocol.md 3.5）：声明经桥接登记到本页（`event.declare`），页面刷新 / 关闭后由对方撤销。 */
  declareEvent(event: EventDefinition): void {
    const info = this.events.declare(event)
    if (this.disposed) return
    this.sendEventOp({ op: 'event.declare', event: info })
  }

  removeEvent(name: string): boolean {
    const removed = this.events.remove(name)
    if (removed && !this.disposed) this.sendEventOp({ op: 'event.remove', name })
    return removed
  }

  /**
   * 本地校验后按镜像的连接状态决定：不是 `connected` 时丢弃并返回 false；否则经桥接发出并返回 true
   * （与对方断线竞争时事件在对方丢弃，见 {@link AppMcp.emitEvent}）。
   */
  emitEvent(name: string, payload?: EventPayload): boolean {
    const payloadJson = this.events.prepareEmit(name, payload)
    if (this.disposed || !this.client.bridge || this.currentState.status !== 'connected') return false
    this.sendEventOp({
      op: 'event.emit',
      name,
      ...(payloadJson !== undefined && { payload: JSON.parse(payloadJson) as Record<string, unknown> }),
    })
    return true
  }

  private sendEventOp(op: EventOp): void {
    if (!this.client.bridge) return
    void this.client.enqueue(() => op).then((reply) => {
      if (reply && !reply.ok) this.client.logger.warn(`[app-mcp] 主进程未接受事件操作 ${op.op}：${reply.message}`)
    })
  }

  private lifecycle(op: 'lifecycle.wake' | 'lifecycle.sleep' | 'lifecycle.connectNow'): void {
    if (this.disposed) return
    void this.client.enqueue(() => ({ op }))
  }

  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  /** @invariant 连接 ID 与状态一起更新：先更新再通知监听器，监听器内读到的是新状态对应的连接 ID。 */
  private setState(state: ConnectionState, connectionId?: unknown): void {
    this.currentState = state
    this.currentConnectionId = typeof connectionId === 'string' && connectionId !== '' ? connectionId : undefined
    for (const listener of [...this.listeners]) {
      try {
        listener(state)
      } catch (error) {
        this.client.logger.error('[app-mcp] onStateChange 监听器抛出异常', error)
      }
    }
  }

  private onEvent(event: MainEvent): void {
    if (this.disposed) return
    switch (event.type) {
      case 'call':
        this.client.onCall(
          event.callId,
          event.toolId,
          event.input,
          typeof event.idempotencyKey === 'string' ? event.idempotencyKey : undefined,
        )
        break
      case 'cancel':
        this.client.onCancel(event.callId, event.kind, event.message)
        break
      case 'read':
        this.client.onRead(event.readId, event.resourceId)
        break
      case 'state':
        this.setState(event.state, event.connectionId)
        break
    }
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.navigation?.dispose()
    this.navigation = undefined
    this.detachChildren()
    for (const controller of this.client.calls.values()) controller.abort(callError('CANCELLED', 'SDK 已停止'))
    this.client.calls.clear()
    this.client.dropHolds()
    if (this.client.bridge) {
      void this.client.enqueue(() => ({ op: 'reset' }))
      this.unsubscribe()
      this.setState({ status: 'stopped' })
    }
    this.listeners.clear()
  }
}

/**
 * 经 Electron 桥接创建 AppMcp。`bridge` 为 null 时返回“disabled”实例（注册为空操作）。
 * 一般不需要直接调用：`createAppMcp` 会自动检测桥接。
 */
export function createBridgeAppMcp(options: AppMcpOptions, bridge: AppMcpBridge | null): AppMcp {
  return new BridgeAppMcp(options, bridge)
}