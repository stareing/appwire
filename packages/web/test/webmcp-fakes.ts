/** WebMCP 测试工具：伪造的原生 modelContext、Host 调用辅助。 */

import type { CoreOutcome, CoreToolDef } from '../src/core'
import type { Harness } from './fakes'
import { settle } from './fakes'

export type Rec = [method: string, ...args: unknown[]]

interface NativeTool {
  name: string
  description: string
  inputSchema?: unknown
  annotations?: Record<string, unknown>
  title?: string
  execute: (input: unknown, options: { signal: AbortSignal }) => unknown
}

/**
 * 按当前标准实现的"原生" modelContext：`registerTool(tool, { signal })` 返回 Promise，
 * 没有 unregisterTool / provideContext / clearContext。记录所有调用。
 */
export class FakeNativeModelContext extends EventTarget {
  readonly tools = new Map<string, NativeTool>()
  readonly log: Rec[] = []

  registerTool(tool: NativeTool, options: { signal?: AbortSignal } = {}): Promise<undefined> {
    this.log.push(['registerTool', tool.name])
    if (this.tools.has(tool.name)) return Promise.reject(new DOMException('duplicate', 'InvalidStateError'))
    if (options.signal?.aborted) return Promise.reject(options.signal.reason)
    this.tools.set(tool.name, tool)
    options.signal?.addEventListener('abort', () => {
      if (this.tools.get(tool.name) === tool) {
        this.tools.delete(tool.name)
        this.log.push(['unregister(signal)', tool.name])
      }
    })
    return Promise.resolve(undefined)
  }

  /** 模拟浏览器内置 AI 调用：返回 JSON 字符串（同标准 executeTool）。 */
  async invoke(name: string, input: unknown = {}): Promise<unknown> {
    const tool = this.tools.get(name)
    if (!tool) throw new Error(`no tool ${name}`)
    const result = await tool.execute(input, { signal: new AbortController().signal })
    return JSON.parse(JSON.stringify(result) ?? 'null')
  }
}

/** 旧版原生（navigator.modelContext，2026 年初的形态）：同步返回，有 unregisterTool。 */
export class FakeLegacyModelContext extends EventTarget {
  readonly tools = new Map<string, NativeTool>()
  readonly log: Rec[] = []

  registerTool(tool: NativeTool): void {
    this.log.push(['registerTool', tool.name])
    if (this.tools.has(tool.name)) throw new DOMException('duplicate', 'InvalidStateError')
    this.tools.set(tool.name, tool)
  }
  unregisterTool(name: string): void {
    this.log.push(['unregisterTool', name])
    this.tools.delete(name)
  }
  provideContext(): void {
    throw new Error('不应调用原生 provideContext')
  }
  clearContext(): void {
    throw new Error('不应调用原生 clearContext')
  }
}

/** 核心为工具分配的句柄（FakeCore 按 createScope / registerTool / registerResource 顺序递增）。 */
export function toolIdOf(h: Harness, name: string): number | undefined {
  let id = 0
  let found: number | undefined
  for (const [method, ...args] of h.core.calls) {
    if (method === 'createScope' || method === 'registerTool' || method === 'registerResource') {
      id++
      if (method === 'registerTool' && (args[0] as CoreToolDef).name === name) found = id
    }
  }
  return found
}

/** 已注册到核心（且未注销）的工具定义。 */
export function coreTool(h: Harness, name: string): CoreToolDef | undefined {
  const id = toolIdOf(h, name)
  if (id === undefined) return undefined
  if (h.core.callsOf('unregisterTool').some(([t]) => t === id)) return undefined
  return h.core
    .callsOf('registerTool')
    .map(([d]) => d as CoreToolDef)
    .filter((d) => d.name === name)
    .at(-1)
}

let seq = 0

/** 模拟 Host 调用工具，返回交给核心的结果。 */
export async function hostCall(h: Harness, name: string, args: unknown = {}): Promise<CoreOutcome | undefined> {
  const tool = toolIdOf(h, name)
  if (tool === undefined) throw new Error(`工具 ${name} 未注册到核心`)
  const callId = `host-${++seq}`
  h.socket().script({ type: 'invokeTool', callId, tool, name, arguments: args })
  await settle()
  return h.core.callsOf('completeCall').find(([id]) => id === callId)?.[1] as CoreOutcome | undefined
}

/** 开始一个 Host 调用但不等待结果，返回 callId。 */
export function startHostCall(h: Harness, name: string, args: unknown = {}): string {
  const tool = toolIdOf(h, name)
  if (tool === undefined) throw new Error(`工具 ${name} 未注册到核心`)
  const callId = `host-${++seq}`
  h.socket().script({ type: 'invokeTool', callId, tool, name, arguments: args })
  return callId
}

/** 在 owner 上定义 modelContext（模拟原生），返回撤销函数。 */
export function defineNative(owner: object, value: unknown): () => void {
  Object.defineProperty(owner, 'modelContext', { configurable: true, enumerable: true, value })
  return () => {
    delete (owner as Record<string, unknown>).modelContext
  }
}
