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

class FakeCall implements NativeCall {
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
    let details: unknown
    if (detailsJson != null) {
      try {
        details = JSON.parse(detailsJson)
      } catch (error) {
        throw new NativeErrorWithCode('INVALID_JSON', String(error))
      }
    }
    this.done = true
    this.settle(details === undefined ? { ok: false, kind, message } : { ok: false, kind, message, details })
  }

  hold(): NativeHold {
    if (this.done) throw new NativeErrorWithCode('ALREADY_COMPLETED', 'call or read already completed or cancelled')
    return this.holdFactory()
  }

  /** 由 FakeNativeClient 设置。 */
  holdFactory: () => NativeHold = () => ({ release() {} })

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
  invoke(name: string, args: unknown = {}): { callId: string; result: Promise<CallOutcome> } {
    const rec = this.tools.get(name)
    if (!rec) throw new Error(`fake: no tool ${name}`)
    const callId = `c${this.nextCall++}`
    let settle!: (o: CallOutcome) => void
    const result = new Promise<CallOutcome>((resolve) => (settle = resolve))
    const call = new FakeCall(callId, name, JSON.stringify(args), (o) => {
      this.calls.delete(callId)
      settle(o)
    })
    call.holdFactory = () => this.hold()
    this.calls.set(callId, call)
    queueMicrotask(() => rec.handler(call))
    return { callId, result }
  }

  call(name: string, args: unknown = {}): Promise<CallOutcome> {
    return this.invoke(name, args).result
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
