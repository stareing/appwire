/** 测试用假核心与假 WebSocket。 */

import { type Mock, vi } from 'vitest'
import type {
  CoreClient,
  CoreConfig,
  CoreEvent,
  CoreNavigateOutcome,
  CoreOutcome,
  CoreResourceDef,
  CoreState,
  CoreToolDef,
  CoreToolUpdate,
  SleepReason,
  WakeReason,
} from '../src/core'
import { AppMcpDriver, type DriverDeps, parseWakeTokenJs, type WebSocketLike } from '../src/driver'
import type { AppMcpOptions, Logger, Visibility } from '../src/types'

export type Call = [method: string, ...args: unknown[]]

/**
 * 假核心：记录所有调用；事件由测试通过 `emit` 注入，
 * 或通过 WebSocket 消息注入（`handleMessage` 把文本解析为 CoreEvent 数组）。
 */
export class FakeCore implements CoreClient {
  config: CoreConfig | undefined
  calls: Call[] = []
  events: CoreEvent[] = []
  timeout: number | undefined
  private nextId = 1
  private current: CoreState = { status: 'idle' }
  /** 这些名字注册时抛错（模拟核心校验失败）。 */
  rejectNames = new Set<string>()

  private rec(method: string, ...args: unknown[]): void {
    this.calls.push([method, ...args])
  }
  emit(...events: CoreEvent[]): void {
    this.events.push(...events)
  }
  methods(): string[] {
    return this.calls.map((c) => c[0])
  }
  callsOf(method: string): unknown[][] {
    return this.calls.filter((c) => c[0] === method).map((c) => c.slice(1))
  }

  state(): CoreState {
    return this.current
  }
  token(): string | undefined {
    return this.config?.token
  }
  start(now: number): void {
    this.rec('start', now)
    this.current = { status: 'connecting' }
    this.emit({ type: 'stateChanged', state: this.current }, { type: 'connect' })
  }
  stop(now: number): void {
    this.rec('stop', now)
    this.current = { status: 'stopped' }
    this.emit({ type: 'disconnect' }, { type: 'stateChanged', state: this.current })
  }
  createScope(name: string, parent?: number): number {
    this.rec('createScope', name, parent)
    return this.nextId++
  }
  disposeScope(scope: number): void {
    this.rec('disposeScope', scope)
  }
  registerTool(def: CoreToolDef): number {
    this.rec('registerTool', def)
    if (this.rejectNames.has(def.name)) throw new Error(`invalid name: ${def.name}`)
    return this.nextId++
  }
  updateTool(tool: number, update: CoreToolUpdate): void {
    this.rec('updateTool', tool, update)
  }
  unregisterTool(tool: number): void {
    this.rec('unregisterTool', tool)
  }
  registerResource(def: CoreResourceDef): number {
    this.rec('registerResource', def)
    return this.nextId++
  }
  notifyResourceChanged(resource: number, now: number): void {
    this.rec('notifyResourceChanged', resource, now)
  }
  unregisterResource(resource: number): void {
    this.rec('unregisterResource', resource)
  }
  setVisibility(visibility: Visibility, focused: boolean, now: number): void {
    this.rec('setVisibility', visibility, focused, now)
  }
  handleConnected(now: number): void {
    this.rec('handleConnected', now)
    this.current = { status: 'connected' }
    this.emit({ type: 'stateChanged', state: this.current })
  }
  handleDisconnected(now: number): void {
    this.rec('handleDisconnected', now)
  }
  handleConnectFailed(code: string, message: string, now: number): void {
    this.rec('handleConnectFailed', code, message, now)
  }
  handleDisconnectedWith(code: string, message: string, now: number): void {
    this.rec('handleDisconnectedWith', code, message, now)
  }
  handleMessage(text: string, now: number): void {
    this.rec('handleMessage', text, now)
    try {
      this.emit(...(JSON.parse(text) as CoreEvent[]))
    } catch {
      // 非脚本消息
    }
  }
  handleTimeout(now: number): void {
    this.rec('handleTimeout', now)
  }
  completeCall(callId: string, outcome: CoreOutcome, now: number): void {
    this.rec('completeCall', callId, outcome, now)
  }
  completeRead(read: number, outcome: CoreOutcome): void {
    this.rec('completeRead', read, outcome)
  }
  completeNavigate(navigate: number, outcome: CoreNavigateOutcome): void {
    this.rec('completeNavigate', navigate, outcome)
  }
  setNavigation(enabled: boolean): void {
    this.rec('setNavigation', enabled)
  }
  pollEvent(): CoreEvent | undefined {
    return this.events.shift()
  }
  pollTimeout(): number | undefined {
    return this.timeout
  }
  free(): void {
    this.rec('free')
  }

  // ---- 生命周期（简化：只记录调用并模拟 dormant / waking 转换）----
  setState(state: CoreState): void {
    this.current = state
    this.emit({ type: 'stateChanged', state })
  }
  handleWake(args: string, now: number): boolean {
    this.rec('handleWake', args, now)
    return parseWakeTokenJs(args) !== undefined
  }
  wake(now: number): boolean {
    return this.wakeWithReason('app', now)
  }
  wakeWithReason(reason: WakeReason, now: number): boolean {
    this.rec('wakeWithReason', reason, now)
    if (this.current.status === 'host-mismatch') {
      // 与核心一致：不是 app-mcp 时不自动重试，wake / connectNow 再试一次
      this.setState({ status: 'connecting' })
      this.emit({ type: 'connect' })
      return true
    }
    if (this.current.status !== 'dormant') return false
    this.setState({ status: 'waking' })
    this.emit({ type: 'connect' })
    return true
  }
  connectNow(now: number): boolean {
    this.rec('connectNow', now)
    return this.wakeWithReason('app', now)
  }
  sleep(now: number): boolean {
    return this.sleepWithReason('app', now)
  }
  sleepWithReason(reason: SleepReason, now: number): boolean {
    this.rec('sleepWithReason', reason, now)
    if (this.current.status === 'dormant' || this.current.status === 'stopped') return false
    this.emit({ type: 'disconnect' })
    this.setState({ status: 'dormant' })
    return true
  }
  hold(now: number): number {
    this.rec('hold', now)
    return this.nextId++
  }
  holdForCall(callId: string, now: number): number {
    this.rec('holdForCall', callId, now)
    if (callId.startsWith('local-')) throw new Error(`unknown call: ${callId}`)
    return this.nextId++
  }
  reportProgress(callId: string, progress: number, total: number | undefined, message: string | undefined, now: number): void {
    this.rec('reportProgress', callId, progress, total, message, now)
    if (callId.startsWith('local-')) throw new Error(`unknown call: ${callId}`)
  }
  releaseHold(hold: number, now: number): boolean {
    this.rec('releaseHold', hold, now)
    return true
  }
  toolsHash(): string {
    return '0123456789abcdef'
  }
  resumeToken(): string | undefined {
    return undefined
  }

  // ---- 诊断 ----
  /** 测试设置的连接 ID。 */
  cid: string | undefined
  connectionId(): string | undefined {
    return this.cid
  }
  reportIssue(code: string, message: string): void {
    this.rec('reportIssue', code, message)
  }
}

export class FakeSocket implements WebSocketLike {
  readyState = 0
  sent: string[] = []
  closed = false
  onopen: ((ev: unknown) => void) | null = null
  onmessage: ((ev: { data: unknown }) => void) | null = null
  onclose: ((ev: unknown) => void) | null = null
  onerror: ((ev: unknown) => void) | null = null
  constructor(readonly url: string) {}
  send(data: string): void {
    this.sent.push(data)
  }
  close(): void {
    this.closed = true
    this.readyState = 3
  }
  open(): void {
    this.readyState = 1
    this.onopen?.({})
  }
  receive(data: unknown): void {
    this.onmessage?.({ data })
  }
  /** 向驱动注入核心事件（经由假核心的 handleMessage）。 */
  script(...events: CoreEvent[]): void {
    this.receive(JSON.stringify(events))
  }
  fail(): void {
    this.readyState = 3
    this.onerror?.({})
    this.onclose?.({ code: 1006, reason: '', wasClean: false })
  }
  /** 对端关闭连接：只有 `close` 事件（CloseEvent 形状）。 */
  closeByPeer(code: number, reason = '', wasClean = true): void {
    this.readyState = 3
    this.onclose?.({ code, reason, wasClean })
  }
}

export function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

type LogFn = (message: string, ...args: unknown[]) => void

export function silentLogger(): { debug: Mock<LogFn>; warn: Mock<LogFn>; error: Mock<LogFn> } & Logger {
  return { debug: vi.fn<LogFn>(), warn: vi.fn<LogFn>(), error: vi.fn<LogFn>() }
}

/** 等待微任务与已排队的 promise 回调。 */
export async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}

export interface Harness {
  app: AppMcpDriver
  core: FakeCore
  sockets: FakeSocket[]
  logger: ReturnType<typeof silentLogger>
  clock: { now: number }
  /** 核心加载完成（手动模式下需调用）。 */
  load(): Promise<void>
  socket(): FakeSocket
}

export function setup(options: Partial<AppMcpOptions> = {}, manualLoad = false, deps: Partial<DriverDeps> = {}): Harness {
  const core = new FakeCore()
  const sockets: FakeSocket[] = []
  const logger = silentLogger()
  const clock = { now: 1000 }
  const gate = deferred<void>()
  if (!manualLoad) gate.resolve()
  const app = new AppMcpDriver(
    { appId: 'shop', appName: '示例商城', logger, ...options },
    {
      loadCore: async () => {
        await gate.promise
        return (config) => {
          core.config = config
          return core
        }
      },
      createWebSocket: (url) => {
        const s = new FakeSocket(url)
        sockets.push(s)
        return s
      },
      now: () => clock.now,
      wallNow: () => 1_700_000_000_000 + clock.now,
      ...deps,
    },
  )
  return {
    app,
    core,
    sockets,
    logger,
    clock,
    async load() {
      gate.resolve()
      await settle()
    },
    socket() {
      const s = sockets[sockets.length - 1]
      if (!s) throw new Error('没有 WebSocket')
      return s
    },
  }
}

/** 创建、加载并打开连接。 */
export async function connected(options: Partial<AppMcpOptions> = {}): Promise<Harness> {
  const h = setup(options)
  await settle()
  h.socket().open()
  return h
}
