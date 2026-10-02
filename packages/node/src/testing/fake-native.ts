/**
 * 测试用的假原生模块：行为模拟 bindings/node 的 NativeClient（同步注册、异步回调、完成只能一次）。
 * 不属于公开 API，只在测试中使用（@app-mcp/electron 的测试也复用它）。
 */

import type {
  NativeBinding,
  NativeCall,
  NativeCallResult,
  NativeCancelReason,
  NativeClient,
  NativeClientConfig,
  NativeClientEvent,
  NativeHold,
  NativeNavigate,
  NativeRead,
  NativeRegistrar,
  NativeResource,
  NativeResourceSpec,
  NativeScope,
  NativeStateInfo,
  NativeTool,
  NativeToolSpec,
} from '../native.js'

export type CallOutcome =
  | ({ ok: true; data: unknown; stateHints: string[] } & Omit<NativeCallResult, 'dataJson' | 'stateHints'>)
  | { ok: false; kind: string; message: string; details?: unknown }

class NativeErrorWithCode extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message)
  }
}

interface ToolRecord {
  spec: NativeToolSpec
  handler: (call: NativeCall) => void
  scope: FakeScope | null
}

interface ResourceRecord {
  spec: NativeResourceSpec
  reader: (read: NativeRead) => void
  scope: FakeScope | null
  changes: number
}

/** {@link FakeNativeClient.progressReports} 的一条。 */
/** {@link FakeNativeClient.navigate} 的结果。 */
export type NavigateOutcome =
  | { ok: true }
  | { ok: false; kind: 'fail' | 'deny' | 'unsupported'; message: string }
  | { ok: false; kind: 'userAction'; message: string; reason: string | null; uri: string | null }

export interface FakeProgress {
  callId: string
  progress: number
  total: number | null
  message: string | null
}

class FakeCall implements NativeCall {
  /** 与原生模块一致：没有幂等键时为 `null`。 */
  idempotencyKey: string | null = null
  private done = false
  private cancelled: NativeCancelReason | null = null
  private cancelListener: ((reason: NativeCancelReason) => void) | null = null

  constructor(
    readonly callId: string,
    readonly toolName: string,
    readonly argumentsJson: string,
    private readonly settle: (outcome: CallOutcome) => void,
  ) {}

  isCancelled(): boolean {
    return this.cancelled !== null
  }

  setCancelListener(listener: (reason: NativeCancelReason) => void): void {
    this.cancelListener = listener
    const reason = this.cancelled
    if (reason) queueMicrotask(() => listener(reason))
  }

  complete(dataJson?: string | null, stateHints?: string[]): void {
    this.succeed(dataJson, stateHints, {})
  }

  private succeed(
    dataJson: string | null | undefined,
    stateHints: string[] | undefined,
    extras: Omit<NativeCallResult, 'dataJson' | 'stateHints'>,
  ): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    let data: unknown = null
    if (dataJson != null) {
      try {
        data = JSON.parse(dataJson)
      } catch (error) {
        throw new NativeErrorWithCode('INVALID_JSON', String(error))
      }
    }
    this.done = true
    this.settle({ ok: true, data, stateHints: stateHints ?? [], ...extras })
  }

  /** 与原生绑定一致：status / audience 取值不合法时抛出 `INVALID_ARG`（调用仍未完成）。 */
  completeWith(result: NativeCallResult): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    const { dataJson, stateHints, ...extras } = result
    if (extras.status !== undefined && !['done', 'pending', 'partial', 'noop'].includes(extras.status)) {
      throw new NativeErrorWithCode('INVALID_ARG', `未知的 status：${JSON.stringify(extras.status)}`)
    }
    for (const role of extras.annotations?.audience ?? []) {
      if (role !== 'user' && role !== 'assistant') {
        throw new NativeErrorWithCode('INVALID_ARG', `未知的 audience：${JSON.stringify(role)}`)
      }
    }
    this.succeed(dataJson, stateHints, extras)
  }

  fail(kind: string, message: string): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    this.done = true
    this.settle({ ok: false, kind, message })
  }

  failWithDetails(kind: string, message: string, detailsJson?: string | null): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    const details = parseDetails(detailsJson)
    this.done = true
    this.settle(details === undefined ? { ok: false, kind, message } : { ok: false, kind, message, details })
  }

  hold(): NativeHold {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    return this.holdFactory()
  }

  /** 由 FakeNativeClient 设置。 */
  holdFactory: () => NativeHold = () => ({ release() {} })

  /** 由 FakeNativeClient 设置：收到的进度。 */
  progressSink: (p: FakeProgress) => void = () => {}

  /** 与原生绑定一致：调用已结束时抛出 `ALREADY_COMPLETED`。 */
  reportProgress(progress: number, total?: number | null, message?: string | null): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    this.progressSink({ callId: this.callId, progress, total: total ?? null, message: message ?? null })
  }

  cancel(reason: NativeCancelReason): void {
    if (this.done) return
    this.done = true
    this.cancelled = reason
    this.settle({ ok: false, kind: reason === 'timeout' ? 'TIMEOUT' : 'CANCELLED', message: reason })
    const listener = this.cancelListener
    // 真实实现经 ThreadsafeFunction 异步投递。
    if (listener) queueMicrotask(() => listener(reason))
  }
}

class FakeRead implements NativeRead {
  private done = false
  constructor(
    readonly resourceName: string,
    private readonly settle: (outcome: CallOutcome) => void,
  ) {}
  complete(contentsJson: string): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'already completed')
    this.done = true
    this.settle({ ok: true, data: JSON.parse(contentsJson), stateHints: [] })
  }
  fail(kind: string, message: string): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'already completed')
    this.done = true
    this.settle({ ok: false, kind, message })
  }
  failWithDetails(kind: string, message: string, detailsJson?: string | null): void {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'already completed')
    const details = parseDetails(detailsJson)
    this.done = true
    this.settle(details === undefined ? { ok: false, kind, message } : { ok: false, kind, message, details })
  }
}

/** 与原生绑定一致：非法 JSON 抛出 `INVALID_JSON`（调用 / 读取仍未完成）；`null` / 省略为无详情。 */
function parseDetails(detailsJson: string | null | undefined): unknown {
  if (detailsJson == null) return undefined
  try {
    return JSON.parse(detailsJson)
  } catch (error) {
    throw new NativeErrorWithCode('INVALID_JSON', String(error))
  }
}

abstract class FakeRegistrarBase implements NativeRegistrar {
  protected abstract get client(): FakeNativeClient
  protected abstract get scopeRef(): FakeScope | null

  registerTool(spec: NativeToolSpec, handler: (call: NativeCall) => void): NativeTool {
    this.client.checkUsable()
    if (!/^[a-zA-Z0-9_.-]{1,64}$/.test(spec.name)) throw new NativeErrorWithCode('INVALID_NAME', spec.name)
    if (this.client.tools.has(spec.name) || this.client.resources.has(spec.name)) {
      throw new NativeErrorWithCode('DUPLICATE_NAME', `a tool or resource named "${spec.name}" is already registered`)
    }
    if (spec.inputSchemaJson !== undefined) {
      const schema = JSON.parse(spec.inputSchemaJson) as { type?: unknown }
      if (schema.type !== 'object') throw new NativeErrorWithCode('INVALID_SCHEMA', 'type must be object')
    }
    const name = spec.name
    this.client.tools.set(name, { spec: { ...spec }, handler, scope: this.scopeRef })
    const client = this.client
    return {
      name,
      update(next) {
        const rec = client.tools.get(name)
        if (!rec) throw new NativeErrorWithCode('DISPOSED', 'disposed')
        // 与原生绑定一致：update 保留已声明的注解与输出 schema
        const { annotations: _a, outputSchemaJson: _o, ...rest } = next
        const kept = {
          ...(rec.spec.annotations !== undefined && { annotations: rec.spec.annotations }),
          ...(rec.spec.outputSchemaJson !== undefined && { outputSchemaJson: rec.spec.outputSchemaJson }),
        }
        rec.spec = { ...rest, ...kept, name }
      },
      updateWith(next) {
        const rec = client.tools.get(name)
        if (!rec) throw new NativeErrorWithCode('DISPOSED', 'disposed')
        rec.spec = { ...next, name }
      },
      setEnabled(enabled) {
        const rec = client.tools.get(name)
        if (!rec) throw new NativeErrorWithCode('DISPOSED', 'disposed')
        rec.spec.enabled = enabled
      },
      dispose() {
        client.tools.delete(name)
      },
    }
  }

  registerResource(spec: NativeResourceSpec, reader: (read: NativeRead) => void): NativeResource {
    this.client.checkUsable()
    if (this.client.tools.has(spec.name) || this.client.resources.has(spec.name)) {
      throw new NativeErrorWithCode('DUPLICATE_NAME', spec.name)
    }
    const name = spec.name
    const rec: ResourceRecord = { spec: { ...spec }, reader, scope: this.scopeRef, changes: 0 }
    this.client.resources.set(name, rec)
    const client = this.client
    return {
      name,
      notifyChanged() {
        rec.changes++
      },
      dispose() {
        if (client.resources.get(name) === rec) client.resources.delete(name)
      },
    }
  }

  createScope(name: string): NativeScope {
    this.client.checkUsable()
    return new FakeScope(this.client, this.scopeRef, name)
  }
}

export class FakeScope extends FakeRegistrarBase implements NativeScope {
  disposed = false
  constructor(
    private readonly owner: FakeNativeClient,
    readonly parent: FakeScope | null,
    readonly name: string,
  ) {
    super()
  }

  protected get client(): FakeNativeClient {
    return this.owner
  }

  protected get scopeRef(): FakeScope | null {
    return this
  }

  isWithin(other: FakeScope): boolean {
    for (let s: FakeScope | null = this; s; s = s.parent) if (s === other) return true
    return false
  }

  dispose(): void {
    this.disposed = true
    for (const [name, rec] of this.client.tools) if (rec.scope?.isWithin(this)) this.client.tools.delete(name)
    for (const [name, rec] of this.client.resources) if (rec.scope?.isWithin(this)) this.client.resources.delete(name)
  }
}

export class FakeNativeClient extends FakeRegistrarBase implements NativeClient {
  static last: FakeNativeClient | undefined
  readonly tools = new Map<string, ToolRecord>()
  readonly resources = new Map<string, ResourceRecord>()
  readonly instanceId: string
  state: NativeStateInfo = { status: 'idle' }
  token: string | null
  started = 0
  stopped = false
  visibility: [string, boolean] | undefined
  /** 生命周期调用记录（测试断言用）。 */
  readonly lifecycleCalls: string[] = []
  /** 当前未释放的持有数。 */
  activeHolds = 0
  /** `handleWake` 识别的参数前缀。 */
  wakePrefix = 'app-mcp-wake:'
  private nextCall = 1
  private readonly calls = new Map<string, FakeCall>()
  /** handler 经 `ctx.progress()` 报告的进度（按到达顺序）。 */
  readonly progressReports: FakeProgress[] = []

  constructor(
    readonly config: NativeClientConfig,
    readonly listener?: (event: NativeClientEvent) => void,
  ) {
    super()
    this.instanceId = config.instanceId ?? 'fake-instance'
    this.token = config.token ?? null
    FakeNativeClient.last = this
  }

  protected get client(): FakeNativeClient {
    return this
  }

  protected get scopeRef(): FakeScope | null {
    return null
  }

  checkUsable(): void {
    if (this.stopped) throw new NativeErrorWithCode('STOPPED', 'client stopped')
  }

  start(): void {
    this.started++
  }

  stop(): void {
    this.stopped = true
    for (const call of this.calls.values()) call.cancel('stopped')
  }

  setVisibility(visibility: string, focused: boolean): void {
    this.visibility = [visibility, focused]
  }

  /** 当前导航回调（`setNavigationHandler`）；`undefined` = 未设置 / 已清除。 */
  navigationHandler: ((navigate: NativeNavigate) => void) | undefined

  setNavigationHandler(handler: ((navigate: NativeNavigate) => void) | null): void {
    this.navigationHandler = handler ?? undefined
  }

  /** 不可见时导航是否仍交给回调（`setNavigateInBackground`）；缺省同桌面原生运行时为 true。 */
  navigateInBackground = true

  setNavigateInBackground(enabled: boolean): void {
    this.navigateInBackground = enabled
  }

  /**
   * 模拟 Host 的 `app/navigate`：没有回调时同真实原生层以 `unsupported` 失败；不可见（`hidden` / `frozen`）且
   * `navigateInBackground` 为 false 时同核心立即以 `userAction`（`reason: "foreground"`）回复、不调用回调。
   * @output `{ ok: true }` / `{ ok: false, kind: 'fail' | 'deny' | 'unsupported', message }` /
   *   `{ ok: false, kind: 'userAction', message, reason, uri }`。
   */
  navigate(page: string, params?: unknown): Promise<NavigateOutcome> {
    return new Promise((resolve) => {
      const handler = this.navigationHandler
      if (!handler) {
        resolve({ ok: false, kind: 'unsupported', message: `App 不支持由 Agent 导航（页面「${page}」）。` })
        return
      }
      const hidden = this.visibility !== undefined && this.visibility[0] !== 'visible'
      if (hidden && !this.navigateInBackground) {
        resolve({ ok: false, kind: 'userAction', message: `App 在后台，无法切换到页面「${page}」`, reason: 'foreground', uri: null })
        return
      }
      let done = false
      const finish = (outcome: NavigateOutcome) => {
        if (done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'navigation already completed')
        done = true
        resolve(outcome)
      }
      handler({
        page,
        paramsJson: params === undefined ? null : JSON.stringify(params),
        complete: () => finish({ ok: true }),
        fail: (message) => finish({ ok: false, kind: 'fail', message }),
        deny: (message) => finish({ ok: false, kind: 'deny', message }),
        failUserAction: (message, reason, uri) =>
          finish({ ok: false, kind: 'userAction', message, reason: reason ?? null, uri: uri ?? null }),
      })
    })
  }

  handleWake(args: string): boolean {
    this.lifecycleCalls.push(`handleWake:${args}`)
    return args.startsWith(this.wakePrefix) || args.includes('://app-mcp/wake?token=')
  }

  wake(reason?: string | null): boolean {
    this.lifecycleCalls.push(`wake:${reason ?? 'app'}`)
    return true
  }

  connectNow(): boolean {
    this.lifecycleCalls.push('connectNow')
    return true
  }

  sleep(reason?: string | null): boolean {
    this.lifecycleCalls.push(`sleep:${reason ?? 'app'}`)
    return true
  }

  hold(): NativeHold {
    this.activeHolds++
    let released = false
    return {
      release: () => {
        if (released) return
        released = true
        this.activeHolds--
      },
    }
  }

  toolsHash(): string {
    return `fake-hash-${this.tools.size}-${this.resources.size}`
  }

  // ---- 测试驱动 ----------------------------------------------------------

  emit(event: NativeClientEvent): void {
    if (event.type === 'state') this.state = event.state
    if (event.type === 'paired') this.token = event.token
    // 真实实现经 ThreadsafeFunction 异步投递到事件循环。
    this.listener?.(event)
  }

  /** 模拟 Host 调用工具；返回调用 ID 与结果 Promise。 */
  /** `options.idempotencyKey`：模拟 Host 转来的 Agent 幂等键（spec/protocol.md 3.3）。 */
  invoke(
    name: string,
    args: unknown = {},
    options: { idempotencyKey?: string } = {},
  ): { callId: string; result: Promise<CallOutcome> } {
    const rec = this.tools.get(name)
    if (!rec) throw new Error(`fake: no tool ${name}`)
    const callId = `c${this.nextCall++}`
    let settle!: (o: CallOutcome) => void
    const result = new Promise<CallOutcome>((resolve) => (settle = resolve))
    const call = new FakeCall(callId, name, JSON.stringify(args), (o) => {
      this.calls.delete(callId)
      settle(o)
    })
    call.idempotencyKey = options.idempotencyKey ?? null
    call.holdFactory = () => this.hold()
    call.progressSink = (p) => this.progressReports.push(p)
    this.calls.set(callId, call)
    queueMicrotask(() => rec.handler(call))
    return { callId, result }
  }

  call(name: string, args: unknown = {}, options: { idempotencyKey?: string } = {}): Promise<CallOutcome> {
    return this.invoke(name, args, options).result
  }

  cancel(callId: string, reason: NativeCancelReason = 'requested'): void {
    this.calls.get(callId)?.cancel(reason)
  }

  read(name: string): Promise<CallOutcome> {
    const rec = this.resources.get(name)
    if (!rec) throw new Error(`fake: no resource ${name}`)
    return new Promise((resolve) => {
      queueMicrotask(() => rec.reader(new FakeRead(name, resolve)))
    })
  }

  resourceChanges(name: string): number {
    return this.resources.get(name)?.changes ?? -1
  }
}

export const fakeBinding: NativeBinding = { NativeClient: FakeNativeClient }
