/**
 * 工具输入定义 → JSON Schema，以及 zod 校验。
 */

import type { InputDefinition, JsonSchema, OutputDefinition, OutputSchema, ZodLike } from './types'

export const EMPTY_INPUT_SCHEMA: JsonSchema = { type: 'object', properties: {} }

export function isZodLike(input: unknown): input is ZodLike<unknown> {
  return (
    typeof input === 'object' &&
    input !== null &&
    '_zod' in input &&
    typeof (input as { parse?: unknown }).parse === 'function'
  )
}

type SchemaIo = 'input' | 'output'

interface StandardJsonSchema {
  '~standard'?: { jsonSchema?: Partial<Record<SchemaIo, (options: { target: string }) => unknown>> }
}

type ZodModule = { toJSONSchema?: (schema: unknown, params?: { io?: 'input' | 'output' }) => unknown }

const IO_LABEL: Record<SchemaIo, string> = { input: '输入', output: '输出' }

function asObject(value: unknown, io: SchemaIo): OutputSchema {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${IO_LABEL[io]} schema 必须是 JSON 对象`)
  }
  return value as OutputSchema
}

/** zod v4：优先用 Standard JSON Schema（zod ≥ 4.2，无需再 import zod），否则动态加载 `z.toJSONSchema`。 */
async function zodToJsonSchemaAsync(schema: unknown, io: SchemaIo): Promise<OutputSchema> {
  let mod: ZodModule & { z?: ZodModule }
  try {
    mod = (await import('zod')) as unknown as ZodModule & { z?: ZodModule }
  } catch (e) {
    throw new Error(`无法加载 zod 以转换${IO_LABEL[io]} schema：${e instanceof Error ? e.message : String(e)}`)
  }
  const toJSONSchema = mod.toJSONSchema ?? mod.z?.toJSONSchema
  if (typeof toJSONSchema !== 'function') throw new Error('当前 zod 版本不支持 toJSONSchema（需要 zod v4）')
  return asObject(toJSONSchema(schema, { io }), io)
}

/** schema 定义（JSON Schema / zod / 带 `toJSONSchema()` 的对象）→ JSON Schema 对象，按 `io` 选择 zod 的转换形态。 */
function convertDefinition(definition: object, io: SchemaIo): OutputSchema | Promise<OutputSchema> {
  if (isZodLike(definition)) {
    const convert = (definition as StandardJsonSchema)['~standard']?.jsonSchema?.[io]
    if (typeof convert === 'function') return asObject(convert({ target: 'draft-2020-12' }), io)
    // zod v4 classic 的实例方法，无需再 import zod
    const method = (definition as { toJSONSchema?: unknown }).toJSONSchema
    if (typeof method === 'function') {
      return asObject((method as (params: { io: SchemaIo }) => unknown).call(definition, { io }), io)
    }
    return zodToJsonSchemaAsync(definition, io)
  }
  const withMethod = definition as { toJSONSchema?: unknown }
  if (typeof withMethod.toJSONSchema === 'function') {
    return asObject((withMethod.toJSONSchema as () => unknown).call(definition), io)
  }
  return asObject(definition, io)
}

/**
 * 转换输入定义。能同步完成时直接返回结果，只有需要动态加载 zod 时返回 Promise。
 * 转换失败时同步抛出或返回 rejected Promise。
 */
export function toJsonSchema(input: InputDefinition<unknown> | undefined): JsonSchema | Promise<JsonSchema> {
  if (input === undefined || input === null) return EMPTY_INPUT_SCHEMA
  return convertDefinition(input, 'input') as JsonSchema | Promise<JsonSchema>
}

/**
 * 转换输出定义（MCP `outputSchema`），zod 按输出形态转换；根类型不限于 object。
 * 同步 / 异步与失败方式同 {@link toJsonSchema}。
 */
export function toOutputSchema(output: OutputDefinition<unknown>): OutputSchema | Promise<OutputSchema> {
  return convertDefinition(output, 'output')
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
