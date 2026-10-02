/**
 * Vue Router 导航适配（`@app-mcp/web/vue-router`，第 4c 项 D）：把 Host 的 `app/navigate` 转成 `router.push({ name })`。
 *
 * ```ts
 * import { bindVueRouter } from '@app-mcp/web/vue-router'
 * const unbind = bindVueRouter(appMcp, router)   // 页面名 = 路由 name；在 app.use(router) 之后、挂载前调用
 * ```
 *
 * @why 只依赖结构化类型（{@link VueRouterLike}），不导入 vue-router：本包不为一个适配引入框架依赖，
 *   与 vue-router 4.x 的 `Router` 结构兼容。
 */

import { ToolCallError, type AppMcp, type NavigationOptions, type NavigationRequest } from '../types'
import { guardDenial, type GuardResult } from './guard'

/** `router.getRoutes()` 的条目子集。 */
export interface VueRouteRecordLike {
  name?: string | symbol | null
  path: string
}

/** 命名路由的跳转目标。 */
export interface VueRouteLocationLike {
  name: string
  params?: Record<string, string | string[]>
  query?: Record<string, string>
}

/** vue-router 4 `Router` 的子集。 */
export interface VueRouterLike {
  push(to: VueRouteLocationLike): Promise<unknown>
  hasRoute(name: string): boolean
  getRoutes(): readonly VueRouteRecordLike[]
}

export interface VueRouterNavigationOptions extends NavigationOptions {
  /** 页面名 → 路由 name。缺省页面名即路由 name；给出时只有表中的页面可导航。 */
  pages?: Readonly<Record<string, string>>
  /** 导航前检查：返回 `false` 或字符串（拒绝原因）时以 `NAVIGATION_DENIED` 拒绝。 */
  guard?: (request: NavigationRequest) => GuardResult
}

/** vue-router `NavigationFailureType`：被守卫中止 / 被新导航取消 / 已在目标位置。 */
const FAILURE_ABORTED = 4
const FAILURE_CANCELLED = 8
const FAILURE_DUPLICATED = 16

const PATH_PARAM = /:([A-Za-z_$][\w$]*)/g

function failureType(result: unknown): number | undefined {
  if (result === null || typeof result !== 'object') return undefined
  const type = (result as { type?: unknown }).type
  return typeof type === 'number' ? type : undefined
}

function toParam(value: unknown): string | string[] {
  if (Array.isArray(value)) return value.map((v) => String(v))
  return typeof value === 'object' && value !== null ? JSON.stringify(value) : String(value)
}

/** 按路由记录的路径参数名拆分：路径参数进 `params`，其余进 `query`。 */
function splitParams(
  path: string | undefined,
  params: Record<string, unknown> | undefined,
): Pick<VueRouteLocationLike, 'params' | 'query'> {
  if (!params) return {}
  const keys = new Set([...(path ?? '').matchAll(PATH_PARAM)].map((m) => m[1]))
  const pathParams: Record<string, string | string[]> = {}
  const query: Record<string, string> = {}
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue
    if (keys.has(key)) pathParams[key] = toParam(value)
    else query[key] = Array.isArray(value) || typeof value === 'object' ? JSON.stringify(value) : String(value)
  }
  return {
    ...(Object.keys(pathParams).length > 0 && { params: pathParams }),
    ...(Object.keys(query).length > 0 && { query }),
  }
}

/**
 * 设置 Vue Router 导航回调，返回解除函数。`appMcp` 不支持导航（桥接实现）时记录警告并返回空操作。
 *
 * @error 回调中：页面不存在 → `NAVIGATION_FAILED`；守卫拒绝（`guard` 或路由守卫返回 false）→ `NAVIGATION_DENIED`；
 *   已在目标页面按成功处理。
 */
export function bindVueRouter(appMcp: AppMcp, router: VueRouterLike, options: VueRouterNavigationOptions = {}): () => void {
  const { pages, guard, ...navOptions } = options
  if (typeof appMcp.setNavigationHandler !== 'function') {
    appMcp.options.logger?.warn('[app-mcp] 当前 AppMcp 实现不支持导航（Electron / Tauri 页面侧），bindVueRouter 未生效')
    return () => {}
  }
  const handler = async (request: NavigationRequest): Promise<void> => {
    const { page, params } = request
    const name = pages ? pages[page] : page
    if (name === undefined || !router.hasRoute(name)) {
      throw ToolCallError.navigationFailed(`页面「${page}」不存在（没有对应的命名路由）`)
    }
    const denial = guard ? guardDenial(guard(request), page) : undefined
    if (denial !== undefined) throw ToolCallError.navigationDenied(denial)
    const record = router.getRoutes().find((r) => r.name === name)
    const result = await router.push({ name, ...splitParams(record?.path, params) })
    const type = failureType(result)
    if (type === undefined || type & FAILURE_DUPLICATED) return
    if (type & FAILURE_ABORTED) throw ToolCallError.navigationDenied(`页面「${page}」的路由守卫拒绝了本次导航`)
    if (type & FAILURE_CANCELLED) throw ToolCallError.navigationFailed(`到页面「${page}」的导航被新的导航取消`)
    throw ToolCallError.navigationFailed(`到页面「${page}」的导航失败`)
  }
  appMcp.setNavigationHandler(handler, navOptions)
  let bound = true
  return () => {
    if (!bound) return
    bound = false
    appMcp.setNavigationHandler?.(null)
  }
}
