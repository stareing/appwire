import type { Registrar, ResourceDefinition, ResourceHandle, Scope, ToolDefinition, ToolHandle } from '@app-mcp/web'

export interface FakeTool {
  name: string
  def: ToolDefinition<any, any>
  disposed: boolean
}

/** 记录注册并可直接调用 handler 的假 Registrar。 */
export class FakeRegistrar implements Registrar {
  readonly tools = new Map<string, FakeTool>()

  tool<I, O>(name: string, definition: ToolDefinition<I, O>): ToolHandle {
    if (this.tools.has(name)) throw new Error(`工具 ${name} 已注册`)
    const rec: FakeTool = { name, def: definition as ToolDefinition<any, any>, disposed: false }
    this.tools.set(name, rec)
    return {
      name,
      update: (changes) => {
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

  resource<T>(_name: string, _definition: ResourceDefinition<T>): ResourceHandle {
    throw new Error('未实现')
  }

  scope(): Scope {
    throw new Error('未实现')
  }

  getTool(name: string): FakeTool {
    const t = this.tools.get(name)
    if (!t) throw new Error(`工具 ${name} 未注册`)
    return t
  }

  async call(name: string, input: unknown = {}): Promise<any> {
    return this.getTool(name).def.handler(input, { callId: 'c1', signal: new AbortController().signal })
  }
}
