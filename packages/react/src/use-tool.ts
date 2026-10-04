import { useEffect, useRef } from 'react'
import type { LazyToolDefinition, Registrar, ToolDefinition, ToolHandle } from '@app-mcp/web'
import { useRegistrar } from './context'
import { schemaKey, useIsomorphicLayoutEffect } from './internal'

type Anchor = NonNullable<ToolDefinition['anchor']>
type AnyDef = ToolDefinition<any, any> | LazyToolDefinition<any, any>
type ToolChanges = Parameters<ToolHandle['update']>[0]

/** 已注册到 Host 的元数据快照，用于判断是否需要 `update`。 */
interface MetaSnapshot {
  description: string
  title: string | undefined
  risk: ToolDefinition['risk']
  activation: ToolDefinition['activation']
  enabled: boolean
  surface: ToolDefinition['surface']
  page: ToolDefinition['page']
  visibility: ToolDefinition['visibility']
  concurrency: ToolDefinition['concurrency']
  exclusive: ToolDefinition['exclusive']
  /** 按内容比较的字段：原值与比较键（{@link schemaKey}）。 */
  structured: Record<StructuredKey, { value: unknown; key: string | null }>
}

interface Registration {
  handle: ToolHandle
  registrar: Registrar
  name: string
  meta: MetaSnapshot
  hasAnchor: boolean
}

const SCALAR_KEYS = [
  'description', 'title', 'risk', 'activation', 'enabled', 'surface', 'page', 'visibility', 'concurrency', 'exclusive',
] as const
/** 常写成内联字面量（每次渲染引用都变）的字段：先比较引用，再比较内容。 */
const STRUCTURED_KEYS = ['input', 'outputSchema', 'annotations', 'implements', 'cache', 'deprecated'] as const
type StructuredKey = (typeof STRUCTURED_KEYS)[number]

function snapshot(def: AnyDef, prev?: MetaSnapshot): MetaSnapshot {
  const structured = {} as MetaSnapshot['structured']
  for (const k of STRUCTURED_KEYS) {
    const value: unknown = def[k]
    const before = prev?.structured[k]
    structured[k] = { value, key: before !== undefined && before.value === value ? before.key : schemaKey(value) }
  }
  return {
    description: def.description,
    title: def.title,
    risk: def.risk,
    activation: def.activation,
    enabled: def.enabled ?? true,
    surface: def.surface,
    page: def.page,
    visibility: def.visibility,
    concurrency: def.concurrency,
    exclusive: def.exclusive,
    structured,
  }
}

function diff(prev: MetaSnapshot, next: MetaSnapshot, def: AnyDef): ToolChanges | null {
  const changes: Record<string, unknown> = {}
  let changed = false
  for (const key of SCALAR_KEYS) {
    if (prev[key] !== next[key]) {
      changes[key] = next[key]
      changed = true
    }
  }
  for (const k of STRUCTURED_KEYS) {
    const a = prev.structured[k]
    const b = next.structured[k]
    if (a.value === b.value) continue
    // 引用变化时再比较内容；无法比较（key 为 null）时视为已变化。
    if (a.key === null || b.key === null || a.key !== b.key) {
      changes[k] = def[k]
      changed = true
    }
  }
  return changed ? (changes as ToolChanges) : null
}

/**
 * 在组件生命周期内注册一个工具。
 *
 * - 挂载时注册，卸载时注销；`name` 变化时重新注册。
 * - `handler` 每次渲染后通过 `setHandler` 刷新，始终使用最新闭包，不重新注册。
 * - 也可以给出惰性加载器 `load`（与 `handler` 二选一，见 `LazyToolDefinition`）：首次调用时加载并缓存；
 *   `load` 的引用变化不会重新加载。
 * - `description` / `title` / `input` / `outputSchema` / `risk` / `annotations` / `activation` / `enabled` /
 *   `surface` / `page` / `visibility` / `concurrency` / `exclusive` / `implements` / `cache` / `deprecated` 变化时调用 `update`，只传变化的字段
 *   （`input` / `outputSchema` / `annotations` / `implements` / `cache` / `deprecated`
 *   先比较引用，再比较转换后的 JSON 内容）。
 * - `surface: 'view'` 的工具由 `@app-mcp/web` 按可见性门控（锚点 `anchor`，缺省继承 `<ToolScope anchor>`；
 *   `<ToolLayer>` 打开时下层暂停）；`visibility: 'always'` 关闭门控。
 * - 不触发任何额外渲染：注册信息保存在 ref 中。
 * - 没有 `<AppMcpProvider>` 时为空操作。
 *
 * 返回当前的 `ToolHandle`；首次渲染时（注册发生在 effect 中）为 null。
 */
export function useTool<I = unknown, O = unknown>(
  name: string,
  definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>,
): ToolHandle | null {
  const registrar = useRegistrar()
  const latest = useRef<AnyDef>(definition)
  const registration = useRef<Registration | null>(null)
  // 稳定的 anchor 包装：始终解析最新定义中的 anchor，anchor 变化不需要 update。
  const anchorRef = useRef<Anchor | null>(null)
  if (anchorRef.current === null) {
    anchorRef.current = () => {
      const anchor = latest.current.anchor
      return typeof anchor === 'function' ? anchor() : (anchor ?? null)
    }
  }

  // 同步最新定义：刷新 handler，元数据变化时 update。
  useIsomorphicLayoutEffect(() => {
    latest.current = definition
    const reg = registration.current
    if (!reg || reg.registrar !== registrar || reg.name !== name) return
    if (definition.handler) reg.handle.setHandler(definition.handler)
    const next = snapshot(definition, reg.meta)
    const changes = diff(reg.meta, next, definition) ?? ({} as ToolChanges)
    if (!reg.hasAnchor && definition.anchor !== undefined) {
      changes.anchor = anchorRef.current!
      reg.hasAnchor = true
    }
    if (Object.keys(changes).length > 0) reg.handle.update(changes)
    reg.meta = next
  })

  // 注册 / 注销。StrictMode 下会经历 注册 → 注销 → 再注册，最终只保留一个。
  useEffect(() => {
    if (!registrar) return
    const def = latest.current
    const hasAnchor = def.anchor !== undefined
    const handle = registrar.tool(name, {
      ...def,
      ...(hasAnchor ? { anchor: anchorRef.current! } : {}),
    })
    const reg: Registration = { handle, registrar, name, meta: snapshot(def), hasAnchor }
    registration.current = reg
    return () => {
      if (registration.current === reg) registration.current = null
      handle.dispose()
    }
  }, [registrar, name])

  return registration.current?.handle ?? null
}
