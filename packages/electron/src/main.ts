/**
 * 主进程接入：把渲染进程经 IPC 登记的工具 / 资源代为注册到主进程的 @app-mcp/node 客户端上，
 * 调用时转发给渲染进程执行并回传结果。
 *
 * ```ts
 * import { app, ipcMain } from 'electron'
 * import { createAppMcp } from '@app-mcp/node'
 * import { attachAppMcp } from '@app-mcp/electron/main'
 *
 * const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城', clientKind: 'hybrid' })
 * attachAppMcp({ appMcp, ipcMain })
 * appMcp.tool('app.quit', { description: '退出应用', handler: () => app.quit() })  // 原生能力
 * ```
 *
 * 每个 webContents 的登记放在独立的 scope 中；页面刷新（`hello`）、卸载（`reset`）、
 * webContents 销毁或渲染进程崩溃时整体注销，进行中的调用以 APP_DISCONNECTED 失败。
 *
 * 页面的生命周期操作（`wake` / `sleep` / `connectNow` / `hold`）转给主进程客户端；
 * 页面的 hold 按 webContents 记录，上述注销时一并释放。
 */

import { ToolCallError, type AppMcp, type Logger, type Registrar, type Scope, type ToolHandle, type ResourceHandle } from '@app-mcp/node'
import type { ConnectionState, ErrorKind, HoldHandle } from '@app-mcp/node'
import {
  CHANNEL_EVENT,
  CHANNEL_OP,
  type HelloReply,
  type MainEvent,
  type OpReply,
  type Outcome,
  type RendererOp,
  type ToolSpecMessage,
} from './protocol.js'

export type { HelloReply, MainEvent, OpReply, RendererOp } from './protocol.js'

/** Electron `WebContents` 的最小接口。 */
export interface WebContentsLike {
  readonly id: number
  send(channel: string, ...args: any[]): void
  isDestroyed?(): boolean
  on(event: 'destroyed' | 'render-process-gone', listener: (...args: any[]) => void): unknown
  removeListener(event: 'destroyed' | 'render-process-gone', listener: (...args: any[]) => void): unknown
}

/** `ipcMain.handle` 回调的事件参数。 */
export interface IpcMainInvokeEventLike {
  readonly sender: WebContentsLike
}

/** Electron `ipcMain` 的最小接口。 */
export interface IpcMainLike {
  handle(channel: string, listener: (event: IpcMainInvokeEventLike, ...args: any[]) => unknown): void
  removeHandler(channel: string): void
}

/**
 * 主进程客户端（@app-mcp/node 的 AppMcp）所需的部分。
 * 生命周期方法可选：缺少时页面的 `wake()` / `sleep()` / `hold()` / `connectNow()` 为空操作。
 */
export type MainAppMcp = Pick<AppMcp, 'scope' | 'instanceId' | 'state' | 'onStateChange'> &
  Partial<Pick<AppMcp, 'wake' | 'sleep' | 'hold' | 'connectNow' | 'connectionId'>>

export interface AttachOptions {
  appMcp: MainAppMcp
  ipcMain: IpcMainLike
  /** 只接受这些 webContents 的登记（或由函数判断）。缺省接受全部。 */
  webContents?: WebContentsLike | WebContentsLike[] | ((webContents: WebContentsLike) => boolean)
  logger?: Pick<Logger, 'warn' | 'error'>
}

export interface AppMcpAttachment {
  /** 当前有登记的 webContents 数量。 */
  readonly sessionCount: number
  /** 移除 IPC 处理器并注销全部渲染进程登记。 */
  dispose(): void
}

interface Pending<T> {
  resolve(value: T): void
  reject(error: unknown): void
}

class RendererSession {
  private readonly scope: Scope
  private readonly tools = new Map<number, ToolHandle>()
  private readonly resources = new Map<number, ResourceHandle>()
  private readonly scopes = new Map<number, Scope>()
  private readonly calls = new Map<string, Pending<unknown>>()
  private readonly reads = new Map<number, Pending<unknown>>()
  /** 页面持有的 hold（页面分配的 holdId → 主进程句柄），会话结束时全部释放。 */
  private readonly holds = new Map<number, HoldHandle>()
  private nextReadId = 1
  private disposed = false
  private readonly onGone = () => this.owner.endSession(this)

  constructor(
    readonly sender: WebContentsLike,
    private readonly owner: Attachment,
    private readonly appMcp: MainAppMcp,
  ) {
    this.scope = appMcp.scope(`renderer-${sender.id}`)
    sender.on('destroyed', this.onGone)
    sender.on('render-process-gone', this.onGone)
  }

  send(event: MainEvent): void {
    if (this.disposed || this.sender.isDestroyed?.()) return
    try {
      this.sender.send(CHANNEL_EVENT, event)
    } catch (error) {
      this.owner.logger.warn('[app-mcp] 向渲染进程发送消息失败', error)
    }
  }

  private registrar(scopeId: number | undefined): Registrar {
    if (scopeId === undefined) return this.scope
    const scope = this.scopes.get(scopeId)
    if (!scope) throw new Error(`未知的 scope ${scopeId}`)
    return scope
  }

  handle(op: RendererOp): unknown {
    switch (op.op) {
      case 'tool.register': {
        requireId(op.id)
        const handle = this.registrar(op.scopeId).tool(op.name, {
          ...toolDefinition(op.spec),
          handler: (input, context) => this.forwardCall(op.id, input, context.callId, context.signal),
        })
        this.tools.set(op.id, handle)
        return undefined
      }
      case 'tool.update':
        this.tools.get(op.id)?.update(toolDefinition(op.spec))
        return undefined
      case 'tool.dispose':
        this.tools.get(op.id)?.dispose()
        this.tools.delete(op.id)
        return undefined
      case 'resource.register': {
        requireId(op.id)
        const handle = this.registrar(op.scopeId).resource(op.name, {
          description: op.description,
          ...(op.mimeType !== undefined && { mimeType: op.mimeType }),
          ...(op.realtime === true && { realtime: true }),
          read: () => this.forwardRead(op.id),
        })
        this.resources.set(op.id, handle)
        return undefined
      }
      case 'resource.notify':
        this.resources.get(op.id)?.notifyChanged()
        return undefined
      case 'resource.dispose':
        this.resources.get(op.id)?.dispose()
        this.resources.delete(op.id)
        return undefined
      case 'scope.create':
        requireId(op.id)
        this.scopes.set(op.id, this.registrar(op.scopeId).scope(op.name))
        return undefined
      case 'scope.dispose':
        this.scopes.get(op.id)?.dispose()
        this.scopes.delete(op.id)
        return undefined
      case 'call.result': {
        const pending = this.calls.get(op.callId)
        if (!pending) return undefined // 已取消或超时
        this.calls.delete(op.callId)
        if (op.ok) pending.resolve({ data: op.data, ...(op.stateHints && { stateHints: op.stateHints }) })
        else pending.reject(new ToolCallError(op.kind, op.message))
        return undefined
      }
      case 'read.result': {
        const pending = this.reads.get(op.readId)
        if (!pending) return undefined
        this.reads.delete(op.readId)
        if (op.ok) pending.resolve(op.data)
        else pending.reject(new ToolCallError(op.kind, op.message))
        return undefined
      }
      case 'lifecycle.wake':
        return this.appMcp.wake?.('app') ?? false
      case 'lifecycle.sleep':
        return this.appMcp.sleep?.() ?? false
      case 'lifecycle.connectNow':
        return this.appMcp.connectNow?.() ?? false
      case 'lifecycle.hold': {
        requireId(op.holdId)
        if (this.holds.has(op.holdId) || !this.appMcp.hold) return undefined
        this.holds.set(op.holdId, this.appMcp.hold())
        return undefined
      }
      case 'lifecycle.release':
        this.holds.get(op.holdId)?.release()
        this.holds.delete(op.holdId)
        return undefined
      default:
        throw new Error(`未知的操作 ${JSON.stringify((op as { op?: unknown }).op)}`)
    }
  }

  private forwardCall(toolId: number, input: unknown, callId: string, signal: AbortSignal): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (this.disposed) {
        reject(new ToolCallError('APP_DISCONNECTED', '页面已关闭'))
        return
      }
      this.calls.set(callId, { resolve, reject })
      signal.addEventListener(
        'abort',
        () => {
          if (!this.calls.delete(callId)) return
          const reason = signal.reason as { kind?: ErrorKind; message?: string } | undefined
          this.send({
            type: 'cancel',
            callId,
            kind: reason?.kind ?? 'CANCELLED',
            message: reason?.message ?? '调用已取消',
          })
          reject(signal.reason)
        },
        { once: true },
      )
      this.send({ type: 'call', callId, toolId, input })
    })
  }

  private forwardRead(resourceId: number): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (this.disposed) {
        reject(new ToolCallError('APP_DISCONNECTED', '页面已关闭'))
        return
      }
      const readId = this.nextReadId++
      this.reads.set(readId, { resolve, reject })
      this.send({ type: 'read', readId, resourceId })
    })
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.sender.removeListener('destroyed', this.onGone)
    this.sender.removeListener('render-process-gone', this.onGone)
    const gone = new ToolCallError('APP_DISCONNECTED', '页面已关闭或刷新')
    for (const p of this.calls.values()) p.reject(gone)
    for (const p of this.reads.values()) p.reject(gone)
    this.calls.clear()
    this.reads.clear()
    this.tools.clear()
    this.resources.clear()
    this.scopes.clear()
    for (const hold of this.holds.values()) hold.release()
    this.holds.clear()
    this.scope.dispose()
  }
}

function requireId(id: unknown): void {
  if (typeof id !== 'number' || !Number.isInteger(id)) throw new Error('非法的 id')
}

function toolDefinition(spec: ToolSpecMessage) {
  if (typeof spec !== 'object' || spec === null || typeof spec.description !== 'string') {
    throw new Error('非法的工具定义')
  }
  return {
    description: spec.description,
    ...(spec.title !== undefined && { title: spec.title }),
    // 缺省为无参数：显式写出，以便 update 时能清除之前的 schema。
    input: spec.inputSchema ?? { type: 'object' as const, properties: {} },
    ...(spec.risk !== undefined && { risk: spec.risk }),
    ...(spec.activation !== undefined && { activation: spec.activation }),
    ...(spec.enabled !== undefined && { enabled: spec.enabled }),
  }
}

class Attachment implements AppMcpAttachment {
  private readonly sessions = new Map<number, RendererSession>()
  private readonly unsubscribe: () => void
  private disposed = false
  readonly logger: Pick<Logger, 'warn' | 'error'>

  constructor(private readonly options: AttachOptions) {
    this.logger = options.logger ?? console
    options.ipcMain.handle(CHANNEL_OP, (event, op: unknown) => this.onOp(event.sender, op))
    this.unsubscribe = options.appMcp.onStateChange((state) => this.broadcast(state))
  }

  get sessionCount(): number {
    return this.sessions.size
  }

  private accepts(sender: WebContentsLike): boolean {
    const filter = this.options.webContents
    if (filter === undefined) return true
    if (typeof filter === 'function') return filter(sender)
    return Array.isArray(filter) ? filter.some((wc) => wc.id === sender.id) : filter.id === sender.id
  }

  /** 主进程客户端当前的连接 ID（缺省时省略字段，与旧版本消息同形）。 */
  private connectionIdField(): { connectionId?: string } {
    const connectionId = this.options.appMcp.connectionId
    return typeof connectionId === 'string' && connectionId !== '' ? { connectionId } : {}
  }

  private broadcast(state: ConnectionState): void {
    const event: MainEvent = { type: 'state', state, ...this.connectionIdField() }
    for (const session of this.sessions.values()) session.send(event)
  }

  endSession(session: RendererSession): void {
    if (this.sessions.get(session.sender.id) === session) this.sessions.delete(session.sender.id)
    session.dispose()
  }

  private onOp(sender: WebContentsLike, raw: unknown): OpReply {
    if (this.disposed) return { ok: false, code: 'DISPOSED', message: 'app-mcp 已停止' }
    if (!this.accepts(sender)) return { ok: false, code: 'FORBIDDEN', message: '该页面不允许登记 app-mcp 工具' }
    if (typeof raw !== 'object' || raw === null || typeof (raw as { op?: unknown }).op !== 'string') {
      return { ok: false, code: 'INVALID_OP', message: '非法的消息' }
    }
    const op = raw as RendererOp
    try {
      const existing = this.sessions.get(sender.id)
      if (op.op === 'hello' || op.op === 'reset') {
        if (existing) this.endSession(existing)
        if (op.op === 'reset') return { ok: true }
        const reply: HelloReply = {
          instanceId: this.options.appMcp.instanceId,
          state: this.options.appMcp.state,
          ...this.connectionIdField(),
        }
        return { ok: true, value: reply }
      }
      let session = existing
      if (!session) {
        session = new RendererSession(sender, this, this.options.appMcp)
        this.sessions.set(sender.id, session)
      }
      return { ok: true, value: session.handle(op) }
    } catch (error) {
      const code = (error as { code?: unknown } | null)?.code
      return {
        ok: false,
        ...(typeof code === 'string' && { code }),
        message: error instanceof Error ? error.message : String(error),
      }
    }
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.options.ipcMain.removeHandler(CHANNEL_OP)
    this.unsubscribe()
    for (const session of [...this.sessions.values()]) this.endSession(session)
  }
}

/** 在主进程上接入渲染进程的 app-mcp 登记。每个 ipcMain 只能接入一次（`ipcMain.handle` 的限制）。 */
export function attachAppMcp(options: AttachOptions): AppMcpAttachment {
  return new Attachment(options)
}

export type { Outcome }

// ---------------------------------------------------------------------------
// 生命周期（spec/lifecycle.md 第 5 节：主进程 `second-instance` / `open-url` → `handleWake`）
// ---------------------------------------------------------------------------

/** Electron `app` 的最小接口。 */
export interface ElectronAppLike {
  on(event: 'second-instance', listener: (event: unknown, argv: string[]) => void): unknown
  on(event: 'open-url', listener: (event: { preventDefault?(): void }, url: string) => void): unknown
  removeListener(event: 'second-instance' | 'open-url', listener: (...args: any[]) => void): unknown
  quit(): void
}

/** Electron `BrowserWindow` 的最小接口（用于上报可见性）。 */
export interface BrowserWindowLike {
  isDestroyed?(): boolean
  isVisible(): boolean
  isMinimized(): boolean
  isFocused(): boolean
  on(event: WindowVisibilityEvent, listener: () => void): unknown
  removeListener(event: WindowVisibilityEvent, listener: () => void): unknown
}

type WindowVisibilityEvent = 'show' | 'hide' | 'minimize' | 'restore' | 'focus' | 'blur' | 'closed'

export interface LifecycleAttachOptions {
  appMcp: Pick<AppMcp, 'handleWake' | 'onIdleExit' | 'setVisibility'>
  app: ElectronAppLike
  /**
   * 启动参数，缺省 `process.argv`：由唤醒冷启动时（如 `app-mcp-wake:<token>`）在这里识别。
   * 传 `[]` 跳过。
   */
  argv?: readonly string[]
  /** 休眠且 `residency` 允许退出时调用 `app.quit()`，缺省 true。为 false 时由 App 自行订阅 `onIdleExit`。 */
  quitOnIdleExit?: boolean
  /** 按该窗口的显示 / 最小化 / 焦点上报可见性（`setVisibility`）。 */
  window?: BrowserWindowLike
  /** 识别到唤醒后调用（如需要时显示窗口）。 */
  onWake?: () => void
}

/**
 * 把 Electron 主进程的激活事件接到 @app-mcp/node 的生命周期上：
 *
 * ```ts
 * if (!app.requestSingleInstanceLock()) app.quit()
 * const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城', lifecycle: { mode: 'idle', wake: { kind: 'uri', target: 'shop://' } } })
 * attachLifecycle({ appMcp, app, window: mainWindow })
 * ```
 *
 * 返回解除函数。`second-instance` 需要 App 自己申请单实例锁；`open-url` 仅 macOS 触发。
 */
export function attachLifecycle(options: LifecycleAttachOptions): () => void {
  const { appMcp, app } = options
  const cleanups: Array<() => void> = []
  const wake = (args: string | readonly string[]): boolean => {
    if (!appMcp.handleWake(args)) return false
    options.onWake?.()
    return true
  }

  const onSecondInstance = (_event: unknown, argv: string[]) => {
    if (Array.isArray(argv)) wake(argv)
  }
  const onOpenUrl = (event: { preventDefault?(): void }, url: string) => {
    if (typeof url === 'string' && wake(url)) event?.preventDefault?.()
  }
  app.on('second-instance', onSecondInstance)
  app.on('open-url', onOpenUrl)
  cleanups.push(() => {
    app.removeListener('second-instance', onSecondInstance)
    app.removeListener('open-url', onOpenUrl)
  })

  if (options.quitOnIdleExit !== false) cleanups.push(appMcp.onIdleExit(() => app.quit()))

  const win = options.window
  if (win) {
    const report = () => {
      if (win.isDestroyed?.()) return
      const visible = win.isVisible() && !win.isMinimized()
      appMcp.setVisibility(visible ? 'visible' : 'hidden', visible && win.isFocused())
    }
    const onClosed = () => appMcp.setVisibility('hidden', false)
    const events: WindowVisibilityEvent[] = ['show', 'hide', 'minimize', 'restore', 'focus', 'blur']
    for (const e of events) win.on(e, report)
    win.on('closed', onClosed)
    cleanups.push(() => {
      for (const e of events) win.removeListener(e, report)
      win.removeListener('closed', onClosed)
    })
    report()
  }

  const argv = options.argv ?? (typeof process !== 'undefined' ? process.argv : [])
  if (argv.length > 0) wake(argv)

  let detached = false
  return () => {
    if (detached) return
    detached = true
    for (const cleanup of cleanups.splice(0)) cleanup()
  }
}
