/**
 * 工具输入定义 → JSON Schema 文本 + 可选的调用前校验函数。
 *
 * - zod v4 schema：优先用 schema 自带的 `toJSONSchema()`（zod ≥ 4.1）；没有时动态 `import('zod')`
 *   调用 `z.toJSONSchema`（zod 是可选 peer 依赖）。调用前用 `parse` 校验并转换参数。
 * - 带 `toJSONSchema()` 方法的对象：调用它得到 JSON Schema，不做额外校验（Host 已按 schema 校验）。
 * - 普通 JSON Schema 对象：原样使用。
 */

import type { InputDefinition, OutputDefinition } from './types.js'

export interface ResolvedInput {
  /** JSON Schema 文本；`undefined` 表示无参数。 */
  schemaJson: string | undefined
  /** 调用前校验 / 转换参数（zod）。 */
  parse?: (input: unknown) => unknown
}

interface ZodSchemaLike {
  _zod: unknown
  parse(input: unknown): unknown
  toJSONSchema?: (params?: Record<string, unknown>) => unknown
}

interface ZodModuleLike {
  toJSONSchema(schema: unknown, params?: Record<string, unknown>): unknown
}

type SchemaIo = 'input' | 'output'

/** zod 转 JSON Schema 的参数：输入 schema 用输入侧类型（默认值、transform 前的形状），输出 schema 用输出侧类型。 */
const zodParams = (io: SchemaIo) => ({ io, unrepresentable: 'any' }) as const

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

export function isZodSchema(value: unknown): value is ZodSchemaLike {
  return isObject(value) && '_zod' in value && typeof value.parse === 'function'
}

function toSchemaJson(schema: unknown): string {
  if (!isObject(schema) || schema.type !== 'object') {
    throw new TypeError('工具的 input schema 顶层 type 必须为 "object"')
  }
  return JSON.stringify(schema)
}

/** 输出 schema 只要求是 JSON 对象（根类型不限）。 */
function toOutputSchemaJson(schema: unknown): string {
  if (!isObject(schema) || Array.isArray(schema)) throw new TypeError('工具的 outputSchema 必须是 JSON 对象')
  return JSON.stringify(schema)
}

/** zod schema → JSON Schema 值（同步可得时直接返回）。顺序与 @app-mcp/web 相同：Standard JSON Schema → 实例方法 → `z.toJSONSchema`。 */
function zodToJsonSchema(schema: ZodSchemaLike, io: SchemaIo): unknown {
  const std = (schema as { '~standard'?: { jsonSchema?: Partial<Record<SchemaIo, (o: { target: string }) => unknown>> } })[
    '~standard'
  ]?.jsonSchema?.[io]
  if (typeof std === 'function') return std({ target: 'draft-2020-12' })
  if (typeof schema.toJSONSchema === 'function') return schema.toJSONSchema(zodParams(io))
  return importZod().then(
    (z) => z.toJSONSchema(schema, zodParams(io)),
    (error: unknown) => {
      throw new TypeError(`无法加载 zod 以转换 ${io} schema：${String(error)}`)
    },
  )
}

/** 同步值或 Promise 统一做后续转换。 */
function then<T, R>(value: unknown, map: (v: unknown) => R): R | Promise<R> {
  return value instanceof Promise ? (value.then(map) as Promise<R>) : map(value as T)
}

/** 可替换的 zod 加载器（测试用）。 */
let importZod: () => Promise<ZodModuleLike> = async () => {
  // 变量形式的模块名：zod 是可选依赖，避免打包器 / 类型检查强制解析。
  const name = 'zod'
  const mod = (await import(/* @vite-ignore */ name)) as { z?: ZodModuleLike } & ZodModuleLike
  return mod.z ?? mod
}

export function setZodImporter(importer: () => Promise<ZodModuleLike>): void {
  importZod = importer
}

/**
 * 解析输入定义。能同步完成时返回结果本身，需要动态加载 zod 时返回 Promise。
 * 定义不合法时抛出（或 reject）TypeError。
 */
export function resolveInput(input: InputDefinition<unknown> | undefined): ResolvedInput | Promise<ResolvedInput> {
  if (input === undefined) return { schemaJson: undefined }
  if (isZodSchema(input)) {
    const parse = (value: unknown) => input.parse(value)
    return then(zodToJsonSchema(input, 'input'), (schema) => ({ schemaJson: toSchemaJson(schema), parse }))
  }
  if (isObject(input) && typeof input.toJSONSchema === 'function') {
    return { schemaJson: toSchemaJson((input.toJSONSchema as () => unknown)()) }
  }
  return { schemaJson: toSchemaJson(input) }
}

/**
 * 解析输出定义（MCP `outputSchema`）→ JSON Schema 文本；`undefined` 表示未声明。zod 按输出形态转换、不做校验。
 * 同步 / 异步与失败方式同 {@link resolveInput}。
 */
export function resolveOutput(output: OutputDefinition<unknown> | undefined): string | undefined | Promise<string> {
  if (output === undefined) return undefined
  if (isZodSchema(output)) return then(zodToJsonSchema(output, 'output'), toOutputSchemaJson)
  if (isObject(output) && typeof output.toJSONSchema === 'function') {
    return toOutputSchemaJson((output.toJSONSchema as () => unknown)())
  }
  return toOutputSchemaJson(output)
}

/** zod 校验失败的简要说明与详情（与 @app-mcp/web 的格式一致：`details.issues` 为 `{ path, message }[]`）。 */
export function describeParseError(error: unknown): { message: string; details?: Record<string, unknown> } {
  const issues = (error as { issues?: unknown } | null)?.issues
  if (Array.isArray(issues)) {
    const list = issues.map((i: { path?: unknown; message?: unknown }) => ({
      path: Array.isArray(i.path) ? i.path.map(String).join('.') : '',
      message: String(i.message ?? ''),
    }))
    const summary = list.map((i) => (i.path ? `${i.path}: ${i.message}` : i.message)).join('; ')
    return { message: `参数校验失败：${summary}`, details: { issues: list } }
  }
  return { message: `参数校验失败：${error instanceof Error ? error.message : String(error)}` }
}
