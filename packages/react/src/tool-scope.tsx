import { useEffect, useMemo, useRef, type ReactNode, type RefObject } from 'react'
import type {
  LazyToolDefinition,
  ResourceDefinition,
  ResourceHandle,
  Registrar,
  Scope,
  ScopeOptions,
  ToolDefinition,
  ToolHandle,
  ToolSurface,
  ViewVisibility,
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
    private readonly options?: ScopeOptions,
  ) {}

  /** 真实的 scope 是否已创建（测试用）。 */
  get active(): boolean {
    return this.current !== null
  }

  private ensure(): Scope {
    if (!this.current) {
      if (this.parent instanceof LazyScope) this.parent.children.add(this)
      this.current = this.options ? this.parent.scope(this.name, this.options) : this.parent.scope(this.name)
    }
    return this.current
  }

  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle {
    return this.ensure().tool(name, definition)
  }

  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return this.ensure().resource(name, definition)
  }

  scope(name: string, options?: ScopeOptions): Scope {
    return this.ensure().scope(name, options)
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

/** 锚点：元素、返回元素的函数或 React ref。 */
export type AnchorProp = Element | (() => Element | null) | RefObject<Element | null>

function resolveAnchor(anchor: AnchorProp | undefined): Element | null {
  if (anchor === undefined) return null
  if (typeof anchor === 'function') return anchor()
  if ('current' in anchor && !(anchor instanceof Element)) return anchor.current
  return anchor as Element
}

/**
 * 稳定的锚点解析函数：始终读取最新一次渲染给出的 `anchor`（内联函数 / ref 变化不需要重建 scope）。
 * 没有给出 `anchor` 时返回 undefined（不声明锚点，继承上层）。
 */
export function useAnchorResolver(anchor: AnchorProp | undefined): (() => Element | null) | undefined {
  const latest = useRef(anchor)
  latest.current = anchor
  const resolver = useMemo(() => () => resolveAnchor(latest.current), [])
  return anchor === undefined ? undefined : resolver
}

/** 去掉值为 undefined 的键；全部为空时返回 undefined（不传 options，与旧行为一致）。 */
export function compactOptions(options: ScopeOptions): ScopeOptions | undefined {
  const entries = Object.entries(options).filter(([, v]) => v !== undefined)
  return entries.length === 0 ? undefined : (Object.fromEntries(entries) as ScopeOptions)
}

export interface ToolScopeProps {
  /** scope 名称（仅用于调试，不影响工具名）。 */
  name: string
  /**
   * 其下工具的缺省锚点（如页面根元素的 ref）：`view` 工具在锚点可见时才启用（spec/protocol.md 3.4）。
   * 挂载后才变化的元素请用 ref 或函数。
   */
  anchor?: AnchorProp
  /** 其下工具的缺省页面名（页面目录的键，与清单 `pages[].name` 一致）。 */
  page?: string
  /** 其下工具的缺省 surface（缺省继承上层，最终缺省 `app`）。 */
  surface?: ToolSurface
  /** 其下 `view` 工具的缺省可见性门控。 */
  visibility?: ViewVisibility
  children?: ReactNode
}

/**
 * 为子组件中的 `useTool` / `useResource` 创建子 scope；卸载时注销其下全部工具与资源。
 * `anchor` / `page` / `surface` / `visibility` 是其下工具的缺省界面声明（工具自身声明优先），只在创建 scope 时读取
 * （`page` / `surface` / `visibility` 变化时重建 scope；`anchor` 始终读取最新值）。
 * 没有 `<AppMcpProvider>` 时直接渲染子组件。
 */
export function ToolScope({ name, anchor, page, surface, visibility, children }: ToolScopeProps) {
  const parent = useRegistrar()
  const anchorResolver = useAnchorResolver(anchor)
  const scope = useMemo(
    () => (parent ? new LazyScope(parent, name, compactOptions({ anchor: anchorResolver, page, surface, visibility })) : null),
    [parent, name, anchorResolver, page, surface, visibility],
  )

  useEffect(() => {
    if (!scope) return
    return () => scope.dispose()
  }, [scope])

  return <RegistrarContext.Provider value={scope ?? parent}>{children}</RegistrarContext.Provider>
}
