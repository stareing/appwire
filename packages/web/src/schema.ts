/**
 * 工具输入定义 → JSON Schema，以及 zod 校验。
 */

import type { InputDefinition, JsonSchema, ZodLike } from './types'

export const EMPTY_INPUT_SCHEMA: JsonSchema = { type: 'object', properties: {} }

export function isZodLike(input: unknown): input is ZodLike<unknown> {
  return (
    typeof input === 'object' &&
    input !== null &&
    '_zod' in input &&
    typeof (input as { parse?: unknown }).parse === 'function'
  )
}

interface StandardJsonSchema {
  '~standard'?: { jsonSchema?: { input?: (options: { target: string }) => unknown } }
}

type ZodModule = { toJSONSchema?: (schema: unknown, params?: { io?: 'input' | 'output' }) => unknown }

function asSchema(value: unknown): JsonSchema {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('输入 schema 必须是 JSON 对象')
  }
  return value as JsonSchema
}

/** zod v4：优先用 Standard JSON Schema（zod ≥ 4.2，无需再 import zod），否则动态加载 `z.toJSONSchema`。 */
async function zodToJsonSchemaAsync(schema: unknown): Promise<JsonSchema> {
  let mod: ZodModule & { z?: ZodModule }
  try {
    mod = (await import('zod')) as unknown as ZodModule & { z?: ZodModule }
  } catch (e) {
    throw new Error(`无法加载 zod 以转换输入 schema：${e instanceof Error ? e.message : String(e)}`)
  }
  const toJSONSchema = mod.toJSONSchema ?? mod.z?.toJSONSchema
  if (typeof toJSONSchema !== 'function') throw new Error('当前 zod 版本不支持 toJSONSchema（需要 zod v4）')
  return asSchema(toJSONSchema(schema, { io: 'input' }))
}

/**
 * 转换输入定义。能同步完成时直接返回结果，只有需要动态加载 zod 时返回 Promise。
 * 转换失败时同步抛出或返回 rejected Promise。
 */
export function toJsonSchema(input: InputDefinition<unknown> | undefined): JsonSchema | Promise<JsonSchema> {
  if (input === undefined || input === null) return EMPTY_INPUT_SCHEMA
  if (isZodLike(input)) {
    const std = (input as StandardJsonSchema)['~standard']?.jsonSchema
    if (std && typeof std.input === 'function') {
      return asSchema(std.input({ target: 'draft-2020-12' }))
    }
    // zod v4 classic 的实例方法（按输入形态转换），无需再 import zod
    const method = (input as { toJSONSchema?: unknown }).toJSONSchema
    if (typeof method === 'function') {
      return asSchema((method as (params: { io: 'input' }) => unknown).call(input, { io: 'input' }))
    }
    return zodToJsonSchemaAsync(input)
  }
  const withMethod = input as { toJSONSchema?: unknown }
  if (typeof withMethod.toJSONSchema === 'function') {
    return asSchema((withMethod.toJSONSchema as () => unknown).call(input))
  }
  return asSchema(input)
}

/** zod 校验失败的简要说明与详情。 */
export function describeParseError(error: unknown): { message: string; details?: Record<string, unknown> } {
  const issues = (error as { issues?: unknown }).issues
  if (Array.isArray(issues)) {
    const list = issues.map((i: { path?: unknown[]; message?: string }) => ({
      path: Array.isArray(i.path) ? i.path.map(String).join('.') : '',
      message: String(i.message ?? ''),
    }))
    const summary = list.map((i) => (i.path ? `${i.path}: ${i.message}` : i.message)).join('; ')
    return { message: `参数校验失败：${summary}`, details: { issues: list } }
  }
  return { message: `参数校验失败：${error instanceof Error ? error.message : String(error)}` }
}
