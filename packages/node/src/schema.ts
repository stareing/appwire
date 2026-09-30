/**
 * 工具输入定义 → JSON Schema 文本 + 可选的调用前校验函数。
 *
 * - zod v4 schema：优先用 schema 自带的 `toJSONSchema()`（zod ≥ 4.1）；没有时动态 `import('zod')`
 *   调用 `z.toJSONSchema`（zod 是可选 peer 依赖）。调用前用 `parse` 校验并转换参数。
 * - 带 `toJSONSchema()` 方法的对象：调用它得到 JSON Schema，不做额外校验（Host 已按 schema 校验）。
 * - 普通 JSON Schema 对象：原样使用。
 */

import type { InputDefinition } from './types.js'

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

/** zod 转 JSON Schema 时使用输入侧类型（默认值、transform 前的形状）。 */
const ZOD_JSON_SCHEMA_PARAMS = { io: 'input', unrepresentable: 'any' } as const

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
    // Standard JSON Schema（zod ≥ 4.2）：与 @app-mcp/web 相同的优先顺序。
    const std = (input as { '~standard'?: { jsonSchema?: { input?: (o: { target: string }) => unknown } } })[
      '~standard'
    ]?.jsonSchema
    if (std && typeof std.input === 'function') {
      return { schemaJson: toSchemaJson(std.input({ target: 'draft-2020-12' })), parse }
    }
    if (typeof input.toJSONSchema === 'function') {
      return { schemaJson: toSchemaJson(input.toJSONSchema(ZOD_JSON_SCHEMA_PARAMS)), parse }
    }
    return importZod().then(
      (z) => ({ schemaJson: toSchemaJson(z.toJSONSchema(input, ZOD_JSON_SCHEMA_PARAMS)), parse }),
      (error: unknown) => {
        throw new TypeError(`无法加载 zod 以转换 input schema：${String(error)}`)
      },
    )
  }
  if (isObject(input) && typeof input.toJSONSchema === 'function') {
    return { schemaJson: toSchemaJson((input.toJSONSchema as () => unknown)()) }
  }
  return { schemaJson: toSchemaJson(input) }
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
