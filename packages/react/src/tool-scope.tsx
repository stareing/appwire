import { useEffect, useMemo, type ReactNode } from 'react'
import type {
  LazyToolDefinition,
  ResourceDefinition,
  ResourceHandle,
  Registrar,
  Scope,
  ToolDefinition,
  ToolHandle,
} from '@app-mcp/web'
import { RegistrarContext, useRegistrar } from './context'

/**
 * 惰性 scope：第一次有工具、资源或子 scope 注册时才真正创建 `Scope`。
 *
 * React 先执行子组件的 effect、再执行父组件的 effect，所以 `<ToolScope>` 不能在自己的
 * effect 里创建 scope 再交给子组件；这里改为由第一个注册者触发创建。渲染阶段只创建本对象，
 * 没有副作用，StrictMode 下重复渲染也不会泄漏。
 */
export class LazyScope implements Registrar {
  private current: Scope | null = null
  private readonly children = new Set<LazyScope>()

  constructor(
    private readonly parent: Registrar,
    readonly name: string,
  ) {}

  /** 真实的 scope 是否已创建（测试用）。 */
  get active(): boolean {
    return this.current !== null
  }

  private ensure(): Scope {
    if (!this.current) {
      if (this.parent instanceof LazyScope) this.parent.children.add(this)
      this.current = this.parent.scope(this.name)
    }
    return this.current
  }

  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle {
    return this.ensure().tool(name, definition)
  }

  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return this.ensure().resource(name, definition)
  }

  scope(name: string): Scope {
    return this.ensure().scope(name)
  }

  /** 注销真实 scope；之后再有注册时会重新创建。 */
  dispose(): void {
    const scope = this.current
    this.current = null
    // 真实子 scope 会随父 scope 一起注销，这里同步重置子 LazyScope 的引用。
    for (const child of this.children) child.reset()
    this.children.clear()
    scope?.dispose()
  }

  private reset(): void {
    this.current = null
    for (const child of this.children) child.reset()
    this.children.clear()
  }
}

export interface ToolScopeProps {
  /** scope 名称（仅用于调试，不影响工具名）。 */
  name: string
  children?: ReactNode
}

/**
 * 为子组件中的 `useTool` / `useResource` 创建子 scope；卸载时注销其下全部工具与资源。
 * 没有 `<AppMcpProvider>` 时直接渲染子组件。
 */
export function ToolScope({ name, children }: ToolScopeProps) {
  const parent = useRegistrar()
  const scope = useMemo(() => (parent ? new LazyScope(parent, name) : null), [parent, name])

  useEffect(() => {
    if (!scope) return
    return () => scope.dispose()
  }, [scope])

  return <RegistrarContext.Provider value={scope ?? parent}>{children}</RegistrarContext.Provider>
}
