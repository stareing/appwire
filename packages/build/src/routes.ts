/**
 * 路由扫描（第 4c 项 C）：从路由表模块（React Router / Vue Router）静态得到页面目录（清单 `pages`，spec/manifest.md 2.3）。
 *
 * 规则（只认静态可确定的写法，不猜）：
 * - **路由表**：元素都是对象字面量、且至少一项带 `path` 的数组字面量（`createBrowserRouter([...])`、`createRouter({ routes: [...] })`、
 *   `export const routes = [...]` 等）；`children` 递归，子路径拼接父路径，React Router 的 index 路由取父路径。
 * - **页面**：React Router 中带 `id` 的路由、Vue Router 中带 `name` 的路由；页面名必须是字符串字面量。
 *   页面说明写在 React Router 的 `handle: { mcp: {...} }` / Vue Router 的 `meta: { mcp: {...} }`（`title`、`description`、
 *   `navigable`、`activation`、`params`），必须是字面量。
 * - **页面组件**：React Router 的 `element: <X />` / `Component: X` / `lazy: () => import('./x')`，Vue Router 的
 *   `component: X` / `component: () => import('./x.vue')`；`X` 必须从相对路径导入。只扫描该组件模块本身（不跟随它导入的
 *   子组件——对话框等子组件的工具只在打开时存在，不能作为导航目标）。
 * - **页面工具**：组件模块中的 `useTool('名称', { ... })` 与 `<x>.tool('名称', { ... })`；名称、`description`、`title`、`risk`、
 *   `activation`、`surface`、`page`、`annotations`、`implements`、`cache`、`input`、`outputSchema` 必须是字面量（`input` / `outputSchema` 为 JSON Schema
 *   字面量；zod 等运行时 schema 无法静态确定）。`handler` / `load` / `enabled` / `anchor` / `visibility` 是运行时字段，忽略。
 *
 * 不满足以上规则的页面给出错误，要求用 `definePage()`（插件 `pages` 模块）显式声明；显式声明的页面不再扫描（`skip`）。
 *
 * 本模块依赖 `typescript`（可选 peer 依赖），插件只在配置了 `routes` 时动态加载它。
 */
import { existsSync, readFileSync } from 'node:fs'
import { dirname, extname, isAbsolute, relative, resolve } from 'node:path'
import ts from 'typescript'
import type { PageDefinition, StaticToolDefinition } from './define'

export type RouterKind = 'react-router' | 'vue-router'

export interface RouteScanOption {
  /** 路由表模块（相对 root）。 */
  file: string
  router: RouterKind
}

export interface RouteScanOptions {
  root: string
  routes: readonly RouteScanOption[]
  /** 已显式声明的页面名：只取路由，不扫描组件。 */
  skip?: ReadonlySet<string>
}

export interface ScannedPage {
  page: PageDefinition
  /** 路由所在文件（相对 root）。 */
  source: string
}

export interface RouteScanResult {
  pages: ScannedPage[]
  errors: string[]
  /** 读取过的文件（绝对路径，供 dev 监听）。 */
  dependencies: string[]
}

// ---------------------------------------------------------------------------
// 源文件
// ---------------------------------------------------------------------------

interface Source {
  file: string
  sf: ts.SourceFile
  /** `.vue` 中 script 块之前的行数（错误位置换算）。 */
  lineOffset: number
}

const SCRIPT_BLOCK = /<script\b([^>]*)>([\s\S]*?)<\/script>/
const RESOLVE_SUFFIXES = ['', '.tsx', '.ts', '.jsx', '.js', '.vue', '/index.tsx', '/index.ts', '/index.jsx', '/index.js']

function scriptKind(file: string, lang?: string): ts.ScriptKind {
  const ext = lang ? `.${lang}` : extname(file)
  if (ext === '.tsx') return ts.ScriptKind.TSX
  if (ext === '.jsx') return ts.ScriptKind.JSX
  if (ext === '.js' || ext === '.mjs') return ts.ScriptKind.JS
  return ts.ScriptKind.TS
}

function readSource(file: string): Source {
  const text = readFileSync(file, 'utf8')
  if (extname(file) === '.vue') {
    const m = SCRIPT_BLOCK.exec(text)
    const code = m?.[2] ?? ''
    const lang = /\blang=["'](\w+)["']/.exec(m?.[1] ?? '')?.[1]
    const lineOffset = m ? text.slice(0, m.index + m[0].indexOf('>') + 1).split('\n').length - 1 : 0
    return { file, sf: ts.createSourceFile(file, code, ts.ScriptTarget.Latest, true, scriptKind(file, lang ?? 'ts')), lineOffset }
  }
  return { file, sf: ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, scriptKind(file)), lineOffset: 0 }
}

// ---------------------------------------------------------------------------
// 字面量求值
// ---------------------------------------------------------------------------

type Json = string | number | boolean | null | Json[] | { [key: string]: Json }
const NOT_STATIC = Symbol('not-static')
type Static = Json | typeof NOT_STATIC

function unwrap(expr: ts.Expression): ts.Expression {
  let e = expr
  while (ts.isAsExpression(e) || ts.isSatisfiesExpression(e) || ts.isParenthesizedExpression(e) || ts.isTypeAssertionExpression(e)) {
    e = e.expression
  }
  return e
}

function propertyName(name: ts.PropertyName): string | undefined {
  if (ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name)) return name.text
  return undefined
}

/** 字面量表达式的值；含变量、调用、展开、模板插值等时为 {@link NOT_STATIC}。 */
function staticValue(expr: ts.Expression): Static {
  const e = unwrap(expr)
  if (ts.isStringLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e)) return e.text
  if (ts.isNumericLiteral(e)) return Number(e.text)
  if (e.kind === ts.SyntaxKind.TrueKeyword) return true
  if (e.kind === ts.SyntaxKind.FalseKeyword) return false
  if (e.kind === ts.SyntaxKind.NullKeyword) return null
  if (ts.isPrefixUnaryExpression(e) && e.operator === ts.SyntaxKind.MinusToken && ts.isNumericLiteral(e.operand)) {
    return -Number(e.operand.text)
  }
  if (ts.isArrayLiteralExpression(e)) {
    const out: Json[] = []
    for (const el of e.elements) {
      if (ts.isSpreadElement(el)) return NOT_STATIC
      const v = staticValue(el)
      if (v === NOT_STATIC) return NOT_STATIC
      out.push(v)
    }
    return out
  }
  if (ts.isObjectLiteralExpression(e)) {
    const out: { [key: string]: Json } = {}
    for (const p of e.properties) {
      if (!ts.isPropertyAssignment(p)) return NOT_STATIC
      const key = propertyName(p.name)
      if (key === undefined) return NOT_STATIC
      const v = staticValue(p.initializer)
      if (v === NOT_STATIC) return NOT_STATIC
      out[key] = v
    }
    return out
  }
  return NOT_STATIC
}

/** 对象字面量的属性（键 → 节点）；含展开 / 计算属性名时返回 undefined。 */
function objectProps(obj: ts.ObjectLiteralExpression): Map<string, ts.ObjectLiteralElementLike> | undefined {
  const props = new Map<string, ts.ObjectLiteralElementLike>()
  for (const p of obj.properties) {
    if (ts.isSpreadAssignment(p)) return undefined
    const key = p.name ? propertyName(p.name) : undefined
    if (key === undefined) return undefined
    props.set(key, p)
  }
  return props
}

function initializer(p: ts.ObjectLiteralElementLike | undefined): ts.Expression | undefined {
  if (!p) return undefined
  if (ts.isPropertyAssignment(p)) return p.initializer
  if (ts.isShorthandPropertyAssignment(p)) return p.name
  return undefined
}

// ---------------------------------------------------------------------------
// 扫描器
// ---------------------------------------------------------------------------

/** 工具定义中由构建期读取的字段（其余为运行时字段，忽略）。 */
const TOOL_META_KEYS = ['title', 'risk', 'activation', 'surface', 'page', 'annotations', 'outputSchema', 'implements', 'cache'] as const
const PAGE_META_KEYS = ['title', 'description', 'navigable', 'activation', 'params'] as const

interface RouteContext {
  base: string | null
  source: Source
  router: RouterKind
}

class Scanner {
  readonly pages: ScannedPage[] = []
  readonly errors: string[] = []
  readonly dependencies = new Set<string>()

  constructor(
    private readonly root: string,
    private readonly skip: ReadonlySet<string>,
  ) {}

  private where(source: Source, node: ts.Node): string {
    const { line } = source.sf.getLineAndCharacterOfPosition(node.getStart(source.sf))
    return `${relative(this.root, source.file) || source.file}:${line + 1 + source.lineOffset}`
  }

  private fail(source: Source, node: ts.Node, message: string): void {
    this.errors.push(`${this.where(source, node)} ${message}`)
  }

  scanRouteFile(option: RouteScanOption): void {
    const file = isAbsolute(option.file) ? option.file : resolve(this.root, option.file)
    if (!existsSync(file)) {
      this.errors.push(`路由表模块不存在：${option.file}`)
      return
    }
    this.dependencies.add(file)
    const source = readSource(file)
    const consumed = new Set<ts.Node>()
    const visit = (node: ts.Node): void => {
      if (ts.isArrayLiteralExpression(node) && !consumed.has(node) && this.looksLikeRouteTable(node)) {
        this.routeList(node, { base: '', source, router: option.router }, consumed)
        return
      }
      ts.forEachChild(node, visit)
    }
    visit(source.sf)
  }

  /** 至少一个元素是带 `path` 的对象字面量。 */
  private looksLikeRouteTable(arr: ts.ArrayLiteralExpression): boolean {
    return arr.elements.some((el) => {
      const e = unwrap(el as ts.Expression)
      return ts.isObjectLiteralExpression(e) && e.properties.some((p) => p.name && propertyName(p.name) === 'path')
    })
  }

  private routeList(arr: ts.ArrayLiteralExpression, ctx: RouteContext, consumed: Set<ts.Node>): void {
    consumed.add(arr)
    for (const el of arr.elements) {
      const e = ts.isSpreadElement(el) ? el : unwrap(el)
      if (!ts.isObjectLiteralExpression(e)) {
        this.fail(ctx.source, el, '路由表中有无法静态确定的条目（变量 / 展开 / 调用）；其中的页面请用 definePage() 显式声明')
        continue
      }
      this.route(e, ctx, consumed)
    }
  }

  private route(obj: ts.ObjectLiteralExpression, ctx: RouteContext, consumed: Set<ts.Node>): void {
    const props = objectProps(obj)
    if (!props) {
      this.fail(ctx.source, obj, '路由对象含展开或计算属性名，无法静态确定；其中的页面请用 definePage() 显式声明')
      return
    }
    const fullPath = this.routePath(props, ctx)
    const pageKey = ctx.router === 'react-router' ? 'id' : 'name'
    const nameExpr = initializer(props.get(pageKey))
    if (nameExpr) {
      const name = staticValue(nameExpr)
      if (typeof name !== 'string') this.fail(ctx.source, nameExpr, `路由的 ${pageKey} 不是字符串字面量，无法作为页面名；请用 definePage() 显式声明`)
      else this.page(name, props, fullPath, ctx, nameExpr)
    }
    const children = initializer(props.get('children'))
    if (children) {
      const list = unwrap(children)
      if (ts.isArrayLiteralExpression(list)) this.routeList(list, { ...ctx, base: fullPath }, consumed)
      else this.fail(ctx.source, children, 'children 不是数组字面量，其中的页面无法静态确定；请用 definePage() 显式声明')
    }
  }

  /** 完整路由模式；`path` 不是字面量时为 null（其下页面的 route 无法确定）。 */
  private routePath(props: Map<string, ts.ObjectLiteralElementLike>, ctx: RouteContext): string | null {
    if (ctx.base === null) return null
    const index = initializer(props.get('index'))
    if (index && staticValue(index) === true) return ctx.base || '/'
    const pathExpr = initializer(props.get('path'))
    if (!pathExpr) return ctx.base || '/'
    const path = staticValue(pathExpr)
    if (typeof path !== 'string') return null
    if (path.startsWith('/') || ctx.base === '') return path || '/'
    return `${ctx.base.endsWith('/') ? ctx.base.slice(0, -1) : ctx.base}/${path}`
  }

  private page(
    name: string,
    props: Map<string, ts.ObjectLiteralElementLike>,
    route: string | null,
    ctx: RouteContext,
    at: ts.Node,
  ): void {
    const page: PageDefinition = { name }
    if (route !== null) page.route = route
    const source = relative(this.root, ctx.source.file) || ctx.source.file
    if (this.skip.has(name)) {
      this.pages.push({ page, source })
      return
    }
    if (route === null) {
      this.fail(ctx.source, at, `页面 ${name} 的路由路径不是字面量；请用 definePage() 显式声明`)
      return
    }
    if (!this.pageMeta(page, props, ctx)) return
    const component = this.componentModule(name, props, ctx)
    if (component === false) return
    if (component !== undefined) {
      const tools = this.pageTools(name, component)
      if (tools === undefined) return
      if (tools.length > 0) page.tools = tools
    }
    this.pages.push({ page, source })
  }

  /** `handle.mcp`（React Router）/ `meta.mcp`（Vue Router）中的页面说明。 */
  private pageMeta(page: PageDefinition, props: Map<string, ts.ObjectLiteralElementLike>, ctx: RouteContext): boolean {
    const holder = initializer(props.get(ctx.router === 'react-router' ? 'handle' : 'meta'))
    if (!holder) return true
    const holderObj = unwrap(holder)
    if (!ts.isObjectLiteralExpression(holderObj)) return true
    const mcpExpr = initializer(objectProps(holderObj)?.get('mcp'))
    if (!mcpExpr) return true
    const meta = staticValue(mcpExpr)
    if (meta === NOT_STATIC || meta === null || typeof meta !== 'object' || Array.isArray(meta)) {
      this.fail(ctx.source, mcpExpr, `页面 ${page.name} 的 mcp 说明不是字面量对象；请用 definePage() 显式声明`)
      return false
    }
    for (const key of PAGE_META_KEYS) {
      if (meta[key] !== undefined) (page as unknown as Record<string, unknown>)[key] = meta[key]
    }
    return true
  }

  /**
   * 页面组件所在模块（绝对路径）；没有组件时 undefined，无法确定时记录错误并返回 false。
   */
  private componentModule(page: string, props: Map<string, ts.ObjectLiteralElementLike>, ctx: RouteContext): string | undefined | false {
    const keys = ctx.router === 'react-router' ? ['element', 'Component', 'lazy'] : ['component', 'components']
    for (const key of keys) {
      const expr = initializer(props.get(key))
      if (!expr) continue
      const target = this.componentTarget(unwrap(expr), key)
      if (target === undefined) {
        this.fail(ctx.source, expr, `无法静态确定页面 ${page} 的组件（${key}）；请用 definePage() 显式声明该页面`)
        return false
      }
      return this.resolveComponent(page, target, ctx, expr)
    }
    return undefined
  }

  /** 组件表达式 → 导入标识符或动态导入的模块说明符。 */
  private componentTarget(expr: ts.Expression, key: string): { identifier: string } | { specifier: string } | undefined {
    if (ts.isJsxSelfClosingElement(expr) || ts.isJsxElement(expr)) {
      const tag = ts.isJsxElement(expr) ? expr.openingElement.tagName : expr.tagName
      return ts.isIdentifier(tag) ? { identifier: tag.text } : undefined
    }
    if (ts.isIdentifier(expr)) return { identifier: expr.text }
    if (ts.isArrowFunction(expr) || ts.isFunctionExpression(expr)) {
      const body = ts.isBlock(expr.body)
        ? expr.body.statements.find(ts.isReturnStatement)?.expression
        : (expr.body as ts.Expression)
      const call = body ? unwrap(body) : undefined
      if (call && ts.isCallExpression(call) && call.expression.kind === ts.SyntaxKind.ImportKeyword) {
        const arg = call.arguments[0]
        return arg && ts.isStringLiteral(arg) ? { specifier: arg.text } : undefined
      }
      return undefined
    }
    if (key === 'components' && ts.isObjectLiteralExpression(expr)) {
      const def = initializer(objectProps(expr)?.get('default'))
      return def ? this.componentTarget(unwrap(def), 'component') : undefined
    }
    return undefined
  }

  private resolveComponent(
    page: string,
    target: { identifier: string } | { specifier: string },
    ctx: RouteContext,
    at: ts.Node,
  ): string | false {
    let specifier: string | undefined
    if ('specifier' in target) specifier = target.specifier
    else {
      specifier = importSpecifier(ctx.source.sf, target.identifier)
      if (specifier === undefined) {
        this.fail(
          ctx.source,
          at,
          `页面 ${page} 的组件 ${target.identifier} 不是从其他模块导入的（定义在路由文件中），无法确定其工具；请把组件放到独立模块或用 definePage() 显式声明`,
        )
        return false
      }
    }
    if (!specifier.startsWith('.')) {
      this.fail(ctx.source, at, `页面 ${page} 的组件来自包 ${specifier}，不扫描；请用 definePage() 显式声明该页面`)
      return false
    }
    const file = resolveModule(dirname(ctx.source.file), specifier)
    if (!file) {
      this.fail(ctx.source, at, `找不到页面 ${page} 的组件模块 ${specifier}`)
      return false
    }
    return file
  }

  /** 组件模块中的工具；有无法静态确定的工具时记录错误并返回 undefined。 */
  private pageTools(page: string, file: string): StaticToolDefinition<any>[] | undefined {
    this.dependencies.add(file)
    const source = readSource(file)
    const tools: StaticToolDefinition<any>[] = []
    const before = this.errors.length
    const visit = (node: ts.Node): void => {
      if (ts.isCallExpression(node) && isToolCall(node)) {
        const tool = this.tool(page, node, source)
        if (tool) {
          if (tools.some((t) => t.name === tool.name)) this.fail(source, node, `页面 ${page} 中工具 ${tool.name} 重复注册`)
          else tools.push(tool)
        }
      }
      ts.forEachChild(node, visit)
    }
    visit(source.sf)
    return this.errors.length > before ? undefined : tools
  }

  private tool(page: string, call: ts.CallExpression, source: Source): StaticToolDefinition<any> | undefined {
    const [nameArg, defArg] = call.arguments
    const name = nameArg ? staticValue(nameArg) : NOT_STATIC
    const hint = `；请用 definePage() 显式声明页面 ${page}`
    if (typeof name !== 'string') {
      this.fail(source, call, `工具名不是字符串字面量${hint}`)
      return undefined
    }
    const def = defArg ? unwrap(defArg) : undefined
    if (!def || !ts.isObjectLiteralExpression(def)) {
      this.fail(source, call, `工具 ${name} 的定义不是对象字面量${hint}`)
      return undefined
    }
    const props = objectProps(def)
    if (!props) {
      this.fail(source, def, `工具 ${name} 的定义含展开或计算属性名${hint}`)
      return undefined
    }
    const description = staticValue(initializer(props.get('description')) ?? def)
    if (typeof description !== 'string') {
      this.fail(source, def, `工具 ${name} 的 description 不是字符串字面量${hint}`)
      return undefined
    }
    const tool: StaticToolDefinition<any> = { name, description }
    const inputExpr = initializer(props.get('input'))
    if (inputExpr) {
      const input = staticValue(inputExpr)
      if (input === NOT_STATIC || input === null || typeof input !== 'object' || Array.isArray(input)) {
        this.fail(source, inputExpr, `工具 ${name} 的 input 不是 JSON Schema 字面量（zod 等运行时 schema 无法静态确定）${hint}`)
        return undefined
      }
      tool.input = input as StaticToolDefinition['input']
    }
    for (const key of TOOL_META_KEYS) {
      const expr = initializer(props.get(key))
      if (!expr) continue
      const value = staticValue(expr)
      if (value === NOT_STATIC) {
        this.fail(source, expr, `工具 ${name} 的 ${key} 不是字面量${hint}`)
        return undefined
      }
      ;(tool as unknown as Record<string, unknown>)[key] = value
    }
    if (tool.page !== undefined && tool.page !== page) {
      this.fail(source, def, `工具 ${name} 声明的 page "${tool.page}" 与所在路由页面 "${page}" 不一致`)
      return undefined
    }
    return tool
  }
}

/** `useTool('x', {...})` 或 `<expr>.tool('x', {...})`。 */
function isToolCall(call: ts.CallExpression): boolean {
  if (call.arguments.length < 2) return false
  const callee = call.expression
  if (ts.isIdentifier(callee)) return callee.text === 'useTool'
  return ts.isPropertyAccessExpression(callee) && callee.name.text === 'tool' && ts.isStringLiteralLike(call.arguments[0]!)
}

/** 标识符的导入来源（默认导入、具名导入、命名空间导入）；不是导入的返回 undefined。 */
function importSpecifier(sf: ts.SourceFile, identifier: string): string | undefined {
  for (const stmt of sf.statements) {
    if (!ts.isImportDeclaration(stmt) || !ts.isStringLiteral(stmt.moduleSpecifier) || !stmt.importClause) continue
    const clause = stmt.importClause
    const named = clause.namedBindings
    const matches =
      clause.name?.text === identifier ||
      (named !== undefined &&
        (ts.isNamespaceImport(named)
          ? named.name.text === identifier
          : named.elements.some((e) => e.name.text === identifier)))
    if (matches) return stmt.moduleSpecifier.text
  }
  return undefined
}

function resolveModule(dir: string, specifier: string): string | undefined {
  const base = resolve(dir, specifier)
  for (const suffix of RESOLVE_SUFFIXES) {
    const candidate = base + suffix
    if (existsSync(candidate) && extname(candidate) !== '') return candidate
  }
  // TypeScript 风格：'./x.js' 指向 './x.ts'
  if (/\.(m?js|jsx)$/.test(base)) {
    for (const ext of ['.ts', '.tsx']) {
      const candidate = base.replace(/\.(m?js|jsx)$/, ext)
      if (existsSync(candidate)) return candidate
    }
  }
  return undefined
}

/** 扫描路由表模块，得到页面目录。 */
export function scanRoutes(options: RouteScanOptions): RouteScanResult {
  const scanner = new Scanner(options.root, options.skip ?? new Set())
  for (const route of options.routes) scanner.scanRouteFile(route)
  const seen = new Map<string, string>()
  for (const { page, source } of scanner.pages) {
    const prev = seen.get(page.name)
    if (prev !== undefined) scanner.errors.push(`页面名 ${page.name} 重复（${prev}、${source}）`)
    else seen.set(page.name, source)
  }
  return { pages: scanner.pages, errors: scanner.errors, dependencies: [...scanner.dependencies] }
}
