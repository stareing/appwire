import { useEffect, useRef } from 'react'
import type { CacheScope, Registrar, ResourceDefinition, ResourceHandle } from '@app-mcp/web'
import { useRegistrar } from './context'
import { depsChanged, useIsomorphicLayoutEffect } from './internal'

export interface UseResourceOptions<T = unknown> extends ResourceDefinition<T> {
  /**
   * 资源内容依赖的值。任一项变化（Object.is）时调用 `notifyChanged()`；
   * 首次挂载不通知。缺省时从不自动通知（可以用返回的 handle 手动通知）。
   */
  deps?: readonly unknown[]
}

interface Registration {
  handle: ResourceHandle
  registrar: Registrar
  name: string
  description: string
  mimeType: string | undefined
  cacheTtlMs: number | undefined
  cacheScope: CacheScope | undefined
  deps: readonly unknown[] | undefined
}

/**
 * 在组件生命周期内注册一个资源。
 *
 * - 挂载时注册，卸载时注销；`name`、`description`、`mimeType`、`cache`（按 `ttlMs` / `scope` 的值比较）变化时重新注册。
 * - `read` 每次渲染后通过 `setReader` 刷新，始终读取最新状态。
 * - `deps` 变化时调用 `notifyChanged()`（首次挂载与重新注册时不调用）。
 * - 不触发任何额外渲染；没有 `<AppMcpProvider>` 时为空操作。
 *
 * 返回当前的 `ResourceHandle`；首次渲染时为 null。
 */
export function useResource<T = unknown>(
  name: string,
  options: UseResourceOptions<T>,
): ResourceHandle | null {
  const registrar = useRegistrar()
  const latest = useRef<UseResourceOptions<any>>(options)
  const registration = useRef<Registration | null>(null)
  const { description, mimeType } = options
  const cacheTtlMs = options.cache?.ttlMs
  const cacheScope = options.cache?.scope

  useIsomorphicLayoutEffect(() => {
    latest.current = options
    const reg = registration.current
    if (
      !reg ||
      reg.registrar !== registrar ||
      reg.name !== name ||
      reg.description !== description ||
      reg.mimeType !== mimeType ||
      reg.cacheTtlMs !== cacheTtlMs ||
      reg.cacheScope !== cacheScope
    ) {
      return
    }
    reg.handle.setReader(options.read)
    if (depsChanged(reg.deps, options.deps)) {
      reg.deps = options.deps
      reg.handle.notifyChanged()
    }
  })

  useEffect(() => {
    if (!registrar) return
    const opts = latest.current
    const definition: ResourceDefinition<any> = { description, read: opts.read }
    if (mimeType !== undefined) definition.mimeType = mimeType
    if (cacheTtlMs !== undefined) definition.cache = { ttlMs: cacheTtlMs, ...(cacheScope !== undefined && { scope: cacheScope }) }
    const handle = registrar.resource(name, definition)
    const reg: Registration = { handle, registrar, name, description, mimeType, cacheTtlMs, cacheScope, deps: opts.deps }
    registration.current = reg
    return () => {
      if (registration.current === reg) registration.current = null
      handle.dispose()
    }
  }, [registrar, name, description, mimeType, cacheTtlMs, cacheScope])

  return registration.current?.handle ?? null
}
