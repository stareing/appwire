/**
 * Electron 渲染进程模式：页面经 preload 暴露的桥接对象把工具 / 资源登记到主进程，
 * 由主进程的 @app-mcp/node 客户端代为连接 Host。此模式下不加载 WASM、不建 WebSocket。
 *
 * 消息格式是 @app-mcp/electron 的 op/event 协议（主进程实现见 packages/electron/src/main.ts），
 * 本文件是该协议消息类型的唯一定义，@app-mcp/electron 从这里重新导出。
 *
 * - 页面 → 主进程：`bridge.request(op)`（preload 内为 `ipcRenderer.invoke('app-mcp:op', op)`），返回 {@link OpReply}。
 * - 主进程 → 页面：`bridge.onMessage(listener)` 收到 {@link MainEvent}。
 *
 * `createAppMcp` 检测到桥接（{@link findElectronBridge}）时自动走这条路径；页面代码无需修改。
 * 身份与连接由主进程负责：`appId` / `appName` / `hostUrl` 等选项在页面中被忽略，
 * `instanceId`、`state` 与 `connectionId` 取自主进程客户端（首次 hello 完成前 `instanceId` 为空字符串）。
 */

import { attachBridgeNavigation, type BridgeNavigation } from './bridge-navigation'
import { checkHandlerOrLoad, loadHandler, type LazySlot } from './lazy'
import { noopHold } from './noop'
import { normalizeToolResult, toJsonValue, type NormalizedResult } from './result'
import { describeParseError, isZodLike, toJsonSchema, toOutputSchema } from './schema'
import { checkPageName, type ScopeChain, ViewDeclaration } from './view'
import type {
  Activation,
  AppMcp,
  AppMcpOptions,
  ConnectionState,
  ContentAnnotations,
  ErrorKind,
  HoldHandle,
  JsonSchema,
  LazyToolDefinition,
  Logger,
  NavigationHandler,
  NavigationOptions,
  OutputSchema,
  ResourceDefinition,
  ResourceHandle,
  Risk,
  Scope,
  ScopeOptions,
  ToolAnnotations,
  ToolDefinition,
  ToolHandle,
  ToolHandler,
  ToolHandlerLoader,
  ToolSurface,
} from './types'

// ---------------------------------------------------------------------------
// 协议（BRIDGE_VERSION = 1）
// ---------------------------------------------------------------------------
//
// @compat 版本 1 内只做可选字段的新增，旧页面忽略、新页面缺省为 undefined，因此不升版本
// （升版本会让 findElectronBridge 拒绝新旧混用）。已有新增：`HelloReply.connectionId`、`state` 事件的 `connectionId`、
// `ToolSpecMessage.annotations` / `outputSchema` / `surface` / `page` / `backgroundTool`、导航消息（`navigation.set`、`navigate`、
// `navigate.result`，见 {@link NavigationOp}；旧主进程对未知 op 回复错误，页面据此得知不支持；`navigate.result` 的
// `USER_ACTION_REQUIRED` 与 `details`：旧主进程 / Rust 侧按失败处理）、成功 `Outcome` 的 `status` / `stateResource` / `summary` / `annotations`、
// 失败 `Outcome` 的 `details`。

/** preload 默认把桥接对象暴露为 `window.appMcpBridge`。 */
export const DEFAULT_BRIDGE_KEY = 'appMcpBridge'
export const BRIDGE_VERSION = 1

export interface ToolSpecMessage {
  description: string
  title?: string
  inputSchema?: JsonSchema
  risk?: Risk
  /** 标准 MCP 工具注解；`tool.update` 时缺省表示清除。 */
  annotations?: ToolAnnotations
  /** 结果的 JSON Schema；`tool.update` 时缺省表示清除。 */
  outputSchema?: OutputSchema
  activation?: Activation
  enabled?: boolean
  /**
   * 对界面的依赖（spec/protocol.md 3.4）；缺省 `app`。`view` 工具的可见性门控在页面侧完成，结果体现在 `enabled` 中
   * （门控变化时页面发 `tool.update`）。`tool.update` 时缺省表示清除。
   */
  surface?: ToolSurface
  /** 所在页面名（spec/protocol.md 3.4）。 */
  page?: string
  /** 后台替代：同一 App 中一个 `app` 工具的名称（spec/protocol.md 3.4）；`tool.update` 时缺省表示清除。 */
  backgroundTool?: string
}

/**
 * 成功结果的字段同 {@link NormalizedResult}（`data` 之外均可选，旧主进程忽略新增字段）。
 * 失败的 `details` 来自 ToolCallError（JSON 对象，如 `USER_ACTION_REQUIRED` 的 `{ reason?, uri? }`），随错误的 `data` 发给 Host。
 */
export type Outcome =
  | ({ ok: true } & NormalizedResult)
  | { ok: false; kind: ErrorKind; message: string; details?: Record<string, unknown> }

export type RendererOp =
  /** 页面（重新）加载：主进程丢弃该 webContents 之前的全部登记。 */
  | { op: 'hello' }
  /** 页面卸载或 dispose：注销该 webContents 的全部登记。 */
  | { op: 'reset' }
  | { op: 'tool.register'; id: number; scopeId?: number; name: string; spec: ToolSpecMessage }
  | { op: 'tool.update'; id: number; spec: ToolSpecMessage }
  | { op: 'tool.dispose'; id: number }
  | {
      op: 'resource.register'
      id: number
      scopeId?: number
      name: string
      description: string
      mimeType?: string
      realtime?: boolean
      annotations?: ContentAnnotations
    }
  | { op: 'resource.notify'; id: number }
  | { op: 'resource.dispose'; id: number }
  | { op: 'scope.create'; id: number; scopeId?: number; name: string }
  | { op: 'scope.dispose'; id: number }
  | ({ op: 'call.result'; callId: string } & Outcome)
  /** handler 的 `context.progress()`（spec/protocol.md 3.3），主进程转给 @app-mcp/node 的同名方法。 */
  | { op: 'call.progress'; callId: string; progress: number; total?: number; message?: string }
  | ({ op: 'read.result'; readId: number } & Outcome)
  /** 生命周期（转给主进程的 @app-mcp/node 客户端）：回连。 */
  | { op: 'lifecycle.wake' }
  /** 主动请求休眠。 */
  | { op: 'lifecycle.sleep' }
  /** `on-demand` 模式下主动连接。 */
  | { op: 'lifecycle.connectNow' }
  /** 持有（阻止自动休眠），`holdId` 由页面分配；页面刷新、卸载或 webContents 销毁时主进程释放该页全部持有。 */
  | { op: 'lifecycle.hold'; holdId: number }
  | { op: 'lifecycle.release'; holdId: number }
  | NavigationOp

/**
 * 导航（spec/protocol.md 3.4）：页面 → 主进程 / Rust 侧。`navigation.set`：本页处理（`enabled: true`）/ 不再处理导航，
 * 对方以最近一次开启的页面为目标，未开启导航转发时回复错误；`navigate.result`：回复一次 {@link NavigateEvent}。
 */
export type NavigationOp =
  | { op: 'navigation.set'; enabled: boolean }
  | { op: 'navigate.result'; navId: number; ok: true }
  | {
      op: 'navigate.result'
      navId: number
      ok: false
      kind: 'NAVIGATION_FAILED' | 'NAVIGATION_DENIED' | 'USER_ACTION_REQUIRED'
      message: string
      /** `USER_ACTION_REQUIRED` 的 `{ reason?, uri? }`；其他类别不带。 */
      details?: Record<string, unknown>
    }

/** 主进程 / Rust 侧请求页面导航（Host 的 `app/navigate`）。`params` 缺省 = Host 没有给出参数。 */
export interface NavigateEvent {
  type: 'navigate'
  navId: number
  page: string
  params?: unknown
}

export interface HelloReply {
  instanceId: string
  state: ConnectionState
  /** 主进程客户端当前的连接 ID（spec/protocol.md 10.3）；未连接或旧主进程时缺省。 */
  connectionId?: string
}

export type OpReply = { ok: true; value?: unknown } | { ok: false; code?: string; message: string }

export type MainEvent =
  | { type: 'call'; callId: string; toolId: number; input: unknown }
  | { type: 'cancel'; callId: string; kind: ErrorKind; message: string }
  | { type: 'read'; readId: number; resourceId: number }
  /** `connectionId`：该状态下主进程客户端的连接 ID；未连接或旧主进程时缺省。 */
  | { type: 'state'; state: ConnectionState; connectionId?: string }
  | NavigateEvent

/** preload 暴露给页面的最小桥接对象。 */
export interface AppMcpBridge {
  readonly version: number
  request(op: RendererOp): Promise<OpReply>
  /** 订阅主进程事件，返回取消订阅函数。 */
  onMessage(listener: (event: MainEvent) => void): () => void
}

// ---------------------------------------------------------------------------
// 检测
// ---------------------------------------------------------------------------

function isBridge(value: unknown): value is AppMcpBridge {
  if (typeof value !== 'object' || value === null) return false
  const b = value as Partial<AppMcpBridge>
  return typeof b.request === 'function' && typeof b.onMessage === 'function'
}

/**
 * 查找 Electron preload 暴露的桥接对象：依次尝试 `getAppMcpBridge()`（若页面上有这个函数）与 `appMcpBridge`。
 * 版本不兼容时返回 undefined（调用方退回 WebSocket + WASM）。
 */
export function findElectronBridge(
  target: Record<string, unknown> = globalThis as unknown as Record<string, unknown>,
  key: string = DEFAULT_BRIDGE_KEY,
): AppMcpBridge | undefined {
  let candidate: unknown
  try {
    const getter = target.getAppMcpBridge
    if (typeof getter === 'function') candidate = (getter as () => unknown)()
    if (!isBridge(candidate)) candidate = target[key]
  } catch {
    return undefined
  }
  if (!isBridge(candidate)) return undefined
  const version = (candidate as { version?: unknown }).version
  if (version !== undefined && version !== BRIDGE_VERSION) return undefined
  return candidate
}

// ---------------------------------------------------------------------------
// 实现
// ---------------------------------------------------------------------------

const defaultLogger: Logger = {
  debug() {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

/** 与 ToolCallError 同形的错误（按 name + kind 识别）。 */
function callError(kind: ErrorKind, message: string): Error & { kind: ErrorKind } {
  const error = new Error(message) as Error & { kind: ErrorKind }
  error.name = 'ToolCallError'
  error.kind = kind
  return error
}

/** ToolCallError 的详情 → 可跨 IPC 传递的 JSON 对象；不是对象或无法序列化时丢弃（错误本身照常上报）。 */
function outcomeDetails(details: unknown): Record<string, unknown> | undefined {
  if (typeof details !== 'object' || details === null || Array.isArray(details)) return undefined
  try {
    const json = toJsonValue(details)
    return typeof json === 'object' && json !== null && !Array.isArray(json) ? (json as Record<string, unknown>) : undefined
  } catch {
    return undefined
  }
}

function toOutcomeError(error: unknown): Outcome {
  if (typeof error === 'object' && error !== null) {
    const e = error as { name?: unknown; kind?: unknown; message?: unknown }
    const message = typeof e.message === 'string' ? e.message : String(error)
    if (e.name === 'ToolCallError' && typeof e.kind === 'string') {
      const details = outcomeDetails((error as { details?: unknown }).details)
      return details === undefined
        ? { ok: false, kind: e.kind as ErrorKind, message }
        : { ok: false, kind: e.kind as ErrorKind, message, details }
    }
    return { ok: false, kind: 'HANDLER_ERROR', message: message || 'handler 执行失败' }
  }
  return { ok: false, kind: 'HANDLER_ERROR', message: String(error) }
}

interface ResolvedInput {
  schema: JsonSchema | undefined
  parse?: ((input: unknown) => unknown) | undefined
}

/** 输入定义 → schema 与 zod 校验函数（需要加载 zod 时返回 Promise；定义不合法时同步抛出）。 */
function resolveInput(input: unknown): ResolvedInput | Promise<ResolvedInput> {
  if (input === undefined) return { schema: undefined }
  const parse = isZodLike(input) ? (value: unknown) => input.parse(value) : undefined
  const schema = toJsonSchema(input as ToolDefinition['input'])
  return schema instanceof Promise ? schema.then((s) => ({ schema: s, parse })) : { schema, parse }
}

/** 输出定义 → JSON Schema（缺省为 undefined；需要加载 zod 时返回 Promise；定义不合法时同步抛出）。 */
function resolveOutput(output: ToolDefinition['outputSchema']): OutputSchema | undefined | Promise<OutputSchema> {
  return output === undefined ? undefined : toOutputSchema(output)
}

type AnyDef = ToolDefinition<any, any> | LazyToolDefinition<any, any>

interface Detachable {
  detach(): void
}

interface Owner {
  readonly scopeId: number | undefined
  readonly children: Set<Detachable>
  /** scope 的界面声明（{@link ScopeOptions}）与上层；实例本身两者都为 undefined。 */
  readonly viewOptions: ScopeOptions | undefined
  readonly parentOwner: Owner | undefined
}

/** 所在 scope 链（自近到远）上的界面声明。 */
function ownerChain(owner: Owner): ScopeChain {
  return function* () {
    for (let o: Owner | undefined = owner; o; o = o.parentOwner) yield o.viewOptions
  }
}

function pageDocument(): Document | undefined {
  return typeof document === 'undefined' ? undefined : document
}

class Client {
  private queue: Promise<unknown> = Promise.resolve()
  private nextId = 1
  readonly tools = new Map<number, ToolEntry>()
  readonly resources = new Map<number, ResourceEntry>()
  readonly calls = new Map<string, AbortController>()
  private readonly holds = new Set<number>()
  private holdsClosed = false

  constructor(
    readonly bridge: AppMcpBridge | null,
    readonly logger: Logger,
  ) {}

  id(): number {
    return this.nextId++
  }

  /** 按顺序发送操作（`build` 在前一个操作完成后才执行，可等待 schema 解析）。 */
  enqueue(build: () => RendererOp | null | Promise<RendererOp | null>): Promise<OpReply | undefined> {
    const bridge = this.bridge
    if (!bridge) return Promise.resolve(undefined)
    const next = this.queue.then(async () => {
      const op = await build()
      if (!op) return undefined
      const reply = await bridge.request(op)
      if (!reply.ok) this.logger.error(`[app-mcp] ${op.op} 失败：${reply.message}`)
      return reply
    })
    this.queue = next.catch((error: unknown) => this.logger.error('[app-mcp] IPC 请求失败', error))
    return next
  }

  /** 持有：经主进程转给 @app-mcp/node 的 `hold()`；`release()` 幂等。 */
  hold(): HoldHandle {
    if (!this.bridge || this.holdsClosed) return noopHold()
    const holdId = this.id()
    this.holds.add(holdId)
    void this.enqueue(() => ({ op: 'lifecycle.hold', holdId }))
    return {
      release: () => {
        if (!this.holds.delete(holdId)) return
        void this.enqueue(() => ({ op: 'lifecycle.release', holdId }))
      },
    }
  }

  /** 实例注销：主进程随 `reset` 释放全部持有，这里只清本地记录（之后的 release 为空操作）。 */
  dropHolds(): void {
    this.holdsClosed = true
    this.holds.clear()
  }

  /** 结果不排队：避免被等待中的注册阻塞。 */
  send(op: RendererOp): void {
    this.bridge?.request(op).then(
      (reply) => {
        if (!reply.ok) this.logger.warn(`[app-mcp] ${op.op} 失败：${reply.message}`)
      },
      (error: unknown) => this.logger.error('[app-mcp] IPC 请求失败', error),
    )
  }

  onCall(callId: string, toolId: number, input: unknown): void {
    const entry = this.tools.get(toolId)
    if (!entry) {
      this.send({ op: 'call.result', callId, ok: false, kind: 'TOOL_NOT_FOUND', message: '工具已注销' })
      return
    }
    const controller = new AbortController()
    this.calls.set(callId, controller)
    const finish = (outcome: Outcome) => {
      if (controller.signal.aborted) return
      this.calls.delete(callId)
      this.send({ op: 'call.result', callId, ...outcome })
    }
    let value = input
    const parse = entry.parse
    if (parse) {
      try {
        value = parse(input)
      } catch (error) {
        finish({ ok: false, kind: 'INVALID_INPUT', message: describeParseError(error).message })
        return
      }
    }
    const handler: Promise<ToolHandler<any, any>> = entry.handler
      ? Promise.resolve(entry.handler)
      : loadHandler(entry).catch((error: unknown) => {
          throw callError(
            'HANDLER_ERROR',
            `加载工具 ${entry.name} 的 handler 失败：${error instanceof Error ? error.message : String(error)}`,
          )
        })
    handler
      .then((fn) =>
        fn(value, {
          callId,
          signal: controller.signal,
          hold: () => this.hold(),
          progress: (progress: number, total?: number, message?: string) => {
            if (controller.signal.aborted || !this.calls.has(callId)) return
            this.send({
              op: 'call.progress',
              callId,
              progress,
              ...(total !== undefined && { total }),
              ...(message !== undefined && { message }),
            })
          },
        }),
      )
      .then(
        (result) => {
          let normalized: NormalizedResult
          try {
            normalized = normalizeToolResult(result)
          } catch (error) {
            finish({ ok: false, kind: 'HANDLER_ERROR', message: `返回值无法序列化为 JSON：${String(error)}` })
            return
          }
          finish({ ok: true, ...normalized })
        },
        (error: unknown) => finish(toOutcomeError(error)),
      )
  }

  onRead(readId: number, resourceId: number): void {
    const entry = this.resources.get(resourceId)
    if (!entry) {
      this.send({ op: 'read.result', readId, ok: false, kind: 'RESOURCE_NOT_FOUND', message: '资源已注销' })
      return
    }
    const reader = entry.reader
    Promise.resolve()
      .then(() => reader())
      .then(
        (value) => {
          let json: unknown
          try {
            json = toJsonValue(value)
          } catch (error) {
            this.send({ op: 'read.result', readId, ok: false, kind: 'HANDLER_ERROR', message: String(error) })
            return
          }
          this.send({ op: 'read.result', readId, ok: true, data: json })
        },
        (error: unknown) => this.send({ op: 'read.result', readId, ...toOutcomeError(error) }),
      )
  }

  onCancel(callId: string, kind: ErrorKind, message: string): void {
    const controller = this.calls.get(callId)
    if (!controller) return
    this.calls.delete(callId)
    controller.abort(callError(kind, message))
  }
}

class ToolEntry implements ToolHandle, Detachable, LazySlot {
  readonly id: number
  private def: Omit<AnyDef, 'handler' | 'load'>
  handler: ToolHandler<any, any> | undefined
  load: ToolHandlerLoader<any, any> | undefined
  loading: Promise<ToolHandler<any, any>> | undefined
  schema: ResolvedInput['schema']
  parse: ResolvedInput['parse']
  outputSchema: OutputSchema | undefined
  /** 界面声明与可见性门控（只在有桥接时创建）。 */
  private decl: ViewDeclaration | undefined
  private disposed = false

  constructor(
    private readonly client: Client,
    private readonly owner: Owner,
    readonly name: string,
    definition: AnyDef,
  ) {
    const { handler, load, ...meta } = definition
    this.def = meta
    this.handler = handler
    this.load = load
    this.id = client.id()
    if (!client.bridge) return
    // 定义不合法时同步抛出（与驱动层一致）；需要加载 zod 时异步解析。
    checkHandlerOrLoad(name, definition)
    // 门控变化：重新发送定义（`enabled` 为生效值）
    this.decl = new ViewDeclaration(name, definition, ownerChain(owner), pageDocument(), () => this.resend())
    const resolved = resolveInput(definition.input)
    const output = resolveOutput(definition.outputSchema)
    client.tools.set(this.id, this)
    owner.children.add(this)
    client.enqueue(async () => {
      const [r, o] = await Promise.all([resolved, output])
      if (this.disposed) return null
      this.schema = r.schema
      this.parse = r.parse
      this.outputSchema = o
      return { op: 'tool.register', id: this.id, ...scopeField(owner), name, spec: this.spec() }
    })
  }

  private spec(): ToolSpecMessage {
    const d = this.def
    return {
      description: d.description,
      ...(d.title !== undefined && { title: d.title }),
      ...(this.schema !== undefined && { inputSchema: this.schema }),
      ...(d.risk !== undefined && { risk: d.risk }),
      ...(d.annotations !== undefined && { annotations: d.annotations }),
      ...(this.outputSchema !== undefined && { outputSchema: this.outputSchema }),
      ...(d.activation !== undefined && { activation: d.activation }),
      ...((d.enabled !== undefined || this.decl?.gated) && { enabled: this.decl?.enabled ?? d.enabled }),
      ...(this.decl?.surface === 'view' && { surface: 'view' as const }),
      ...(this.decl?.page !== undefined && { page: this.decl.page }),
      ...(d.backgroundTool !== undefined && { backgroundTool: d.backgroundTool }),
    }
  }

  private resend(): void {
    if (this.disposed) return
    this.client.enqueue(() => (this.disposed ? null : { op: 'tool.update', id: this.id, spec: this.spec() }))
  }

  update(changes: Parameters<ToolHandle['update']>[0]): void {
    if (this.disposed) return
    const { handler: _h, load: _l, ...meta } = changes as Record<string, unknown>
    this.decl?.apply(changes)
    this.def = { ...this.def, ...meta }
    const resolved = 'input' in changes ? resolveInput(changes.input) : undefined
    const output = 'outputSchema' in changes ? resolveOutput(changes.outputSchema) : undefined
    this.client.enqueue(async () => {
      if (resolved) {
        const r = await resolved
        this.schema = r.schema
        this.parse = r.parse
      }
      if ('outputSchema' in changes) this.outputSchema = await output
      return this.disposed ? null : { op: 'tool.update', id: this.id, spec: this.spec() }
    })
  }

  setHandler(handler: ToolHandler<any, any>): void {
    this.handler = handler
    this.load = undefined
    this.loading = undefined
  }

  dispose(): void {
    if (this.disposed) return
    this.detach()
    this.owner.children.delete(this)
    this.client.enqueue(() => ({ op: 'tool.dispose', id: this.id }))
  }

  detach(): void {
    this.disposed = true
    this.decl?.dispose()
    this.client.tools.delete(this.id)
  }
}

class ResourceEntry implements ResourceHandle, Detachable {
  readonly id: number
  private disposed = false

  constructor(
    private readonly client: Client,
    private readonly owner: Owner,
    readonly name: string,
    definition: ResourceDefinition<any>,
    public reader: ResourceDefinition<any>['read'],
  ) {
    this.id = client.id()
    if (!client.bridge) return
    client.resources.set(this.id, this)
    owner.children.add(this)
    client.enqueue(() => ({
      op: 'resource.register',
      id: this.id,
      ...scopeField(owner),
      name,
      description: definition.description,
      ...(definition.mimeType !== undefined && { mimeType: definition.mimeType }),
      ...(definition.realtime && { realtime: true }),
      ...(definition.annotations !== undefined && { annotations: toJsonValue(definition.annotations) as ContentAnnotations }),
    }))
  }

  notifyChanged(): void {
    if (this.disposed) return
    this.client.enqueue(() => ({ op: 'resource.notify', id: this.id }))
  }

  setReader(read: ResourceDefinition<any>['read']): void {
    this.reader = read
  }

  dispose(): void {
    if (this.disposed) return
    this.detach()
    this.owner.children.delete(this)
    this.client.enqueue(() => ({ op: 'resource.dispose', id: this.id }))
  }

  detach(): void {
    this.disposed = true
    this.client.resources.delete(this.id)
  }
}

function scopeField(owner: Owner): { scopeId?: number } {
  return owner.scopeId === undefined ? {} : { scopeId: owner.scopeId }
}

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
        this.client.onCall(event.callId, event.toolId, event.input)
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
