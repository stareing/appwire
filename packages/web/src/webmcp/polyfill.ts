/**
 * 没有原生 WebMCP 时安装的 `modelContext` 实现（标准接口 + 旧接口）。
 * 工具注册转交 {@link StandardBridge}，进而进入 app-mcp。
 */

import { toJsonSchema } from '../schema'
import type { StandardBridge } from './bridge'
import { assertTrustworthyOrigins, domException, executeOptions, riskToAnnotations, toStandardResult } from './convert'
import type {
  ModelContext,
  ModelContextExecuteToolOptions,
  ModelContextGetToolOptions,
  ModelContextOptions,
  ModelContextRegisterToolOptions,
  ModelContextTool,
  RegisteredTool,
  ToolRegistration,
} from './types'

type Handler = ((event: Event) => unknown) | null

function currentOrigin(): string {
  try {
    return typeof location !== 'undefined' ? location.origin : 'null'
  } catch {
    return 'null'
  }
}

function currentWindow(): Window {
  return (typeof window !== 'undefined' ? window : globalThis) as unknown as Window
}

export class ModelContextPolyfill extends EventTarget implements ModelContext {
  readonly #bridge: StandardBridge
  readonly #handlers = new Map<string, { handler: Handler; listener: (e: Event) => void }>()

  constructor(bridge: StandardBridge) {
    super()
    this.#bridge = bridge
  }

  registerTool(tool: ModelContextTool, options?: ModelContextRegisterToolOptions): ToolRegistration {
    return this.#bridge.registerTool(tool, options)
  }

  unregisterTool(name: string): void {
    this.#bridge.unregisterTool(name)
  }

  provideContext(options?: ModelContextOptions): void {
    this.#bridge.provideContext(options)
  }

  clearContext(): void {
    this.#bridge.clearContext()
  }

  async getTools(options: ModelContextGetToolOptions = {}): Promise<RegisteredTool[]> {
    assertTrustworthyOrigins(options?.fromOrigins)
    const win = currentWindow()
    const origin = currentOrigin()
    const tools: RegisteredTool[] = []
    for (const entry of this.#bridge.entries.values()) {
      const t = entry.tool
      const inputSchema = typeof t.inputSchema === 'string' ? (JSON.parse(t.inputSchema) as object) : t.inputSchema
      tools.push({
        name: entry.name,
        title: t.title ?? '',
        description: t.description,
        ...(inputSchema !== undefined && { inputSchema: JSON.parse(JSON.stringify(inputSchema)) as object }),
        window: win,
        origin,
        ...(t.annotations !== undefined && { annotations: { ...t.annotations } }),
      })
    }
    if (this.#bridge.mirrorOwnTools) {
      for (const view of this.#bridge.ownTools()) {
        const def = view.definition
        if (def.enabled === false) continue
        let inputSchema: object
        try {
          inputSchema = await toJsonSchema(def.input)
        } catch {
          continue
        }
        tools.push({
          name: view.name,
          title: def.title ?? '',
          description: def.description,
          inputSchema,
          window: win,
          origin,
          annotations: riskToAnnotations(def.risk),
        })
      }
    }
    return tools.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
  }

  executeTool(
    tool: RegisteredTool | { name: string },
    input?: object,
    options: ModelContextExecuteToolOptions = {},
  ): Promise<string | null> {
    const bridge = this.#bridge
    const signal = options?.signal
    if (signal?.aborted) return Promise.reject(signal.reason)
    const name = String(tool?.name)
    let args: unknown
    try {
      const text = JSON.stringify(input ?? {})
      if (text === undefined) throw new TypeError('参数无法序列化为 JSON')
      args = JSON.parse(text)
    } catch (e) {
      return Promise.reject(e)
    }
    const entry = bridge.entries.get(name)
    const view =
      entry || !bridge.mirrorOwnTools
        ? undefined
        : bridge.ownTools().find((v) => v.name === name && v.definition.enabled !== false)
    if (!entry && !view) return Promise.reject(domException(`工具 ${name} 不存在`, 'UnknownError'))

    return new Promise<string | null>((resolve, reject) => {
      const controller = new AbortController()
      let settled = false
      const onAbort = (): void => {
        if (settled) return
        settled = true
        reject(signal?.reason)
        controller.abort(signal?.reason)
        bridge.dispatch('toolcancel', name)
      }
      signal?.addEventListener('abort', onAbort, { once: true })
      const finish = (ok: boolean, value: unknown): void => {
        signal?.removeEventListener('abort', onAbort)
        if (settled) return
        settled = true
        if (ok) resolve(value as string)
        else reject(domException(value instanceof Error ? value.message : String(value), 'UnknownError'))
      }
      bridge.dispatch('toolactivated', name)
      const run = async (): Promise<string> => {
        let result: unknown
        if (entry) {
          result = await entry.tool.execute.call(entry.tool, args, executeOptions(controller.signal))
        } else {
          const outcome = await (view as NonNullable<typeof view>).call(args, controller.signal)
          if ('error' in outcome) throw new Error(outcome.error.message)
          result = toStandardResult(outcome)
        }
        const text = JSON.stringify(result)
        if (text === undefined) throw new TypeError('工具返回值无法序列化为 JSON')
        return text
      }
      run().then(
        (text) => finish(true, text),
        (e: unknown) => finish(false, e),
      )
    })
  }

  get ontoolchange(): Handler {
    return this.#getHandler('toolchange')
  }
  set ontoolchange(handler: Handler) {
    this.#setHandler('toolchange', handler)
  }
  get ontoolactivated(): Handler {
    return this.#getHandler('toolactivated')
  }
  set ontoolactivated(handler: Handler) {
    this.#setHandler('toolactivated', handler)
  }
  get ontoolcancel(): Handler {
    return this.#getHandler('toolcancel')
  }
  set ontoolcancel(handler: Handler) {
    this.#setHandler('toolcancel', handler)
  }

  get [Symbol.toStringTag](): string {
    return 'ModelContext'
  }

  #getHandler(type: string): Handler {
    return this.#handlers.get(type)?.handler ?? null
  }

  #setHandler(type: string, handler: Handler): void {
    const prev = this.#handlers.get(type)
    if (prev) {
      this.removeEventListener(type, prev.listener)
      this.#handlers.delete(type)
    }
    if (typeof handler !== 'function') return
    const listener = (e: Event): void => {
      handler.call(this, e)
    }
    this.#handlers.set(type, { handler, listener })
    this.addEventListener(type, listener)
  }
}
