/**
 * 测试用的 AppMcp 假实现：记录注册、更新与注销，并可以模拟模型调用 handler。
 * 不参与打包（tsup 只打包 src/index.ts）。
 */
import type {
  AppMcp,
  AppMcpOptions,
  ConnectionState,
  HoldHandle,
  ResourceDefinition,
  ResourceHandle,
  LazyToolDefinition,
  Scope,
  ToolDefinition,
  ToolHandle,
} from '@app-mcp/web'

type AnyDef<I = any, O = any> = ToolDefinition<I, O> | LazyToolDefinition<I, O>

export type FakeEvent =
  | { type: 'tool.register'; name: string; scope: string | null }
  | { type: 'tool.update'; name: string; changes: Record<string, unknown> }
  | { type: 'tool.setHandler'; name: string }
  | { type: 'tool.dispose'; name: string }
  | { type: 'resource.register'; name: string; scope: string | null }
  | { type: 'resource.setReader'; name: string }
  | { type: 'resource.notify'; name: string }
  | { type: 'resource.dispose'; name: string }
  | { type: 'scope.create'; name: string; parent: string | null }
  | { type: 'scope.dispose'; name: string }

interface ToolEntry {
  name: string
  definition: AnyDef
  scope: FakeScope | null
  disposed: boolean
}

interface ResourceEntry {
  name: string
  definition: ResourceDefinition<any>
  scope: FakeScope | null
  disposed: boolean
}

class FakeScope implements Scope {
  disposed = false
  readonly children = new Set<FakeScope>()
  readonly tools = new Set<ToolEntry>()
  readonly resources = new Set<ResourceEntry>()

  constructor(
    private readonly app: FakeAppMcp,
    readonly name: string,
    readonly parent: FakeScope | null,
  ) {}

  tool<I, O>(name: string, definition: AnyDef<I, O>): ToolHandle {
    return this.app.registerTool(name, definition, this)
  }

  resource<T>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return this.app.registerResource(name, definition, this)
  }

  scope(name: string): Scope {
    return this.app.createScope(name, this)
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    for (const child of this.children) child.dispose()
    for (const tool of this.tools) this.app.disposeTool(tool)
    for (const resource of this.resources) this.app.disposeResource(resource)
    this.app.events.push({ type: 'scope.dispose', name: this.name })
  }
}

export class FakeAppMcp implements AppMcp {
  readonly options: Readonly<AppMcpOptions> = { appId: 'test', appName: '测试' }
  readonly instanceId = 'fake-instance'
  readonly events: FakeEvent[] = []
  private readonly toolEntries = new Map<string, ToolEntry>()
  private readonly resourceEntries = new Map<string, ResourceEntry>()
  private currentState: ConnectionState = { status: 'idle' }
  private readonly listeners = new Set<(state: ConnectionState) => void>()
  /** 仍存活的 scope 数量。 */
  liveScopes = 0
  /** 生命周期调用记录（wake / sleep / connectNow）。 */
  readonly lifecycleCalls: string[] = []
  /** 当前未释放的持有数。 */
  activeHolds = 0

  wake(): void {
    this.lifecycleCalls.push('wake')
  }

  sleep(): void {
    this.lifecycleCalls.push('sleep')
  }

  connectNow(): void {
    this.lifecycleCalls.push('connectNow')
  }

  hold(): HoldHandle {
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

  get state(): ConnectionState {
    return this.currentState
  }

  setState(state: ConnectionState): void {
    this.currentState = state
    for (const listener of this.listeners) listener(state)
  }

  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  tool<I, O>(name: string, definition: AnyDef<I, O>): ToolHandle {
    return this.registerTool(name, definition, null)
  }

  resource<T>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return this.registerResource(name, definition, null)
  }

  scope(name: string): Scope {
    return this.createScope(name, null)
  }

  dispose(): void {
    for (const tool of [...this.toolEntries.values()]) this.disposeTool(tool)
    for (const resource of [...this.resourceEntries.values()]) this.disposeResource(resource)
  }

  // ---- 查询 ---------------------------------------------------------------

  /** 当前已注册的工具名（排序）。 */
  toolNames(): string[] {
    return [...this.toolEntries.keys()].sort()
  }

  resourceNames(): string[] {
    return [...this.resourceEntries.keys()].sort()
  }

  getTool(name: string): AnyDef | undefined {
    return this.toolEntries.get(name)?.definition
  }

  getToolScope(name: string): string | null | undefined {
    const entry = this.toolEntries.get(name)
    return entry ? (entry.scope?.name ?? null) : undefined
  }

  count(type: FakeEvent['type'], name?: string): number {
    return this.events.filter((e) => e.type === type && (name === undefined || ('name' in e && e.name === name)))
      .length
  }

  /** 模拟模型调用工具。 */
  async call(name: string, input: unknown = {}): Promise<unknown> {
    const entry = this.toolEntries.get(name)
    if (!entry) throw new Error(`工具不存在：${name}`)
    if (entry.definition.enabled === false) throw new Error(`工具已禁用：${name}`)
    const def = entry.definition
    let handler = def.handler
    if (!handler && def.load) {
      const loaded = await def.load()
      handler = typeof loaded === 'function' ? loaded : loaded.default
    }
    if (!handler) throw new Error(`工具没有 handler：${name}`)
    return handler(input, { callId: 'call-1', signal: new AbortController().signal })
  }

  async read(name: string): Promise<unknown> {
    const entry = this.resourceEntries.get(name)
    if (!entry) throw new Error(`资源不存在：${name}`)
    return entry.definition.read()
  }

  // ---- 内部 ---------------------------------------------------------------

  registerTool<I, O>(name: string, definition: AnyDef<I, O>, scope: FakeScope | null): ToolHandle {
    if (scope?.disposed) throw new Error(`scope 已注销：${scope.name}`)
    if (this.toolEntries.has(name)) throw new Error(`工具重复注册：${name}`)
    const entry: ToolEntry = { name, definition: { ...definition } as AnyDef, scope, disposed: false }
    this.toolEntries.set(name, entry)
    scope?.tools.add(entry)
    this.events.push({ type: 'tool.register', name, scope: scope?.name ?? null })
    return {
      name,
      update: (changes) => {
        if (entry.disposed) return
        entry.definition = { ...entry.definition, ...changes } as AnyDef
        this.events.push({ type: 'tool.update', name, changes: { ...changes } })
      },
      setHandler: (handler) => {
        if (entry.disposed) return
        entry.definition = { ...entry.definition, handler, load: undefined }
        this.events.push({ type: 'tool.setHandler', name })
      },
      dispose: () => this.disposeTool(entry),
    }
  }

  disposeTool(entry: ToolEntry): void {
    if (entry.disposed) return
    entry.disposed = true
    entry.scope?.tools.delete(entry)
    if (this.toolEntries.get(entry.name) === entry) this.toolEntries.delete(entry.name)
    this.events.push({ type: 'tool.dispose', name: entry.name })
  }

  registerResource<T>(name: string, definition: ResourceDefinition<T>, scope: FakeScope | null): ResourceHandle {
    if (scope?.disposed) throw new Error(`scope 已注销：${scope.name}`)
    if (this.resourceEntries.has(name)) throw new Error(`资源重复注册：${name}`)
    const entry: ResourceEntry = { name, definition: { ...definition } as ResourceDefinition<any>, scope, disposed: false }
    this.resourceEntries.set(name, entry)
    scope?.resources.add(entry)
    this.events.push({ type: 'resource.register', name, scope: scope?.name ?? null })
    return {
      name,
      notifyChanged: () => {
        if (entry.disposed) return
        this.events.push({ type: 'resource.notify', name })
      },
      setReader: (read) => {
        if (entry.disposed) return
        entry.definition.read = read
        this.events.push({ type: 'resource.setReader', name })
      },
      dispose: () => this.disposeResource(entry),
    }
  }

  disposeResource(entry: ResourceEntry): void {
    if (entry.disposed) return
    entry.disposed = true
    entry.scope?.resources.delete(entry)
    if (this.resourceEntries.get(entry.name) === entry) this.resourceEntries.delete(entry.name)
    this.events.push({ type: 'resource.dispose', name: entry.name })
  }

  createScope(name: string, parent: FakeScope | null): Scope {
    if (parent?.disposed) throw new Error(`scope 已注销：${parent.name}`)
    const scope = new FakeScope(this, name, parent)
    parent?.children.add(scope)
    this.liveScopes++
    const originalDispose = scope.dispose.bind(scope)
    scope.dispose = () => {
      if (!scope.disposed) this.liveScopes--
      originalDispose()
    }
    this.events.push({ type: 'scope.create', name, parent: parent?.name ?? null })
    return scope
  }
}
