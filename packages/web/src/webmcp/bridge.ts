/**
 * 标准侧注册表：把经 WebMCP `modelContext` 注册的工具同步到 app-mcp（以及原生 modelContext），
 * 并把 `appMcp.tool()` 注册的工具镜像到原生 modelContext。polyfill 与桥接两种模式共用。
 */

import { toJsonSchema } from '../schema'
import type { ToolHub, ToolView } from '../tool-hub'
import type { AppMcp, Logger, ToolHandle } from '../types'
import {
  APP_MCP_NAME_RE,
  STANDARD_NAME_RE,
  annotationsToRisk,
  assertTrustworthyOrigins,
  domException,
  executeOptions,
  fromStandardResult,
  standardInputSchema,
  standardToToolAnnotations,
  toolToStandardAnnotations,
  toStandardResult,
} from './convert'
import { createToolEvent } from './events'
import type {
  ModelContextOptions,
  ModelContextRegisterToolOptions,
  ModelContextTool,
  ToolExecuteOptions,
  ToolRegistration,
} from './types'

/** 原生 modelContext 的原始方法（覆盖前取得，已绑定）。 */
export interface NativeApi {
  readonly target: EventTarget
  registerTool(tool: object, options: ModelContextRegisterToolOptions): unknown
  /** 旧版原生才有。 */
  unregisterTool?: (name: string) => unknown
}

/** 经标准接口注册的工具。 */
export interface StandardEntry {
  readonly name: string
  readonly tool: ModelContextTool
  /** 注销时 abort；其 signal 交给原生 registerTool，使原生侧同步注销。 */
  readonly controller: AbortController
  handle: ToolHandle | undefined
  /** 原生注册未被拒绝（注销时才调用旧版原生的 unregisterTool）。 */
  nativeRegistered: boolean
  ready: Promise<undefined>
  rejectReady?: (reason: unknown) => void
  detachSignal?: () => void
  evict?: () => void
}

interface Mirror {
  controller: AbortController
  registered: boolean
}

function isThenable(v: unknown): v is PromiseLike<unknown> {
  return typeof v === 'object' && v !== null && typeof (v as { then?: unknown }).then === 'function'
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** 旧版 Chrome 可能以 JSON 字符串传入参数。 */
function normalizeInput(input: unknown): unknown {
  if (typeof input === 'string') {
    try {
      return JSON.parse(input)
    } catch {
      return input
    }
  }
  return input ?? {}
}

export class StandardBridge {
  readonly entries = new Map<string, StandardEntry>()
  private readonly mirrors = new Map<string, Mirror>()
  private unsubscribe: (() => void) | undefined
  private closed = false
  /** 派发 toolactivated / toolcancel 的对象（polyfill 实例或原生对象）。 */
  target: EventTarget | undefined

  constructor(
    private readonly appMcp: AppMcp,
    readonly hub: ToolHub | undefined,
    private readonly log: Logger,
    readonly mirrorOwnTools: boolean,
    /** 原生 modelContext；polyfill 模式为 undefined。 */
    private readonly native: NativeApi | undefined,
    /** 工具集合变化（polyfill 模式用于派发 toolchange）。 */
    private readonly onChange: () => void,
  ) {}

  // ---- 标准接口 -------------------------------------------------------

  registerTool(tool: ModelContextTool, options: ModelContextRegisterToolOptions = {}): ToolRegistration {
    let entry: StandardEntry | undefined
    let promise: Promise<undefined>
    try {
      entry = this.add(tool, options ?? {})
      promise = entry.ready
    } catch (e) {
      promise = Promise.reject(e)
    }
    const registered = entry
    return Object.assign(promise, {
      unregister: () => {
        if (registered) this.remove(registered)
      },
    })
  }

  unregisterTool(name: string): void {
    const entry = this.entries.get(String(name))
    if (entry) {
      this.remove(entry)
      return
    }
    // 安装前直接注册到原生的工具
    if (this.native?.unregisterTool && !this.mirrors.has(String(name))) this.native.unregisterTool(String(name))
  }

  provideContext(options: ModelContextOptions = {}): void {
    this.clearContext()
    for (const tool of options?.tools ?? []) {
      const entry = this.add(tool, {})
      entry.ready.catch((e: unknown) => this.log.warn(`[app-mcp] provideContext 注册工具 ${entry.name} 失败：${errorMessage(e)}`))
    }
  }

  clearContext(): void {
    for (const entry of [...this.entries.values()]) this.remove(entry)
  }

  // ---- 注册 / 注销 ----------------------------------------------------

  /** 同步校验并注册（与标准 registerTool 的检查顺序一致），失败时抛出。 */
  add(tool: ModelContextTool, options: ModelContextRegisterToolOptions): StandardEntry {
    if (this.closed) throw domException('WebMCP 已卸载', 'InvalidStateError')
    if (typeof tool !== 'object' || tool === null) throw new TypeError('registerTool 需要工具对象')
    const { name, description } = tool
    if (typeof name !== 'string') throw new TypeError('工具的 name 必须是字符串')
    if (typeof description !== 'string') throw new TypeError(`工具 ${name} 的 description 必须是字符串`)
    if (typeof tool.execute !== 'function') throw new TypeError(`工具 ${name} 的 execute 必须是函数`)
    if (this.entries.has(name)) throw domException(`工具 ${JSON.stringify(name)} 已注册`, 'InvalidStateError')
    if (this.ownTools().some((v) => v.name === name)) {
      throw domException(
        `工具 ${JSON.stringify(name)} 已由 appMcp.tool() 注册（appMcp 优先）`,
        'InvalidStateError',
      )
    }
    if (!STANDARD_NAME_RE.test(name)) {
      throw domException(`无效的工具名 ${JSON.stringify(name)}：应为 1–128 个 [A-Za-z0-9_.-]`, 'InvalidStateError')
    }
    if (description === '') throw domException(`工具 ${name} 的 description 不能为空`, 'InvalidStateError')
    const inputSchema = standardInputSchema(tool.inputSchema)
    if (options.signal?.aborted) throw options.signal.reason
    assertTrustworthyOrigins(options.exposedTo)

    const entry: StandardEntry = {
      name,
      tool,
      controller: new AbortController(),
      handle: undefined,
      nativeRegistered: false,
      ready: Promise.resolve(undefined),
    }
    this.entries.set(name, entry)

    // 1. 原生（桥接模式）：保持浏览器内置 AI 可用
    if (this.native) {
      let result: unknown
      try {
        result = this.native.registerTool(this.nativeTool(tool), { ...options, signal: entry.controller.signal })
      } catch (e) {
        this.entries.delete(name)
        throw e
      }
      entry.nativeRegistered = true
      entry.ready = Promise.resolve(result).then(
        () => undefined,
        (e: unknown) => {
          entry.nativeRegistered = false
          // 原生拒绝：回滚本 SDK 侧的注册
          if (this.entries.get(name) === entry) this.remove(entry)
          throw e
        },
      )
    } else {
      entry.ready = new Promise<undefined>((resolve, reject) => {
        entry.rejectReady = reject
        queueMicrotask(() => resolve(undefined))
      })
    }

    // 2. app-mcp（进而 Host）
    if (APP_MCP_NAME_RE.test(name)) {
      const standardHints = standardToToolAnnotations(tool.annotations)
      try {
        entry.handle = this.appMcp.tool(name, {
          description,
          ...(typeof tool.title === 'string' && tool.title !== '' && { title: tool.title }),
          input: inputSchema,
          risk: annotationsToRisk(tool.annotations),
          ...(standardHints && { annotations: standardHints }),
          handler: (input, ctx) => this.runForHost(entry, input, ctx.signal),
        })
        entry.evict = () => {
          this.log.warn(`[app-mcp] 工具 ${name} 与 appMcp.tool() 重名，经 modelContext 注册的版本已移除（appMcp 优先）`)
          this.remove(entry)
        }
        this.hub?.yieldable.set(name, entry.evict)
      } catch (e) {
        this.log.warn(`[app-mcp] 工具 ${name} 无法注册到 app-mcp：${errorMessage(e)}`)
      }
    } else {
      this.log.warn(`[app-mcp] 工具名 ${name} 超过 64 个字符，不会暴露给 Host`)
    }

    // 3. 注册时的 signal：abort 即注销（标准语义）
    const signal = options.signal
    if (signal) {
      const onAbort = (): void => {
        entry.controller.abort(signal.reason)
        this.remove(entry, signal.reason)
      }
      signal.addEventListener('abort', onAbort, { once: true })
      entry.detachSignal = () => signal.removeEventListener('abort', onAbort)
    }

    this.onChange()
    return entry
  }

  /** 注销标准侧工具（两边）。`reason` 存在时表示由 signal 触发。 */
  remove(entry: StandardEntry, reason?: unknown): void {
    if (this.entries.get(entry.name) !== entry) return
    this.entries.delete(entry.name)
    entry.detachSignal?.()
    if (entry.evict && this.hub?.yieldable.get(entry.name) === entry.evict) this.hub.yieldable.delete(entry.name)
    if (!entry.controller.signal.aborted) entry.controller.abort(reason ?? domException('工具已注销', 'AbortError'))
    if (entry.nativeRegistered && this.native?.unregisterTool) {
      entry.nativeRegistered = false
      try {
        this.native.unregisterTool(entry.name)
      } catch {
        // 已随 signal 注销
      }
    }
    const handle = entry.handle
    entry.handle = undefined
    handle?.dispose()
    if (reason !== undefined) entry.rejectReady?.(reason)
    this.onChange()
  }

  /** 交给原生的工具对象：`execute` 的第二个参数补上 `requestUserInteraction`（兼容旧代码）。 */
  private nativeTool(tool: ModelContextTool): ModelContextTool {
    return {
      ...tool,
      execute: (input: unknown, options?: Partial<ToolExecuteOptions>) =>
        tool.execute.call(tool, input, executeOptions(options?.signal ?? new AbortController().signal, options)),
    }
  }

  /** Host 调用标准侧工具。 */
  private async runForHost(entry: StandardEntry, input: unknown, signal: AbortSignal): Promise<{ data: unknown }> {
    this.dispatch('toolactivated', entry.name)
    const onAbort = (): void => this.dispatch('toolcancel', entry.name)
    signal.addEventListener('abort', onAbort, { once: true })
    try {
      const result = await entry.tool.execute.call(entry.tool, input ?? {}, executeOptions(signal))
      // 显式包一层 data，避免 `{ data, stateHints }` 形状的返回值被误拆
      return { data: fromStandardResult(result) }
    } finally {
      signal.removeEventListener('abort', onAbort)
    }
  }

  dispatch(type: 'toolactivated' | 'toolcancel', toolName: string): void {
    const target = this.target
    if (!target) return
    try {
      target.dispatchEvent(createToolEvent(type, toolName))
    } catch (e) {
      this.log.error(`[app-mcp] 派发 ${type} 事件失败`, e)
    }
  }

  // ---- appMcp.tool() 的工具 ------------------------------------------

  /** 用 `appMcp.tool()` 注册的工具（不含经标准接口注册的）。 */
  ownTools(): ToolView[] {
    return this.hub?.list().filter((v) => !this.entries.has(v.name)) ?? []
  }

  /** 开始观察 `appMcp.tool()` 的注册；桥接模式下镜像到原生。 */
  startMirroring(): void {
    const hub = this.hub
    if (!this.mirrorOwnTools || !hub) return
    for (const view of this.ownTools()) if (view.definition.enabled !== false) this.mirrorAdd(view)
    this.unsubscribe = hub.subscribe((ev) => {
      const name = ev.type === 'unregister' ? ev.name : ev.tool.name
      if (this.entries.has(name)) return
      this.mirrorRemove(name)
      if (ev.type !== 'unregister' && ev.tool.definition.enabled !== false) this.mirrorAdd(ev.tool)
      this.onChange()
    })
  }

  private mirrorAdd(view: ToolView): void {
    const native = this.native
    if (!native) return
    const mirror: Mirror = { controller: new AbortController(), registered: false }
    this.mirrors.set(view.name, mirror)
    const def = view.definition
    const fail = (e: unknown): void => {
      if (mirror.controller.signal.aborted) return
      mirror.registered = false
      this.log.warn(`[app-mcp] 工具 ${view.name} 无法注册到浏览器原生 modelContext：${errorMessage(e)}`)
    }
    const register = (inputSchema: object): void => {
      if (mirror.controller.signal.aborted) return
      const tool: ModelContextTool = {
        name: view.name,
        description: def.description,
        inputSchema,
        annotations: toolToStandardAnnotations(def.risk, def.annotations),
        execute: async (input: unknown, options?: Partial<ToolExecuteOptions>) =>
          toStandardResult(await view.call(normalizeInput(input), options?.signal ?? new AbortController().signal)),
      }
      if (def.title) tool.title = def.title
      try {
        mirror.registered = true
        const r = native.registerTool(tool, { signal: mirror.controller.signal })
        if (isThenable(r)) r.then(undefined, fail)
      } catch (e) {
        fail(e)
      }
    }
    let schema: ReturnType<typeof toJsonSchema>
    try {
      schema = toJsonSchema(def.input)
    } catch (e) {
      fail(e)
      return
    }
    if (isThenable(schema)) schema.then(register, fail)
    else register(schema)
  }

  private mirrorRemove(name: string): void {
    const mirror = this.mirrors.get(name)
    if (!mirror) return
    this.mirrors.delete(name)
    mirror.controller.abort(domException('工具已注销', 'AbortError'))
    if (mirror.registered && this.native?.unregisterTool) {
      try {
        this.native.unregisterTool(name)
      } catch {
        // 已随 signal 注销
      }
    }
  }

  // ---- 卸载 -----------------------------------------------------------

  /**
   * 停止同步：注销镜像到原生的工具、从 app-mcp 注销标准侧工具。
   * 桥接模式下页面注册到原生的工具保留（浏览器内置 AI 仍可用），其注册 signal 仍然有效。
   */
  close(): void {
    if (this.closed) return
    this.closed = true
    this.unsubscribe?.()
    this.unsubscribe = undefined
    for (const name of [...this.mirrors.keys()]) this.mirrorRemove(name)
    for (const entry of [...this.entries.values()]) {
      if (entry.evict && this.hub?.yieldable.get(entry.name) === entry.evict) this.hub.yieldable.delete(entry.name)
      const handle = entry.handle
      entry.handle = undefined
      handle?.dispose()
      if (!this.native) entry.detachSignal?.()
    }
    this.entries.clear()
  }
}
