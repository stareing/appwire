/**
 * 极简路由（演示用，避免示例依赖 react-router）：History API + 与 React Router 同形的路由对象（`path`、`id`、`element`、
 * `handle`）。`@app-mcp/react` 的 `useRouterNavigation` 只需要 `navigate(地址)` 与路由表，接 React Router 时写法相同。
 *
 * `handle.keepAlive`：首次访问后保持挂载，离开时只隐藏（`hidden`），演示 keep-alive 页面的工具随可见性暂停 / 恢复。
 */
import { useSyncExternalStore, type ReactNode } from 'react'

export interface RouteObject {
  path: string
  /** 页面名（与清单 `pages[].name` 一致）；没有 id 的路由不是页面。 */
  id?: string
  element: ReactNode
  handle?: { mcp?: Record<string, unknown>; keepAlive?: boolean }
}

const listeners = new Set<() => void>()

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  window.addEventListener('popstate', listener)
  return () => {
    listeners.delete(listener)
    window.removeEventListener('popstate', listener)
  }
}

/** 跳转（保留查询串之外的状态由页面自己管理）。 */
export function navigate(to: string): void {
  if (to === location.pathname + location.search) return
  history.pushState(history.state, '', to)
  for (const listener of [...listeners]) listener()
}

export function usePathname(): string {
  return useSyncExternalStore(subscribe, () => location.pathname, () => '/')
}

/** 当前路由对应的页面；`keepAlive` 的页面访问过后一直挂载（不在当前路由时隐藏）。 */
export function Routes({ routes, visited }: { routes: readonly RouteObject[]; visited: Set<string> }) {
  const pathname = usePathname()
  const active = routes.find((r) => r.path === pathname) ?? routes[0]
  if (active?.handle?.keepAlive) visited.add(active.path)
  return (
    <>
      {routes.map((route) => {
        const isActive = route === active
        if (route.handle?.keepAlive && visited.has(route.path)) {
          return (
            <div key={route.path} hidden={!isActive} data-route={route.path}>
              {route.element}
            </div>
          )
        }
        return isActive ? (
          <div key={route.path} data-route={route.path}>
            {route.element}
          </div>
        ) : null
      })}
    </>
  )
}
