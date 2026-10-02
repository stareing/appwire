/**
 * createAppMcp：在原生客户端（bindings/node）之上实现与 @app-mcp/web 同形的 JS API。
 */

import { ToolCallError, nativeErrorCode, toFailure } from './errors.js'
import { checkHandlerOrLoad, loadHandler, type LazySlot } from './lazy.js'
import {
  loadNativeBinding,
  type NativeCall,
  type NativeClientConfig,
  type NativeClient,
  type NativeClientEvent,
  type NativeNavigate,
  type NativeRead,
  type NativeRegistrar,
  type NativeResource,
  type NativeScope,
  type NativeStateInfo,
  type NativeTool,
  type NativeToolSpec,
} from './native.js'
import { hasResultExtras, normalizeToolResult, type NormalizedResult } from './result.js'
import { describeParseError, resolveInput, resolveOutput, type ResolvedInput } from './schema.js'
import type {
  AppMcp,
  ConnectionState,
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
  ToolHandler,
  ToolHandlerLoader,
  Visibility,
} from './types.js'

const defaultLogger: Logger = {
  debug() {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

/** 可被父级（scope / 客户端）批量注销的子项。 */
interface Child {
  /** 父级已在原生侧注销：只更新本地状态，不再调用原生 dispose。 */
  detach(): void
}

interface Owner {
  readonly children: Set<Child>
}

function mapState(info: NativeStateInfo): ConnectionState {
  switch (info.status) {
    case 'backoff':
      return {
        status: 'backoff',
        retryAt: Date.now() + (info.retryInMs ?? 0),
        ...(info.reason != null && { reason: info.reason }),
        ...(info.code != null && { code: info.code }),
      }
    case 'rejected':
      return { status: 'rejected', reason: info.reason ?? '', code: info.code ?? 'REJECTED' }
    case 'host-mismatch':
      return { status: 'host-mismatch', reason: info.reason ?? '', code: info.code ?? 'HOST_NOT_APP_MCP' }
    default:
      return { status: info.status }
  }
}

/** 输入与输出定义解析后的结果。 */
interface ResolvedTool extends ResolvedInput {
  /** 输出 schema 的 JSON 文本；`undefined` 表示未声明。 */
  outputSchemaJson: string | undefined
}

/** 解析输入与输出定义：都能同步完成时同步返回（同步注册的错误直接抛给调用方），否则返回 Promise。 */
function resolveTool(def: Pick<ToolDefinition<any, any>, 'input' | 'outputSchema'>): ResolvedTool | Promise<ResolvedTool> {
  const input = resolveInput(def.input)
  const output = resolveOutput(def.outputSchema)
  if (!(input instanceof Promise) && !(output instanceof Promise)) return { ...input, outputSchemaJson: output }
  return Promise.all([input, output]).then(([i, o]) => ({ ...i, outputSchemaJson: o }))
}

/** 空操作的持有（未启用 / 旧版原生模块）。 */
const NOOP_HOLD: HoldHandle = Object.freeze({ release() {} })

/** 包装原生持有：`release` 幂等。 */
function wrapHold(hold: { release(): void }): HoldHandle {
  let released = false
  return {
    release() {
      if (released) return
      released = true
      hold.release()
    },
  }
}

/**
 * 以失败完成调用或资源读取：`ToolCallError` 的类别与详情（含 `userActionRequired` 的 `reason` / `uri`）原样提交。
 *
 * @error 详情无法序列化为 JSON 时记警告并按无详情提交；原生模块旧（无 `failWithDetails`）时同样丢弃详情。
 */
function submitFailure(
  target: Pick<NativeCall, 'fail' | 'failWithDetails'>,
  error: unknown,
  logger: Logger,
  label: string,
): void {
  const { kind, message, details } = toFailure(error)
  let detailsJson: string | undefined
  if (details !== undefined && target.failWithDetails) {
    try {
      detailsJson = JSON.stringify(details)
    } catch (e) {
      logger.warn(`[app-mcp] ${label} 的错误详情无法序列化为 JSON，已忽略`, e)
    }
  }
  if (detailsJson !== undefined) target.failWithDetails!(kind, message, detailsJson)
  else target.fail(kind, message)
}

function stringifyJson(value: unknown): string {
  const text = JSON.stringify(value === undefined ? null : value)
  return text === undefined ? 'null' : text
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/** 工具元数据（不含 handler / load）。 */
type ToolMeta = Omit<ToolDefinition<any, any>, 'handler' | 'load'>

class ToolEntry implements ToolHandle, Child, LazySlot {
  readonly name: string
  private def: ToolMeta
  handler: ToolHandler<any, any> | undefined
  load: ToolHandlerLoader<any, any> | undefined
  loading: Promise<ToolHandler<any, any>> | undefined
  private resolved: ResolvedTool | undefined
  private native: NativeTool | undefined
  private disposed = false
  /** 每次解析输入定义递增，丢弃过期的异步结果。 */
  private resolveSeq = 0

  constructor(
    private readonly registrar: NativeRegistrar | null,
    private readonly owner: Owner | null,
    name: string,
    definition: ToolDefinition<any, any> | LazyToolDefinition<any, any>,
    private readonly logger: Logger,
  ) {
    checkHandlerOrLoad(name, definition)
    this.name = name
    const { handler, load, ...meta } = definition
    this.def = meta
    this.handler = handler
    this.load = load
    if (!registrar) return
    const resolved = resolveTool(definition)
    if (resolved instanceof Promise) {
      const seq = ++this.resolveSeq
      owner?.children.add(this)
      resolved.then(
        (r) => {
          if (this.disposed || seq !== this.resolveSeq) return
          this.resolved = r
          try {
            this.native = registrar.registerTool(this.spec(), (call) => this.onCall(call))
          } catch (error) {
            this.logger.error(`[app-mcp] 注册工具 ${name} 失败`, error)
            this.disposed = true
            owner?.children.delete(this)
          }
        },
        (error: unknown) => {
          this.logger.error(`[app-mcp] 工具 ${name} 的 input / outputSchema 定义无效`, error)
        },
      )
      return
    }
    this.resolved = resolved
    // 同步注册：重名、非法名称等错误直接抛给调用方。
    this.native = registrar.registerTool(this.spec(), (call) => this.onCall(call))
    owner?.children.add(this)
  }

  private spec(): NativeToolSpec {
    const d = this.def
    const spec: NativeToolSpec = { name: this.name, description: d.description, enabled: d.enabled ?? true }
    if (this.resolved?.schemaJson !== undefined) spec.inputSchemaJson = this.resolved.schemaJson
    if (d.risk !== undefined) spec.risk = d.risk
    if (d.activation !== undefined) spec.activation = d.activation
    if (d.title !== undefined) spec.title = d.title
    if (d.annotations !== undefined) spec.annotations = { ...d.annotations }
    if (this.resolved?.outputSchemaJson !== undefined) spec.outputSchemaJson = this.resolved.outputSchemaJson
    if (d.surface !== undefined) spec.surface = d.surface
    if (d.page !== undefined) spec.page = d.page
    return spec
  }

  update(changes: Partial<Omit<ToolDefinition<any, any>, 'handler'>>): void {
    if (this.disposed) return
    const { handler: _h, load: _l, ...meta } = changes as Partial<ToolDefinition<any, any>>
    this.def = { ...this.def, ...meta }
    if (!this.registrar) return
    if ('input' in changes || 'outputSchema' in changes) {
      const resolved = resolveTool(this.def)
      const seq = ++this.resolveSeq
      if (resolved instanceof Promise) {
        resolved.then(
          (r) => {
            if (this.disposed || seq !== this.resolveSeq) return
            this.resolved = r
            this.pushUpdate()
          },
          (error: unknown) => this.logger.error(`[app-mcp] 工具 ${this.name} 的 input / outputSchema 定义无效`, error),
        )
        return
      }
      this.resolved = resolved
    }
    this.pushUpdate()
  }

  private pushUpdate(): void {
    if (!this.native) return // 尚在异步注册中：注册时会使用最新定义
    // @compat 旧版原生模块没有 updateWith：注解与输出 schema 保持注册时的声明
    if (this.native.updateWith) this.native.updateWith(this.spec())
    else this.native.update(this.spec())
  }

  setHandler(handler: ToolDefinition<any, any>['handler']): void {
    this.handler = handler
    this.load = undefined
    this.loading = undefined
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.owner?.children.delete(this)
    this.native?.dispose()
  }

  detach(): void {
    this.disposed = true
  }

  /**
   * 提交成功结果：带业务状态 / 摘要 / 内容标注时用 `completeWith`，否则用 `complete`（与旧版行为一致）。
   * @compat 旧版原生模块没有 `completeWith`：只提交 data 与 stateHints，并警告一次丢弃的字段。
   * @error `completeWith` 拒绝取值（如非法 audience）时以 `HANDLER_ERROR` 失败完成，不让调用挂起到超时。
   */
  private complete(
    call: NativeCall,
    json: string,
    result: NormalizedResult,
    finish: (action: () => void) => void,
    fail: (error: unknown) => void,
  ): void {
    const hints = result.stateHints ?? []
    if (!hasResultExtras(result)) {
      finish(() => call.complete(json, hints))
      return
    }
    if (!call.completeWith) {
      this.logger.warn(`[app-mcp] 原生模块版本过旧，工具 ${this.name} 结果中的 status / summary / annotations 已忽略`)
      finish(() => call.complete(json, hints))
      return
    }
    try {
      call.completeWith({
        dataJson: json,
        stateHints: hints,
        ...(result.status !== undefined && { status: result.status }),
        ...(result.stateResource !== undefined && { stateResource: result.stateResource }),
        ...(result.summary !== undefined && { summary: result.summary }),
        ...(result.annotations !== undefined && { annotations: { ...result.annotations } }),
      })
      finish(() => {})
    } catch (error) {
      if (nativeErrorCode(error) === 'ALREADY_COMPLETED') return finish(() => {})
      fail(new ToolCallError('HANDLER_ERROR', `返回的结构化结果无效：${error instanceof Error ? error.message : String(error)}`))
    }
  }

  private onCall(call: NativeCall): void {
    const controller = new AbortController()
    let settled = false
    const finish = (action: () => void) => {
      if (settled) return
      settled = true
      try {
        action()
      } catch (error) {
        // 已取消 / 已完成：原生侧拒绝重复完成，属正常情况。
        if (nativeErrorCode(error) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交工具 ${this.name} 的结果失败`, error)
        }
      }
    }
    const fail = (error: unknown) => finish(() => submitFailure(call, error, this.logger, `工具 ${this.name}`))

    try {
      call.setCancelListener((reason) => {
        const kind = reason === 'timeout' ? 'TIMEOUT' : 'CANCELLED'
        settled = true
        controller.abort(new ToolCallError(kind, `调用已取消（${reason}）`))
      })
    } catch (error) {
      this.logger.warn(`[app-mcp] 设置取消监听失败`, error)
    }

    let input: unknown
    try {
      input = call.argumentsJson ? JSON.parse(call.argumentsJson) : {}
    } catch {
      fail(new ToolCallError('INVALID_INPUT', '参数不是合法的 JSON'))
      return
    }
    const parse = this.resolved?.parse
    if (parse) {
      try {
        input = parse(input)
      } catch (error) {
        const { message, details } = describeParseError(error)
        fail(new ToolCallError('INVALID_INPUT', message, details))
        return
      }
    }

    const handler: Promise<ToolHandler<any, any>> = this.handler
      ? Promise.resolve(this.handler)
      : loadHandler(this).catch((error: unknown) => {
          throw new ToolCallError(
            'HANDLER_ERROR',
            `加载工具 ${this.name} 的 handler 失败：${error instanceof Error ? error.message : String(error)}`,
          )
        })
    const context = {
      callId: call.callId,
      signal: controller.signal,
      hold: (): HoldHandle => (call.hold ? wrapHold(call.hold()) : NOOP_HOLD),
      progress: (progress: number, total?: number, message?: string): void => {
        if (controller.signal.aborted) return
        try {
          call.reportProgress?.(progress, total ?? null, message ?? null)
        } catch {
          // @why 调用刚结束时原生层报 ALREADY_COMPLETED：进度只是提示，不影响结果。
        }
      },
    }
    handler
      .then((fn) => {
        if (controller.signal.aborted) throw controller.signal.reason
        return fn(input, context)
      })
      .then(
        (result) => {
          if (controller.signal.aborted) return
          const normalized = normalizeToolResult(result)
          let json: string
          try {
            json = stringifyJson(normalized.data)
          } catch (error) {
            fail(new ToolCallError('HANDLER_ERROR', `返回值无法序列化为 JSON：${String(error)}`))
            return
          }
          this.complete(call, json, normalized, finish, fail)
        },
        (error: unknown) => {
          if (controller.signal.aborted) return
          fail(error)
        },
      )
  }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

class ResourceEntry implements ResourceHandle, Child {
  readonly name: string
  private reader: ResourceDefinition<any>['read']
  private readonly native: NativeResource | undefined
  private disposed = false

  constructor(
    registrar: NativeRegistrar | null,
    private readonly owner: Owner | null,
    name: string,
    definition: ResourceDefinition<any>,
    private readonly logger: Logger,
  ) {
    this.name = name
    this.reader = definition.read
    if (!registrar) return
    this.native = registrar.registerResource(
      {
        name,
        description: definition.description,
        // @compat 未声明时不发送（spec/protocol.md 8.4 的 toolsHash 输入与其他 SDK 一致；Host 按 application/json 处理）
        ...(definition.mimeType !== undefined && { mimeType: definition.mimeType }),
        ...(definition.realtime && { realtime: true }),
        ...(definition.annotations !== undefined && { annotations: { ...definition.annotations } }),
      },
      (read) => this.onRead(read),
    )
    owner?.children.add(this)
  }

  notifyChanged(): void {
    if (this.disposed || !this.native) return
    try {
      this.native.notifyChanged()
    } catch (error) {
      this.logger.warn(`[app-mcp] 资源 ${this.name} 变更通知失败`, error)
    }
  }

  setReader(read: ResourceDefinition<any>['read']): void {
    this.reader = read
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.owner?.children.delete(this)
    this.native?.dispose()
  }

  detach(): void {
    this.disposed = true
  }

  private onRead(read: NativeRead): void {
    const reader = this.reader
    const submit = (action: () => void) => {
      try {
        action()
      } catch (error) {
        if (nativeErrorCode(error) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交资源 ${this.name} 的内容失败`, error)
        }
      }
    }
    Promise.resolve()
      .then(() => reader())
      .then(
        (value) => {
          let json: string
          try {
            json = stringifyJson(value)
          } catch (error) {
            submit(() => read.fail('HANDLER_ERROR', `资源内容无法序列化为 JSON：${String(error)}`))
            return
          }
          submit(() => read.complete(json))
        },
        (error: unknown) => submit(() => submitFailure(read, error, this.logger, `资源 ${this.name}`)),
      )
  }
}

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
      ...(options.overview !== undefined && { overview: options.overview }),
      ...(options.lifecycle !== undefined && { lifecycle: { ...options.lifecycle } }),
      ...(options.connectTimeoutMs !== undefined && { connectTimeoutMs: options.connectTimeoutMs }),
      ...(options.heartbeat !== undefined && { heartbeat: options.heartbeat }),
      ...(options.callDedup !== undefined && { callDedup: { ...options.callDedup } }),
    }
    this.client = new binding.NativeClient(config, (event) => this.onEvent(event))
    this.currentState = mapState(this.client.state)
    if (options.onNavigate) this.setNavigationHandler(options.onNavigate)
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

  /**
   * 执行导航回调并提交结果：正常返回 → 完成；`NAVIGATION_DENIED` 类别的 ToolCallError → 拒绝；其他异常 → 失败。
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
          const { kind, message } = toFailure(error)
          submit(() => (kind === 'NAVIGATION_DENIED' ? navigate.deny(message) : navigate.fail(message)))
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
