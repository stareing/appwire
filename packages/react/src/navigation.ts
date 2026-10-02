import { useCallback, useContext, useRef } from 'react'
import {
  guardDenial,
  pagePath,
  routePages,
  ToolCallError,
  type GuardResult,
  type NavigationHandler,
  type NavigationOptions,
  type NavigationRequest,
  type RouteLike,
} from '@app-mcp/web'
import { AppMcpContext } from './context'
import { useIsomorphicLayoutEffect } from './internal'

/**
 * 在组件挂载期间设置导航回调（spec/protocol.md 3.4，Host 调用不在当前页面的工具时请求切换页面）。
 * 回调始终使用最新闭包；卸载时清除。请放在根组件（早于与 Host 的连接建立，能力在握手时声明）。
 * 没有 `<AppMcpProvider>` 或实现不支持导航（Electron / Tauri 页面侧）时为空操作。
 */
export function useNavigationHandler(handler: NavigationHandler | null, options: NavigationOptions = {}): void {
  const appMcp = useContext(AppMcpContext)
  const latest = useRef(handler)
  latest.current = handler
  const enabled = handler !== null
  const { whileLayerOpen, settleMs } = options

  useIsomorphicLayoutEffect(() => {
    if (!appMcp || !enabled) return
    if (typeof appMcp.setNavigationHandler !== 'function') {
      appMcp.options.logger?.warn('[app-mcp] 当前 AppMcp 实现不支持导航，导航回调未生效')
      return
    }
    const navOptions: NavigationOptions = {}
    if (whileLayerOpen !== undefined) navOptions.whileLayerOpen = whileLayerOpen
    if (settleMs !== undefined) navOptions.settleMs = settleMs
    appMcp.setNavigationHandler(async (request) => {
      await latest.current?.(request)
    }, navOptions)
    return () => appMcp.setNavigationHandler?.(null)
  }, [appMcp, enabled, whileLayerOpen, settleMs])
}

export interface RouterNavigationOptions extends NavigationOptions {
  /**
   * 跳转函数：React Router 的 `useNavigate()`（`(to: string) => void | Promise<void>`），或任何接受地址的函数
   * （自写路由、`history.push`）。
   */
  navigate: (to: string) => unknown
  /**
   * 页面表：`{ 页面名: 路由模式 }`（如 `{ orders: '/orders/:id' }`），或路由表（React Router `RouteObject[]`，带 `id` 的路由
   * 是页面，见 `routePages`）。导航参数填入路由参数，其余作为查询串。
   */
  pages: Readonly<Record<string, string>> | readonly RouteLike[]
  /** 导航前检查：返回 `false` 或字符串（拒绝原因）时以 `NAVIGATION_DENIED` 拒绝。 */
  guard?: (request: NavigationRequest) => GuardResult
}

function isRouteList(pages: RouterNavigationOptions['pages']): pages is readonly RouteLike[] {
  return Array.isArray(pages)
}

/**
 * React Router 适配（第 4c 项 D）：把 Host 的 `app/navigate {page, params}` 转成 `navigate(地址)`。
 *
 * ```tsx
 * function Root() {
 *   useRouterNavigation({ navigate: useNavigate(), pages: routes })
 *   return <Outlet />
 * }
 * ```
 *
 * @error 页面不在表中、缺少路由参数 → `NAVIGATION_FAILED`；`guard` 拒绝 → `NAVIGATION_DENIED`。
 * @why 不导入 react-router：只需要 `navigate` 函数与路由表的结构，任何路由库（或自写路由）都能接入。
 */
export function useRouterNavigation(options: RouterNavigationOptions): void {
  const latest = useRef(options)
  latest.current = options
  const handler = useCallback(async (request: NavigationRequest): Promise<void> => {
    const { navigate, pages, guard } = latest.current
    const table = isRouteList(pages) ? routePages(pages) : pages
    const pattern = Object.prototype.hasOwnProperty.call(table, request.page) ? table[request.page] : undefined
    if (pattern === undefined) throw ToolCallError.navigationFailed(`页面「${request.page}」不存在（不在路由页面表中）`)
    const denial = guard ? guardDenial(guard(request), request.page) : undefined
    if (denial !== undefined) throw ToolCallError.navigationDenied(denial)
    await navigate(pagePath(pattern, request.params ?? {}, request.page))
  }, [])
  useNavigationHandler(handler, { whileLayerOpen: options.whileLayerOpen, settleMs: options.settleMs } as NavigationOptions)
}
