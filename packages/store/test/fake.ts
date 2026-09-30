import type {
  Registrar,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ToolDefinition,
  ToolHandle,
} from '@app-mcp/web'

export interface FakeTool {
  name: string
  def: ToolDefinition<any, any>
  updates: Array<Partial<Omit<ToolDefinition<any, any>, 'handler'>>>
  disposed: boolean
}

export interface FakeResource {
  name: string
  def: ResourceDefinition<any>
  notifies: number
  disposed: boolean
}

/** 记录注册、可调用 handler、记录 update / notifyChanged 的假 Registrar。 */
export class FakeRegistrar implements Registrar {
  readonly tools = new Map<string, FakeTool>()
  readonly resources = new Map<string, FakeResource>()

  tool<I, O>(name: string, definition: ToolDefinition<I, O>): ToolHandle {
    if (this.tools.has(name)) throw new Error(`工具 ${name} 已注册`)
    const rec: FakeTool = { name, def: definition as ToolDefinition<any, any>, updates: [], disposed: false }
    this.tools.set(name, rec)
    return {
      name,
      update: (changes) => {
        rec.updates.push(changes)
        rec.def = { ...rec.def, ...changes }
      },
      setHandler: (handler) => {
        rec.def = { ...rec.def, handler }
      },
      dispose: () => {
        rec.disposed = true
        this.tools.delete(name)
      },
    }
  }

  resource<T>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    if (this.resources.has(name)) throw new Error(`资源 ${name} 已注册`)
    const rec: FakeResource = { name, def: definition, notifies: 0, disposed: false }
    this.resources.set(name, rec)
    return {
      name,
      notifyChanged: () => {
        rec.notifies++
      },
      setReader: (read) => {
        rec.def = { ...rec.def, read }
      },
      dispose: () => {
        rec.disposed = true
        this.resources.delete(name)
      },
    }
  }

  scope(): Scope {
    throw new Error('未实现')
  }

  getTool(name: string): FakeTool {
    const t = this.tools.get(name)
    if (!t) throw new Error(`工具 ${name} 未注册`)
    return t
  }

  getResource(name: string): FakeResource {
    const r = this.resources.get(name)
    if (!r) throw new Error(`资源 ${name} 未注册`)
    return r
  }

  /** 调用工具 handler，返回原始结果（`{ data, stateHints? }`）。 */
  async call(name: string, input: unknown = {}): Promise<any> {
    return this.getTool(name).def.handler(input, { callId: 'c1', signal: new AbortController().signal })
  }
}

/** 等待微任务队列清空（核心在微任务中处理 store 变化）。 */
export function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}
