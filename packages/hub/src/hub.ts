/**
 * `Hub`：嵌入式 Hub 的 TS 封装（spec/hub-api.md 第 3 节）。
 *
 * 原生层以 JSON 文本进出，这里负责序列化 / 解析、错误转换（{@link HubError}）、
 * 事件多播、审批 / 配对回调的 Promise 规整，以及进程保活。
 */

import { fromNativeError, HubError } from './errors.js'
import { loadNativeBinding, type HubBinding, type NativeHub } from './native.js'
import type {
  ErrorKind,
  AppInfo,
  AppOverviewInfo,
  ApprovalHandler,
  ApprovalRequest,
  CallOutcome,
  CallRequest,
  ExportedTools,
  HubConfig,
  HubEvent,
  HubResource,
  HubTool,
  PairingHandler,
  PairingRequest,
  ResourceContent,
  ToolCallInput,
  ToolFilter,
  ToolFormat,
  ToolResultMessage,
  Waker,
  WakeRequest,
} from './types.js'

export interface HubStartOptions extends HubConfig {
  /**
   * 运行期间保持 Node 进程存活（事件循环上保留一个 ref 的定时器），`shutdown()` 后释放。默认 true。
   * 原生回调本身不会阻止进程退出。
   */
  keepAlive?: boolean
  /** 高级：注入原生模块（测试或自定义加载路径）。缺省按平台加载包内的 `.node` 文件。 */
  binding?: HubBinding
  /** 事件监听器抛错时的输出，默认 `console.error`。 */
  onListenerError?: (error: unknown) => void
}

async function wrap<T>(p: Promise<T>): Promise<T> {
  try {
    return await p
  } catch (e) {
    throw fromNativeError(e)
  }
}

function wrapSync<T>(f: () => T): T {
  try {
    return f()
  } catch (e) {
    throw fromNativeError(e)
  }
}

/** 把用户回调规整为“总是 resolve 为布尔值”的 Promise：false / 抛错 / reject / 非 true 都视为拒绝。 */
function decision<R>(handler: (req: R) => boolean | Promise<boolean>): (json: string) => Promise<boolean> {
  return async (json: string) => {
    try {
      const req = JSON.parse(json) as R
      return (await handler(req)) === true
    } catch {
      return false
    }
  }
}

const ERROR_KINDS: ReadonlySet<string> = new Set<ErrorKind>([
  'TOOL_NOT_FOUND', 'TOOL_DISABLED', 'INVALID_INPUT', 'USER_REJECTED', 'TIMEOUT', 'HANDLER_ERROR',
  'CANCELLED', 'APP_DISCONNECTED', 'APP_NOT_INSTALLED', 'LAUNCH_FAILED', 'APP_NOT_RESPONDING',
  'INSTANCE_FROZEN', 'RESOURCE_NOT_FOUND', 'UNAUTHORIZED', 'UNSUPPORTED_PROTOCOL',
])

/** 把用户的 Waker 规整为“总是 resolve”的 Promise：null = 成功；失败为 `{"kind","message"}` JSON。 */
function wakerAdapter(waker: Waker): (json: string) => Promise<string | null> {
  return async (json: string) => {
    try {
      await waker(JSON.parse(json) as WakeRequest)
      return null
    } catch (e) {
      const k = (e as { kind?: unknown } | null)?.kind
      const kind = typeof k === 'string' && ERROR_KINDS.has(k) ? k : 'LAUNCH_FAILED'
      const message = e instanceof Error ? e.message : String(e)
      return JSON.stringify({ kind, message })
    }
  }
}

function filterJson(filter?: ToolFilter): string | null {
  return filter ? JSON.stringify(filter) : null
}

export class Hub {
  readonly #native: NativeHub
  readonly #listeners = new Set<(event: HubEvent) => void>()
  readonly #onListenerError: (error: unknown) => void
  #keepAlive: ReturnType<typeof setInterval> | undefined

  private constructor(native: NativeHub, options: HubStartOptions) {
    this.#native = native
    this.#onListenerError = options.onListenerError ?? ((e) => console.error('[app-mcp-hub] 事件监听器出错：', e))
    if (options.keepAlive !== false) this.#keepAlive = setInterval(() => {}, 1 << 30)
  }

  /** 启动 Hub（App 连接服务、上游等）。 */
  static async start(options: HubStartOptions = {}): Promise<Hub> {
    const { keepAlive: _k, binding, onListenerError: _o, ...config } = options
    const b = binding ?? loadNativeBinding()
    const native = await wrap(b.Hub.start(JSON.stringify(config)))
    return new Hub(native, options)
  }

  /** App 连接服务实际监听的地址（`host:port`）；未开启时为 null。 */
  get wsAddr(): string | null {
    return this.#native.wsAddr
  }

  /** App 端 SDK 的 `hostUrl`（`ws://host:port`）；未开启时为 null。 */
  get wsUrl(): string | null {
    const a = this.#native.wsAddr
    return a ? `ws://${a}` : null
  }

  /** 本地 IPC 连接服务的端点（`unix:…` / `pipe:…`，可直接作为原生 App 端 SDK 的 `hostUrl`）；未开启时为 null。 */
  get ipcEndpoint(): string | null {
    return this.#native.ipcEndpoint
  }

  get isShutdown(): boolean {
    return this.#native.isShutdown
  }

  /** 停止：关闭 App 连接与后台任务，释放回调与保活定时器。可重复调用。 */
  async shutdown(): Promise<void> {
    if (this.#keepAlive !== undefined) {
      clearInterval(this.#keepAlive)
      this.#keepAlive = undefined
    }
    this.#listeners.clear()
    await wrap(this.#native.shutdown())
  }

  // ---- 查询 ----

  /** 所有已知 App（含静态清单、上游）。 */
  apps(): AppInfo[] {
    return wrapSync(() => JSON.parse(this.#native.apps()) as AppInfo[])
  }

  tools(filter?: ToolFilter): HubTool[] {
    return wrapSync(() => JSON.parse(this.#native.tools(filterJson(filter))) as HubTool[])
  }

  resources(): HubResource[] {
    return wrapSync(() => JSON.parse(this.#native.resources()) as HubResource[])
  }

  overview(appId: string): AppOverviewInfo | null {
    return wrapSync(() => {
      const j = this.#native.overview(appId)
      return j == null ? null : (JSON.parse(j) as AppOverviewInfo)
    })
  }

  // ---- 操作 ----

  /**
   * 调用工具。工具层面的失败（用户拒绝、超时、App 报错…）在 `outcome.result.error` 中；
   * 只有名称无法解析（appId 未知 / 不含 `.`）时抛 {@link HubError}。
   */
  async callTool(req: CallRequest): Promise<CallOutcome> {
    const json = await wrap(this.#native.callTool(JSON.stringify(req)))
    return JSON.parse(json) as CallOutcome
  }

  cancelCall(callId: string): void {
    wrapSync(() => this.#native.cancelCall(callId))
  }

  async readResource(uri: string): Promise<ResourceContent> {
    const json = await wrap(this.#native.readResource(uri))
    return JSON.parse(json) as ResourceContent
  }

  /** 订阅资源变化（`resourceUpdated` 事件）。 */
  subscribe(uri: string): void {
    wrapSync(() => this.#native.subscribe(uri))
  }

  unsubscribe(uri: string): void {
    wrapSync(() => this.#native.unsubscribe(uri))
  }

  /** 全局选择 App 的实例；`instanceId` 省略 / null 清除选择。 */
  selectInstance(appId: string, instanceId?: string | null): void {
    wrapSync(() => this.#native.selectInstance(appId, instanceId ?? null))
  }

  /** 开始新对话时重置会话状态（总览首次附带、`apps.select`）。 */
  resetSession(session?: string | null): void {
    wrapSync(() => this.#native.resetSession(session ?? null))
  }

  // ---- 工具格式导出与分派 ----

  /** 按 LLM 厂商格式导出工具定义（名称已编码为 `[a-zA-Z0-9_-]{1,64}`）。 */
  exportTools<F extends ToolFormat>(format: F, filter?: ToolFilter): ExportedTools[F] {
    return wrapSync(() => JSON.parse(this.#native.exportTools(format, filterJson(filter))) as ExportedTools[F])
  }

  /**
   * 执行模型返回的一个工具调用，返回该格式的“工具结果”消息（直接放回对话）。
   * 工具失败以该格式的错误结果返回，不抛错。`session` 为厂商会话 ID（总览首次附带按会话计算）。
   */
  async dispatch<F extends ToolFormat>(
    format: F,
    toolCall: ToolCallInput[F],
    session?: string | null,
  ): Promise<ToolResultMessage[F]> {
    const json = await wrap(this.#native.dispatch(format, JSON.stringify(toolCall), session ?? null))
    return JSON.parse(json) as ToolResultMessage[F]
  }

  /** 同时以 Streamable HTTP MCP 对外提供（`http://<addr>/mcp`），返回实际地址。 */
  async serveHttp(addr: string, allowRemote = false): Promise<string> {
    return wrap(this.#native.serveHttp(addr, allowRemote))
  }

  // ---- 事件 ----

  /** 订阅 Hub 事件，返回取消函数。监听器在 Node 事件循环上执行。 */
  onEvent(listener: (event: HubEvent) => void): () => void {
    if (this.#listeners.size === 0) {
      wrapSync(() =>
        this.#native.onEvent((json) => {
          let event: HubEvent
          try {
            event = JSON.parse(json) as HubEvent
          } catch (e) {
            this.#onListenerError(e)
            return
          }
          for (const l of [...this.#listeners]) {
            try {
              l(event)
            } catch (e) {
              this.#onListenerError(e)
            }
          }
        }),
      )
    }
    this.#listeners.add(listener)
    return () => {
      if (!this.#listeners.delete(listener) || this.#listeners.size > 0) return
      if (!this.#native.isShutdown) this.#native.onEvent(null)
    }
  }

  /** 等待第一个满足条件的事件（便利方法；超时 reject）。 */
  waitForEvent(predicate: (event: HubEvent) => boolean, timeoutMs = 10_000): Promise<HubEvent> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        off()
        reject(new HubError('TIMEOUT', `等待 Hub 事件超时（${timeoutMs} ms）`))
      }, timeoutMs)
      const off = this.onEvent((e) => {
        if (!predicate(e)) return
        clearTimeout(timer)
        off()
        resolve(e)
      })
    })
  }

  // ---- 策略回调 ----

  /**
   * 设置审批回调（配合 `approval.requireAtOrAbove`）。返回 true 同意；
   * false、抛错、reject、超时均视为拒绝，调用以 `USER_REJECTED` 结束。
   */
  setApprovalHandler(handler: ApprovalHandler): void {
    wrapSync(() => this.#native.setApprovalHandler(decision<ApprovalRequest>(handler)))
  }

  /**
   * 设置自定义唤醒（spec/hub-api.md 3.5），替换默认的系统唤醒实现；`null` 恢复默认。
   * 调用休眠实例的工具（或未运行而清单声明了 wake 的 App）时，Hub 生成一次性令牌并调用 `waker`：
   * resolve 表示已发出激活（Hub 随后等待 App 回连）；抛错 / reject → `LAUNCH_FAILED`
   * （抛出 `kind` 为协议错误类别的错误，如 `new HubError('APP_NOT_INSTALLED', …)`，则用该类别）。
   */
  setWaker(waker: Waker | null): void {
    wrapSync(() => this.#native.setWaker(waker ? wakerAdapter(waker) : null))
  }

  /** 设置配对回调：未知 App（无清单或 Origin 不在白名单）首次连接时询问。语义同审批回调。 */
  setPairingHandler(handler: PairingHandler): void {
    wrapSync(() => this.#native.setPairingHandler(decision<PairingRequest>(handler)))
  }
}
