/**
 * 页面名 → 路由地址（路由适配共用，框架无关）：按路由模式（`/orders/:id`）填入导航参数，未用到的参数放进查询串。
 *
 * 支持的模式语法（React Router 与 Vue Router 的公共子集）：静态段、`:name`、可选 `:name?`、重复 `:name+` / `:name*`
 * （值为数组时以 `/` 连接）、Vue 的自定义正则 `:id(\\d+)`（只取参数名，不校验）、React Router 的通配 `*`（取参数 `*`）。
 */

import { ToolCallError } from '../types'

/** 结构化的路由对象（React Router `RouteObject` 的子集）：带 `id` 的路由是一个页面。 */
export interface RouteLike {
  id?: string
  path?: string
  index?: boolean
  children?: readonly RouteLike[]
}

const PARAM_SEGMENT = /^:([A-Za-z_$][\w$]*)(\([^)]*\))?([?+*])?$/

type ParamValue = string | number | boolean | readonly (string | number | boolean)[]

function encodeValue(value: unknown): string {
  return typeof value === 'string' ? value : typeof value === 'object' ? JSON.stringify(value) : String(value)
}

function encodeSegment(name: string, value: unknown, page: string): string {
  if (Array.isArray(value)) return value.map((v) => encodeURIComponent(encodeValue(v))).join('/')
  if (value !== null && typeof value === 'object') {
    throw ToolCallError.navigationFailed(`页面「${page}」的参数 ${name} 应为字符串、数字或数组`)
  }
  return encodeURIComponent(encodeValue(value))
}

/**
 * 按模式生成地址。
 *
 * @input pattern 路由模式；params 导航参数（缺省无参数）。
 * @output 地址（以 `/` 开头时保持），未出现在模式中的参数作为查询串。
 * @error 缺少必填参数、参数类型不支持 → `NAVIGATION_FAILED`（{@link ToolCallError.navigationFailed}）。
 */
export function pagePath(pattern: string, params: Record<string, unknown> = {}, page = pattern): string {
  const used = new Set<string>()
  const segments: string[] = []
  for (const segment of pattern.split('/')) {
    if (segment === '*') {
      used.add('*')
      const rest = params['*']
      if (rest !== undefined && rest !== null) segments.push(encodeValue(rest).split('/').map(encodeURIComponent).join('/'))
      continue
    }
    const m = PARAM_SEGMENT.exec(segment)
    if (!m) {
      segments.push(segment)
      continue
    }
    const name = m[1] as string
    const modifier = m[3]
    used.add(name)
    const value = params[name] as ParamValue | null | undefined
    const optional = modifier === '?' || modifier === '*'
    if (value === undefined || value === null || (Array.isArray(value) && value.length === 0)) {
      if (optional) continue
      throw ToolCallError.navigationFailed(`页面「${page}」缺少导航参数 ${name}`)
    }
    segments.push(encodeSegment(name, value, page))
  }
  let path = segments.join('/')
  if (pattern.startsWith('/') && !path.startsWith('/')) path = `/${path}`
  if (path === '') path = '/'
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (used.has(key) || value === undefined || value === null) continue
    query.append(key, encodeValue(value))
  }
  const qs = query.toString()
  return qs ? `${path}?${qs}` : path
}

function joinPath(parent: string, child: string | undefined): string {
  if (child === undefined || child === '') return parent || '/'
  if (child.startsWith('/')) return child
  const base = parent.endsWith('/') ? parent.slice(0, -1) : parent
  return `${base}/${child}`
}

/**
 * 从路由表（React Router 的 `RouteObject[]`）取页面：带 `id` 的路由 → `{ [id]: 完整路由模式 }`（子路由拼接父路径，
 * index 路由取父路径）。没有 `id` 的路由不是页面（不猜）。
 *
 * @error 同一 `id` 出现两次时抛错。
 */
export function routePages(routes: readonly RouteLike[], parent = ''): Record<string, string> {
  const pages: Record<string, string> = {}
  const visit = (list: readonly RouteLike[], base: string): void => {
    for (const route of list) {
      const full = route.index ? base || '/' : joinPath(base, route.path)
      if (route.id !== undefined) {
        if (route.id in pages) throw new Error(`路由表中页面 id ${JSON.stringify(route.id)} 重复`)
        pages[route.id] = full
      }
      if (route.children) visit(route.children, full)
    }
  }
  visit(routes, parent)
  return pages
}
