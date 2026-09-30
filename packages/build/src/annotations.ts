/**
 * 编译期注释工具：扫描源文件中带 `@mcp` JSDoc 标签的导出函数，用 TypeScript 编译器 API
 * 把参数类型转换为 JSON Schema，并生成虚拟模块 `virtual:app-mcp/annotated` 的代码。
 *
 * 注释语法：
 * ```ts
 * /**
 *  * 结算当前购物车            ← 正文作为描述
 *  * @mcp checkout             ← 工具名（局部名，见下），其后可接描述
 *  * @risk payment
 *  * @activation foreground
 *  * @title 结算
 *  * @param addressId 收货地址 ID
 *  *\/
 * export async function checkout(addressId: string, note?: string) { ... }
 * ```
 *
 * `@mcp` 后首个词在以下情况作为工具名，否则整段为描述、工具名取 `<文件名>.<导出名>`：
 * 词符合名称规则，且（含 `.`，或 JSDoc 正文已给出描述）。
 *
 * 工具名是 App 内的**局部名**（spec/protocol.md 3.1）：Host 对外暴露为 `<appId>.<局部名>`。
 * 给出 `appId` 时，写成带 `<appId>.` 前缀的全名（如 App `shop` 中的 `@mcp shop.info`）会报错并提示改为局部名。
 *
 * 本模块依赖 `typescript`（可选 peer 依赖），插件只在启用 `annotations` 时动态加载它。
 */
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { basename, dirname, isAbsolute, relative, resolve } from 'node:path'
import ts from 'typescript'
import type { Activation, Risk } from '@app-mcp/web'
import { NAME_PATTERN, appIdPrefixMessage } from './manifest'

// ---------------------------------------------------------------------------
// 类型
// ---------------------------------------------------------------------------

export const ANNOTATED_MODULE_ID = 'virtual:app-mcp/annotated'
export const DEFAULT_ANNOTATION_INCLUDE = ['src/**/*.{ts,tsx,mts,cts}']
export const DEFAULT_ANNOTATION_EXCLUDE = [
  '**/*.test.*',
  '**/*.spec.*',
  '**/__tests__/**',
  '**/*.d.ts',
  '**/node_modules/**',
]
/** 对象类型的最大嵌套深度，超过后该处退化为 `{}`（任意值）并给出警告。 */
export const MAX_SCHEMA_DEPTH = 5

export interface AnnotationScanOptions {
  /** 项目根目录（Vite root），glob 相对于它。 */
  root: string
  /** 扫描的文件（glob，相对 root），默认 `src/**\/*.{ts,tsx,mts,cts}`。 */
  include?: string[]
  /** 排除的文件（glob，相对 root），默认排除测试文件、声明文件与 node_modules。 */
  exclude?: string[]
  /** tsconfig 路径（相对 root）。缺省使用 root 下的 tsconfig.json，不存在时用内置默认选项。 */
  tsconfig?: string
  /** App ID。给出时，工具名以 `<appId>.` 开头视为误写的全名并报错（工具名应为局部名）。 */
  appId?: string
}

/** 调用方式：无参数、单个对象参数直接传入、多个位置参数按名称从输入对象中取出。 */
export type AnnotatedCall =
  | { mode: 'none'; context: boolean }
  | { mode: 'object'; context: boolean }
  | { mode: 'positional'; params: string[]; context: boolean }

export interface AnnotatedTool {
  name: string
  description: string
  title?: string
  risk?: Risk
  activation?: Activation
  inputSchema: { type: 'object'; [key: string]: unknown }
  /** 源文件绝对路径（`/` 分隔）。 */
  file: string
  /** 源文件相对 root 的路径。 */
  source: string
  /** 模块导出名（函数、常量或类）。 */
  exportName: string
  /** 类的静态方法名。 */
  member?: string
  call: AnnotatedCall
}

export interface AnnotationScanResult {
  tools: AnnotatedTool[]
  /** 被跳过的工具与其他提示。 */
  warnings: string[]
  /** 致命错误（如工具重名），存在时不应生成清单。 */
  errors: string[]
  /** 影响扫描结果的项目文件（绝对路径，`/` 分隔），用于 dev 下监听。 */
  dependencies: string[]
}

const RISKS: readonly string[] = ['read', 'write', 'destructive', 'payment', 'os-sensitive']
const ACTIVATIONS: readonly string[] = ['headless', 'background', 'foreground']
const MCP_TAG = /@mcp\b/

function slash(path: string): string {
  return path.replace(/\\/g, '/')
}

// ---------------------------------------------------------------------------
// glob
// ---------------------------------------------------------------------------

/** 把 glob 转为正则：支持 `**`、`*`、`?`、`{a,b}`。 */
export function globToRegExp(glob: string): RegExp {
  let re = ''
  let i = 0
  let braces = 0
  const src = slash(glob).replace(/^\.\//, '')
  while (i < src.length) {
    const c = src[i]!
    if (c === '*') {
      if (src[i + 1] === '*') {
        const atSegmentStart = i === 0 || src[i - 1] === '/'
        if (src[i + 2] === '/' && atSegmentStart) {
          re += '(?:.*/)?'
          i += 3
        } else {
          re += '.*'
          i += 2
        }
        continue
      }
      re += '[^/]*'
    } else if (c === '?') {
      re += '[^/]'
    } else if (c === '{') {
      braces++
      re += '(?:'
    } else if (c === '}' && braces > 0) {
      braces--
      re += ')'
    } else if (c === ',' && braces > 0) {
      re += '|'
    } else {
      re += c.replace(/[.+^$()|[\]\\]/g, '\\$&')
    }
    i++
  }
  return new RegExp(`^${re}$`)
}

/** glob 中不含通配符的前缀目录（用于缩小遍历范围）。 */
function globBase(glob: string): string {
  const parts = slash(glob).replace(/^\.\//, '').split('/')
  const base: string[] = []
  for (const part of parts.slice(0, -1)) {
    if (/[*?{}[\]]/.test(part)) break
    base.push(part)
  }
  return base.join('/')
}

class FileMatcher {
  private readonly include: RegExp[]
  private readonly exclude: RegExp[]
  private readonly bases: string[]
  constructor(
    private readonly root: string,
    include: string[],
    exclude: string[],
  ) {
    this.include = include.map(globToRegExp)
    this.exclude = exclude.map(globToRegExp)
    this.bases = [...new Set(include.map(globBase))]
  }

  matches(file: string): boolean {
    const rel = slash(relative(this.root, file))
    if (rel.startsWith('../') || isAbsolute(rel)) return false
    return this.include.some((r) => r.test(rel)) && !this.exclude.some((r) => r.test(rel))
  }

  list(): string[] {
    const found = new Set<string>()
    const walk = (dir: string) => {
      let entries
      try {
        entries = readdirSync(dir, { withFileTypes: true })
      } catch {
        return
      }
      for (const entry of entries) {
        const full = resolve(dir, entry.name)
        if (entry.isDirectory()) {
          if (entry.name === 'node_modules' || entry.name.startsWith('.')) continue
          walk(full)
        } else if (entry.isFile() && this.matches(full)) {
          found.add(slash(full))
        }
      }
    }
    for (const base of this.bases) walk(resolve(this.root, base))
    return [...found].sort()
  }
}

// ---------------------------------------------------------------------------
// 编译选项
// ---------------------------------------------------------------------------

const DEFAULT_COMPILER_OPTIONS: ts.CompilerOptions = {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  jsx: ts.JsxEmit.Preserve,
  strict: true,
  esModuleInterop: true,
  allowImportingTsExtensions: true,
  skipLibCheck: true,
  types: [],
}

function loadCompilerOptions(root: string, tsconfig: string | undefined, warnings: string[]): ts.CompilerOptions {
  const path = tsconfig ? resolve(root, tsconfig) : resolve(root, 'tsconfig.json')
  let options = DEFAULT_COMPILER_OPTIONS
  if (existsSync(path)) {
    const read = ts.readConfigFile(path, ts.sys.readFile)
    if (read.error) {
      warnings.push(`读取 ${slash(relative(root, path))} 失败，使用默认编译选项：${flatten(read.error.messageText)}`)
    } else {
      options = ts.parseJsonConfigFileContent(read.config, ts.sys, dirname(path), undefined, path).options
    }
  } else if (tsconfig) {
    warnings.push(`找不到 tsconfig ${tsconfig}，使用默认编译选项`)
  }
  // 可选参数 / 可选属性依赖 strictNullChecks 才能区分 undefined。
  return { ...options, strictNullChecks: true, noEmit: true, skipLibCheck: true }
}

function flatten(text: string | ts.DiagnosticMessageChain): string {
  return ts.flattenDiagnosticMessageText(text, '\n')
}

// ---------------------------------------------------------------------------
// JSDoc
// ---------------------------------------------------------------------------

interface ParsedDoc {
  body: string
  tags: { name: string; text: string; param?: string }[]
}

function entityName(name: ts.EntityName): string {
  return ts.isIdentifier(name) ? name.text : `${entityName(name.left)}.${name.right.text}`
}

function parseDoc(node: ts.Node): ParsedDoc | null {
  const docs = ts.getJSDocCommentsAndTags(node).filter(ts.isJSDoc)
  const doc = docs[docs.length - 1]
  if (!doc) return null
  const tags = (doc.tags ?? []).map((tag) => {
    const text = (ts.getTextOfJSDocComment(tag.comment) ?? '').trim()
    if (ts.isJSDocParameterTag(tag)) {
      return { name: 'param', text: text.replace(/^-\s*/, ''), param: entityName(tag.name) }
    }
    return { name: tag.tagName.text, text }
  })
  return { body: (ts.getTextOfJSDocComment(doc.comment) ?? '').trim(), tags }
}

// ---------------------------------------------------------------------------
// 类型 → JSON Schema
// ---------------------------------------------------------------------------

class UnsupportedType extends Error {}

type Schema = Record<string, unknown>

interface ConvertContext {
  checker: ts.TypeChecker
  warnings: string[]
  label: string
  stack: ts.Type[]
}

function includesUndefined(type: ts.Type): boolean {
  if (type.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Void)) return true
  return type.isUnion() && type.types.some((t) => (t.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Void)) !== 0)
}

/** 去掉联合中的 undefined / void；只剩一个成员时返回该成员。 */
function withoutUndefined(type: ts.Type): ts.Type | ts.Type[] {
  if (!type.isUnion()) return type
  const rest = type.types.filter((t) => (t.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Void)) === 0)
  return rest.length === 1 ? rest[0]! : rest
}

function typeName(ctx: ConvertContext, type: ts.Type): string {
  return ctx.checker.typeToString(type)
}

function isDateType(type: ts.Type): boolean {
  const symbol = type.getSymbol()
  if (!symbol || symbol.getName() !== 'Date') return false
  return (symbol.getDeclarations() ?? []).some((d) => d.getSourceFile().hasNoDefaultLib || /lib\.[^/]*\.d\.ts$/.test(d.getSourceFile().fileName))
}

function toSchema(type: ts.Type, ctx: ConvertContext, depth: number, where: string): Schema {
  const F = ts.TypeFlags
  const flags = type.flags
  if ((type as { intrinsicName?: string }).intrinsicName === 'error') {
    throw new UnsupportedType(`${where} 的类型无法解析`)
  }
  if (flags & (F.Any | F.Unknown)) return {}
  if (flags & F.TypeParameter) throw new UnsupportedType(`${where} 是泛型参数 ${typeName(ctx, type)}`)
  if (flags & F.ESSymbolLike) throw new UnsupportedType(`${where} 是 Symbol`)
  if (flags & F.BigIntLike) throw new UnsupportedType(`${where} 是 bigint`)
  if (flags & F.Never) throw new UnsupportedType(`${where} 是 never`)
  if (flags & (F.Undefined | F.Void)) throw new UnsupportedType(`${where} 只能是 undefined`)
  if (flags & F.Null) return { type: 'null' }
  if (flags & F.Boolean) return { type: 'boolean' }
  if (flags & F.BooleanLiteral) return { type: 'boolean', enum: [(type as { intrinsicName?: string }).intrinsicName === 'true'] }
  if (type.isStringLiteral()) return { type: 'string', enum: [type.value] }
  if (type.isNumberLiteral()) return { type: 'number', enum: [type.value] }
  if (flags & (F.String | F.TemplateLiteral | F.StringMapping)) return { type: 'string' }
  if (flags & F.Number) return { type: 'number' }
  if (type.isUnion()) return unionSchema(type.types, ctx, depth, where)
  if (flags & F.NonPrimitive) return { type: 'object' }
  if (flags & F.Object || type.isIntersection()) return objectSchema(type, ctx, depth, where)
  throw new UnsupportedType(`${where} 的类型 ${typeName(ctx, type)} 无法表示为 JSON Schema`)
}

function unionSchema(types: readonly ts.Type[], ctx: ConvertContext, depth: number, where: string): Schema {
  const strings: string[] = []
  const numbers: number[] = []
  const bools = new Set<boolean>()
  let hasNull = false
  const others: Schema[] = []
  for (const member of types) {
    if (member.flags & (ts.TypeFlags.Undefined | ts.TypeFlags.Void)) continue
    if (member.isStringLiteral()) strings.push(member.value)
    else if (member.isNumberLiteral()) numbers.push(member.value)
    else if (member.flags & ts.TypeFlags.BooleanLiteral) {
      bools.add((member as { intrinsicName?: string }).intrinsicName === 'true')
    } else if (member.flags & ts.TypeFlags.Null) hasNull = true
    else others.push(toSchema(member, ctx, depth, where))
  }
  const schemas: Schema[] = []
  if (strings.length > 0) schemas.push({ type: 'string', enum: strings })
  if (numbers.length > 0) schemas.push({ type: 'number', enum: numbers })
  if (bools.size === 2) schemas.push({ type: 'boolean' })
  else if (bools.size === 1) schemas.push({ type: 'boolean', enum: [...bools] })
  schemas.push(...others)
  if (hasNull) {
    const only = schemas.length === 1 ? schemas[0]! : null
    if (only && typeof only.type === 'string' && Object.keys(only).every((k) => k === 'type' || k === 'enum')) {
      const merged: Schema = { type: [only.type, 'null'] }
      if (Array.isArray(only.enum)) merged.enum = [...(only.enum as unknown[]), null]
      return merged
    }
    schemas.push({ type: 'null' })
  }
  if (schemas.length === 0) throw new UnsupportedType(`${where} 只能是 undefined`)
  return schemas.length === 1 ? schemas[0]! : { anyOf: schemas }
}

function objectSchema(type: ts.Type, ctx: ConvertContext, depth: number, where: string): Schema {
  const { checker } = ctx
  if (isDateType(type)) return { type: 'string', format: 'date-time' }
  if (type.getCallSignatures().length > 0 || type.getConstructSignatures().length > 0) {
    throw new UnsupportedType(`${where} 是函数类型 ${typeName(ctx, type)}`)
  }
  if (depth >= MAX_SCHEMA_DEPTH || ctx.stack.includes(type)) {
    ctx.warnings.push(
      `${ctx.label}：${where} 的类型 ${typeName(ctx, type)} 嵌套超过 ${MAX_SCHEMA_DEPTH} 层或递归引用自身，按任意值处理`,
    )
    return {}
  }
  ctx.stack.push(type)
  try {
    if (checker.isTupleType(type)) {
      const target = (type as ts.TypeReference).target as ts.TupleType
      const elements = checker.getTypeArguments(type as ts.TypeReference)
      const prefixItems: Schema[] = []
      let rest: Schema | undefined
      elements.forEach((element, i) => {
        const schema = toSchema(element, ctx, depth + 1, `${where}[${i}]`)
        if (target.elementFlags[i]! & ts.ElementFlags.Variable) rest = schema
        else prefixItems.push(schema)
      })
      const result: Schema = { type: 'array', prefixItems, minItems: target.minLength }
      if (rest) result.items = rest
      else result.maxItems = prefixItems.length
      return result
    }
    if (checker.isArrayType(type)) {
      const [element] = checker.getTypeArguments(type as ts.TypeReference)
      return { type: 'array', items: element ? toSchema(element, ctx, depth + 1, `${where}[]`) : {} }
    }
    const properties: Record<string, Schema> = {}
    const required: string[] = []
    for (const prop of checker.getPropertiesOfType(type)) {
      const name = prop.getName()
      if (name.startsWith('__@') || name.startsWith('#')) {
        throw new UnsupportedType(`${where} 的类型 ${typeName(ctx, type)} 含 Symbol 或私有成员`)
      }
      if (prop.flags & ts.SymbolFlags.Method) {
        throw new UnsupportedType(`${where} 的类型 ${typeName(ctx, type)} 含方法 ${name}`)
      }
      const propType = checker.getTypeOfSymbol(prop)
      const optional = (prop.flags & ts.SymbolFlags.Optional) !== 0 || includesUndefined(propType)
      const schema = toSchema(propType, ctx, depth + 1, `${where}.${name}`)
      const doc = ts.displayPartsToString(prop.getDocumentationComment(checker)).trim()
      properties[name] = doc ? { ...schema, description: doc } : schema
      if (!optional) required.push(name)
    }
    const result: Schema = { type: 'object', properties }
    if (required.length > 0) result.required = required
    const index = checker.getIndexInfosOfType(type).find((info) => !(info.keyType.flags & ts.TypeFlags.ESSymbolLike))
    if (index) result.additionalProperties = toSchema(index.type, ctx, depth + 1, `${where}[key]`)
    return result
  } finally {
    ctx.stack.pop()
  }
}

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

interface Candidate {
  /** 用于推导名称的函数名。 */
  localName: string
  /** JSDoc 所在节点。 */
  docNode: ts.Node
  /** 导出查找用的声明节点（函数、变量声明或类）。 */
  exportNode: ts.Declaration
  fn: ts.SignatureDeclaration
  member?: string
  /** 静态方法不是 static 等无法调用的原因。 */
  problem?: string
}

function collectCandidates(sourceFile: ts.SourceFile): Candidate[] {
  const result: Candidate[] = []
  const hasMcp = (node: ts.Node) => parseDoc(node)?.tags.some((t) => t.name === 'mcp') ?? false
  for (const stmt of sourceFile.statements) {
    if (ts.isFunctionDeclaration(stmt) && stmt.body) {
      if (!hasMcp(stmt)) continue
      result.push({ localName: stmt.name?.text ?? 'default', docNode: stmt, exportNode: stmt, fn: stmt })
    } else if (ts.isVariableStatement(stmt)) {
      const single = stmt.declarationList.declarations.length === 1
      for (const decl of stmt.declarationList.declarations) {
        if (!ts.isIdentifier(decl.name) || !decl.initializer) continue
        let init: ts.Expression = decl.initializer
        while (ts.isParenthesizedExpression(init) || ts.isAsExpression(init) || ts.isSatisfiesExpression(init)) {
          init = init.expression
        }
        if (!ts.isArrowFunction(init) && !ts.isFunctionExpression(init)) continue
        const docNode = single ? stmt : decl
        if (!hasMcp(docNode)) continue
        result.push({ localName: decl.name.text, docNode, exportNode: decl, fn: init })
      }
    } else if (ts.isClassDeclaration(stmt)) {
      for (const member of stmt.members) {
        if (!ts.isMethodDeclaration(member) || !hasMcp(member)) continue
        const name = member.name
        const localName = ts.isIdentifier(name) ? name.text : ts.isStringLiteral(name) ? name.text : null
        const isStatic = member.modifiers?.some((m) => m.kind === ts.SyntaxKind.StaticKeyword) ?? false
        const isPrivate = member.modifiers?.some(
          (m) => m.kind === ts.SyntaxKind.PrivateKeyword || m.kind === ts.SyntaxKind.ProtectedKeyword,
        )
        let problem: string | undefined
        if (localName === null) problem = '方法名必须是标识符'
        else if (!isStatic) problem = '只支持类的静态方法'
        else if (isPrivate) problem = '不能是 private / protected 方法'
        result.push({
          localName: localName ?? '?',
          docNode: member,
          exportNode: stmt,
          fn: member,
          member: localName ?? '?',
          ...(problem ? { problem } : {}),
        })
      }
    }
  }
  return result
}

/** 声明节点 → 导出名（支持 `export function`、`export const`、`export { a as b }`、`export default`）。 */
function exportNames(checker: ts.TypeChecker, sourceFile: ts.SourceFile): Map<ts.Node, string> {
  const map = new Map<ts.Node, string>()
  const moduleSymbol = checker.getSymbolAtLocation(sourceFile)
  if (!moduleSymbol) return map
  for (const exp of checker.getExportsOfModule(moduleSymbol)) {
    const target = exp.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(exp) : exp
    for (const decl of target.getDeclarations() ?? []) {
      if (decl.getSourceFile() !== sourceFile) continue
      const current = map.get(decl)
      // 同一声明有多个导出名时优先使用非 default 的名称。
      if (current === undefined || current === 'default') map.set(decl, exp.getName())
    }
  }
  return map
}

function moduleBaseName(file: string): string {
  const name = basename(file).replace(/\.(d\.)?[cm]?tsx?$/, '')
  return name === 'index' ? basename(dirname(file)) : name
}

function isToolContextType(type: ts.Type): boolean {
  const symbol = type.aliasSymbol ?? type.getSymbol()
  return symbol?.getName() === 'ToolContext'
}

function analyzeCandidate(
  candidate: Candidate,
  file: string,
  root: string,
  checker: ts.TypeChecker,
  exports: Map<ts.Node, string>,
  warnings: string[],
  errors: string[],
  appId: string | undefined,
): AnnotatedTool | null {
  const source = slash(relative(root, file))
  const doc = parseDoc(candidate.docNode)!
  const fnLabel = candidate.member ? `${candidate.localName}` : candidate.localName
  const where = `${source} 中的 ${candidate.member ? `静态方法 ${fnLabel}` : `函数 ${fnLabel}`}`
  const skip = (reason: string): null => {
    warnings.push(`注释工具：${where} ${reason}，已跳过`)
    return null
  }

  if (candidate.problem) return skip(candidate.problem)
  const exportName = exports.get(candidate.exportNode)
  if (exportName === undefined) return skip('未导出')

  // @mcp：名称与描述
  const mcpTags = doc.tags.filter((t) => t.name === 'mcp')
  if (mcpTags.length > 1) warnings.push(`注释工具：${where} 有多个 @mcp 标签，只使用第一个`)
  const mcpText = mcpTags[0]!.text
  const [first = '', ...rest] = mcpText.split(/\s+/)
  let name: string
  let mcpDescription: string
  if (NAME_PATTERN.test(first) && (first.includes('.') || doc.body !== '')) {
    name = first
    mcpDescription = mcpText.slice(first.length).trim()
  } else {
    if (candidate.localName === 'default') return skip('是匿名默认导出，需要在 @mcp 后写明工具名')
    name = `${moduleBaseName(file)}.${candidate.localName}`
    mcpDescription = [first, ...rest].join(' ').trim()
  }
  if (!NAME_PATTERN.test(name)) return skip(`的工具名 "${name}" 不合法（应满足 [a-zA-Z0-9_.-]{1,64}）`)
  const prefixed = appId === undefined ? null : appIdPrefixMessage(name, appId)
  if (prefixed) {
    errors.push(`注释工具：${where} 的 @mcp ${prefixed}`)
    return null
  }
  const description = doc.body || mcpDescription
  if (!description) return skip('缺少描述（写在 JSDoc 正文或 @mcp 名称之后）')

  const tool: Omit<AnnotatedTool, 'inputSchema' | 'call'> = {
    name,
    description,
    file: slash(file),
    source,
    exportName,
  }
  if (candidate.member) tool.member = candidate.member
  const tagText = (tagName: string) => doc.tags.find((t) => t.name === tagName)?.text
  const title = tagText('title')
  if (title) tool.title = title
  const risk = tagText('risk')
  if (risk !== undefined) {
    if (!RISKS.includes(risk)) return skip(`的 @risk "${risk}" 不合法（可选：${RISKS.join('、')}）`)
    tool.risk = risk as Risk
  }
  const activation = tagText('activation')
  if (activation !== undefined) {
    if (!ACTIVATIONS.includes(activation)) {
      return skip(`的 @activation "${activation}" 不合法（可选：${ACTIVATIONS.join('、')}）`)
    }
    tool.activation = activation as Activation
  }

  // 参数
  const paramDocs = new Map<string, string>()
  for (const tag of doc.tags) {
    if (tag.name === 'param' && tag.param && tag.text) paramDocs.set(tag.param, tag.text)
  }
  const signature = checker.getSignatureFromDeclaration(candidate.fn)
  if (!signature) return skip('无法解析函数签名')
  const ctx: ConvertContext = { checker, warnings, label: `${where}（工具 ${name}）`, stack: [] }
  const params = candidate.fn.parameters.map((decl, i) => {
    const symbol = signature.parameters[i]
    const type = symbol ? checker.getTypeOfSymbol(symbol) : checker.getTypeAtLocation(decl)
    return { decl, type, name: ts.isIdentifier(decl.name) ? decl.name.text : null }
  })
  let context = false
  const lastParam = params[params.length - 1]
  if (lastParam) {
    const nonUndefined = withoutUndefined(lastParam.type)
    if (!Array.isArray(nonUndefined) && isToolContextType(nonUndefined)) {
      context = true
      params.pop()
    }
  }

  try {
    if (params.some((p) => p.decl.dotDotDotToken)) throw new UnsupportedType('不支持剩余参数')
    if (params.length === 0) {
      return { ...tool, inputSchema: { type: 'object', properties: {} }, call: { mode: 'none', context } }
    }

    // 单个对象参数：直接展开为 schema。
    if (params.length === 1) {
      const param = params[0]!
      const inner = withoutUndefined(param.type)
      if (!Array.isArray(inner) && !isDateType(inner) && (inner.flags & (ts.TypeFlags.Object | ts.TypeFlags.Intersection))) {
        const schema = toSchema(inner, ctx, 0, param.name ?? '参数')
        if (schema.type === 'object') {
          applyPropertyDocs(schema, param.name, paramDocs)
          return {
            ...tool,
            inputSchema: schema as AnnotatedTool['inputSchema'],
            call: { mode: 'object', context },
          }
        }
      }
    }

    // 多个（或非对象的单个）位置参数：组成 { [参数名]: ... }。
    const properties: Record<string, Schema> = {}
    const required: string[] = []
    const names: string[] = []
    for (const param of params) {
      if (param.name === null) throw new UnsupportedType('多个参数时不支持解构参数，请使用具名参数')
      const optional =
        checker.isOptionalParameter(param.decl) || param.decl.initializer !== undefined || includesUndefined(param.type)
      let schema = toSchema(param.type, ctx, 1, param.name)
      const described = paramDocs.get(param.name)
      if (described) schema = { ...schema, description: described }
      if (schema.type === 'object') applyPropertyDocs(schema, param.name, paramDocs, false)
      properties[param.name] = schema
      names.push(param.name)
      if (!optional) required.push(param.name)
    }
    const inputSchema: AnnotatedTool['inputSchema'] = { type: 'object', properties }
    if (required.length > 0) inputSchema.required = required
    return { ...tool, inputSchema, call: { mode: 'positional', params: names, context } }
  } catch (err) {
    if (err instanceof UnsupportedType) return skip(`参数无法转换为 JSON Schema：${err.message}`)
    throw err
  }
}

/**
 * 把 `@param input.a 描述`（或解构参数的 `@param a 描述`）写入对象 schema 的属性描述；
 * `@param input 描述` 写入 schema 本身（仅 `withSelf`）。
 */
function applyPropertyDocs(
  schema: Schema,
  paramName: string | null,
  docs: Map<string, string>,
  withSelf = true,
): void {
  const properties = schema.properties as Record<string, Schema> | undefined
  if (!properties) return
  for (const [key, text] of docs) {
    if (paramName !== null && key === paramName) {
      if (withSelf) schema.description = text
      continue
    }
    const dot = key.indexOf('.')
    const prop = dot >= 0 ? (paramName === null || key.slice(0, dot) === paramName ? key.slice(dot + 1) : null) : withSelf ? key : null
    if (prop !== null && properties[prop]) properties[prop] = { ...properties[prop], description: text }
  }
}

export interface AnnotationScanner {
  /** 扫描（复用上次的 Program 以加快增量扫描）。 */
  scan(): AnnotationScanResult
  /** 文件变化是否可能影响扫描结果（是上次扫描的依赖，或是匹配的源文件且含 `@mcp`）。 */
  isRelevant(file: string): boolean
  /** 文件是否匹配 include / exclude。 */
  matches(file: string): boolean
}

export function createAnnotationScanner(options: AnnotationScanOptions): AnnotationScanner {
  const root = resolve(options.root)
  const matcher = new FileMatcher(
    root,
    options.include ?? DEFAULT_ANNOTATION_INCLUDE,
    options.exclude ?? DEFAULT_ANNOTATION_EXCLUDE,
  )
  let oldProgram: ts.Program | undefined
  let dependencies = new Set<string>()

  function scan(): AnnotationScanResult {
    const warnings: string[] = []
    const errors: string[] = []
    const rootFiles = matcher.list().filter((file) => {
      try {
        return MCP_TAG.test(readFileSync(file, 'utf8'))
      } catch {
        return false
      }
    })
    if (rootFiles.length === 0) {
      oldProgram = undefined
      dependencies = new Set()
      return { tools: [], warnings, errors, dependencies: [] }
    }
    const compilerOptions = loadCompilerOptions(root, options.tsconfig, warnings)
    const program = ts.createProgram({ rootNames: rootFiles, options: compilerOptions, oldProgram })
    oldProgram = program
    const checker = program.getTypeChecker()

    const tools: AnnotatedTool[] = []
    for (const file of rootFiles) {
      const sourceFile = program.getSourceFile(file)
      if (!sourceFile) continue
      const candidates = collectCandidates(sourceFile)
      if (candidates.length === 0) continue
      const exports = exportNames(checker, sourceFile)
      for (const candidate of candidates) {
        const tool = analyzeCandidate(candidate, file, root, checker, exports, warnings, errors, options.appId)
        if (tool) tools.push(tool)
      }
    }

    const seen = new Map<string, AnnotatedTool>()
    for (const tool of tools) {
      const prev = seen.get(tool.name)
      if (prev) {
        errors.push(`注释工具重名：${tool.name}（${prev.source} 的 ${prev.exportName} 与 ${tool.source} 的 ${tool.exportName}）`)
      } else {
        seen.set(tool.name, tool)
      }
    }

    dependencies = new Set(
      program
        .getSourceFiles()
        .filter((sf) => !program.isSourceFileDefaultLibrary(sf))
        .map((sf) => slash(resolve(sf.fileName)))
        .filter((f) => !slash(relative(root, f)).includes('node_modules/')),
    )
    return { tools, warnings, errors, dependencies: [...dependencies].sort() }
  }

  return {
    scan,
    matches: (file) => matcher.matches(file),
    isRelevant(file: string): boolean {
      const normalized = slash(resolve(file))
      if (dependencies.has(normalized)) return true
      if (!matcher.matches(normalized)) return false
      try {
        return MCP_TAG.test(readFileSync(normalized, 'utf8'))
      } catch {
        return false
      }
    },
  }
}

/** 一次性扫描。 */
export function scanAnnotations(options: AnnotationScanOptions): AnnotationScanResult {
  return createAnnotationScanner(options).scan()
}

// ---------------------------------------------------------------------------
// 虚拟模块代码
// ---------------------------------------------------------------------------

/** 虚拟模块中 `annotatedTools` 的元素（不含本地路径与调用方式）。 */
export interface AnnotatedToolInfo {
  name: string
  description: string
  title?: string
  risk?: Risk
  activation?: Activation
  inputSchema: { type: 'object'; [key: string]: unknown }
  /** 源文件相对项目根目录的路径。 */
  source: string
  exportName: string
  member?: string
}

export function toAnnotatedToolInfo(tool: AnnotatedTool): AnnotatedToolInfo {
  const info: AnnotatedToolInfo = {
    name: tool.name,
    description: tool.description,
    inputSchema: tool.inputSchema,
    source: tool.source,
    exportName: tool.exportName,
  }
  if (tool.title !== undefined) info.title = tool.title
  if (tool.risk !== undefined) info.risk = tool.risk
  if (tool.activation !== undefined) info.activation = tool.activation
  if (tool.member !== undefined) info.member = tool.member
  return info
}

/**
 * 生成 `virtual:app-mcp/annotated` 的代码：导入各源文件中的函数，导出 `annotatedTools`
 * 与 `registerAnnotated(registrar)`（注册全部工具，返回统一的 dispose）。
 */
export function generateAnnotatedModule(tools: readonly AnnotatedTool[]): string {
  const lines: string[] = ['// 由 @app-mcp/build 根据 @mcp 注释生成，请勿手动修改。']
  const handlers: string[] = []
  tools.forEach((tool, i) => {
    const local = `__mcp${i}`
    const imported = /^[A-Za-z_$][\w$]*$/.test(tool.exportName) ? tool.exportName : JSON.stringify(tool.exportName)
    lines.push(`import { ${imported} as ${local} } from ${JSON.stringify(tool.file)}`)
    const target = tool.member ? `${local}[${JSON.stringify(tool.member)}]` : local
    const args: string[] = []
    const { call } = tool
    if (call.mode === 'object') args.push('input ?? {}')
    else if (call.mode === 'positional') {
      for (const p of call.params) args.push(`(input ?? {})[${JSON.stringify(p)}]`)
    }
    if (call.context) args.push('context')
    handlers.push(`  (input, context) => ${target}(${args.join(', ')}),`)
  })
  lines.push(
    '',
    `export const annotatedTools = ${JSON.stringify(tools.map(toAnnotatedToolInfo), null, 2)}`,
    '',
    'const handlers = [',
    ...handlers,
    ']',
    '',
    'export function registerAnnotated(registrar) {',
    '  const handles = []',
    '  const dispose = () => {',
    '    while (handles.length > 0) handles.pop().dispose()',
    '  }',
    '  try {',
    '    annotatedTools.forEach((tool, i) => {',
    '      const definition = { description: tool.description, input: tool.inputSchema, handler: handlers[i] }',
    '      if (tool.title !== undefined) definition.title = tool.title',
    '      if (tool.risk !== undefined) definition.risk = tool.risk',
    '      if (tool.activation !== undefined) definition.activation = tool.activation',
    '      handles.push(registrar.tool(tool.name, definition))',
    '    })',
    '  } catch (err) {',
    '    dispose()',
    '    throw err',
    '  }',
    '  return dispose',
    '}',
    '',
  )
  return lines.join('\n')
}
