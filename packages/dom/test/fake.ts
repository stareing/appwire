/**
 * 测试用的假 AppMcp：实现 @app-mcp/web 的公开接口，记录注册与更新，可以直接调用 handler。
 */

import type {
  AppMcp,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ToolDefinition,
  ToolHandle,
} from '@app-mcp/web'

const NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/

export interface FakeTool {
  name: string
  scope: string | undefined
  def: ToolDefinition<any, any>
  /** 合并更新后的当前定义。 */
  current: Omit<ToolDefinition<any, any>, 'handler'>
  updates: Array<Record<string, unknown>>
  disposed: boolean
}

export interface FakeResource {
  name: string
  scope: string | undefined
  def: ResourceDefinition<any>
  notifications: number
  disposed: boolean
}

export interface FakeScopeRec {
  name: string
  path: string
  disposed: boolean
  tools: Set<FakeTool>
  resources: Set<FakeResource>
  children: Set<FakeScopeRec>
}

export interface FakeAppMcp extends AppMcp {
  tools: Map<string, FakeTool>
  resources: Map<string, FakeResource>
  scopes: FakeScopeRec[]
  history: FakeTool[]
  call(name: string, input?: unknown, signal?: AbortSignal): Promise<unknown>
  read(name: string): Promise<unknown>
}

export function createFakeAppMcp(): FakeAppMcp {
  const tools = new Map<string, FakeTool>()
  const resources = new Map<string, FakeResource>()
  const scopes: FakeScopeRec[] = []
  const history: FakeTool[] = []

  const disposeTool = (t: FakeTool): void => {
    if (t.disposed) return
    t.disposed = true
    if (tools.get(t.name) === t) tools.delete(t.name)
  }
  const disposeResource = (r: FakeResource): void => {
    if (r.disposed) return
    r.disposed = true
    if (resources.get(r.name) === r) resources.delete(r.name)
  }
  const disposeScope = (s: FakeScopeRec): void => {
    if (s.disposed) return
    s.disposed = true
    for (const t of s.tools) disposeTool(t)
    for (const r of s.resources) disposeResource(r)
    for (const c of s.children) disposeScope(c)
  }

  const makeRegistrar = (parent: FakeScopeRec | undefined) => ({
    tool(name: string, def: ToolDefinition<any, any>): ToolHandle {
      if (!NAME_RE.test(name)) throw new Error(`无效的工具名 ${name}`)
      if (tools.has(name)) throw new Error(`工具 ${name} 已注册`)
      const { handler: _h, ...rest } = def
      const t: FakeTool = { name, scope: parent?.path, def, current: { ...rest }, updates: [], disposed: false }
      if (parent?.disposed) t.disposed = true
      else {
        tools.set(name, t)
        parent?.tools.add(t)
      }
      history.push(t)
      return {
        name,
        update(changes) {
          if (t.disposed) return
          t.updates.push({ ...changes })
          Object.assign(t.current, changes)
        },
        setHandler(handler) {
          t.def = { ...t.def, handler }
        },
        dispose() {
          disposeTool(t)
        },
      }
    },
    resource(name: string, def: ResourceDefinition<any>): ResourceHandle {
      if (!NAME_RE.test(name)) throw new Error(`无效的资源名 ${name}`)
      if (resources.has(name)) throw new Error(`资源 ${name} 已注册`)
      const r: FakeResource = { name, scope: parent?.path, def, notifications: 0, disposed: false }
      if (parent?.disposed) r.disposed = true
      else {
        resources.set(name, r)
        parent?.resources.add(r)
      }
      return {
        name,
        notifyChanged() {
          if (!r.disposed) r.notifications++
        },
        setReader(read) {
          r.def = { ...r.def, read }
        },
        dispose() {
          disposeResource(r)
        },
      }
    },
    scope(name: string): Scope {
      const rec: FakeScopeRec = {
        name,
        path: parent ? `${parent.path}/${name}` : name,
        disposed: !!parent?.disposed,
        tools: new Set(),
        resources: new Set(),
        children: new Set(),
      }
      parent?.children.add(rec)
      scopes.push(rec)
      return { name, ...makeRegistrar(rec), dispose: () => disposeScope(rec) }
    },
  })

  const root = makeRegistrar(undefined)
  return {
    ...root,
    options: { appId: 'test', appName: '测试' },
    instanceId: 'i1',
    state: { status: 'idle' },
    onStateChange: () => () => {},
    dispose() {},
    tools,
    resources,
    scopes,
    history,
    async call(name, input = {}, signal = new AbortController().signal) {
      const t = tools.get(name)
      if (!t) throw new Error(`工具 ${name} 不存在`)
      return t.def.handler(input, { callId: 'c1', signal })
    },
    async read(name) {
      const r = resources.get(name)
      if (!r) throw new Error(`资源 ${name} 不存在`)
      return r.def.read()
    },
  }
}

/** 等待 MutationObserver 与合并调度完成。 */
export async function settle(ms = 150): Promise<void> {
  await new Promise((r) => setTimeout(r, ms))
}
