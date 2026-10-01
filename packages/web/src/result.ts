/**
 * handler 返回值 → 调用结果（spec/protocol.md 3.2）。驱动层（WASM 核心）与 Electron 渲染进程桥接共用。
 */

import type { ContentAnnotations, ResultStatus, ToolResultEnvelope } from './types'

/** 规范化后的成功结果；可选字段只在给出时出现。 */
export interface NormalizedResult {
  data: unknown
  stateHints?: string[]
  status?: ResultStatus
  stateResource?: string
  summary?: string
  annotations?: ContentAnnotations
}

const RESULT_STATUSES: readonly unknown[] = ['done', 'pending', 'partial', 'noop'] satisfies ResultStatus[]

const isOptional =
  (check: (v: unknown) => boolean) =>
  (v: unknown): boolean =>
    v === undefined || check(v)
const isString = (v: unknown): boolean => typeof v === 'string'
const isPlainObject = (v: unknown): boolean => typeof v === 'object' && v !== null && !Array.isArray(v)

/** 信封允许的键 → 取值检查。`data` 必须出现，其余可选；任何键不在表中或取值不合法时不视为信封。 */
const ENVELOPE_FIELDS: Readonly<Record<keyof NormalizedResult, (v: unknown) => boolean>> = {
  data: () => true,
  stateHints: isOptional(Array.isArray),
  status: isOptional((v) => RESULT_STATUSES.includes(v)),
  stateResource: isOptional(isString),
  summary: isOptional(isString),
  annotations: isOptional(isPlainObject),
}

/**
 * 判断 handler 返回值是否按结构化结果（{@link ToolResultEnvelope}）解释：普通对象、含 `data` 键、其余键都属于信封且取值合法。
 * 封装层（如 @app-mcp/store、@app-mcp/dom）据此决定原样透传还是整体包装为 `data`，与 SDK 的拆分规则保持一致。
 */
export function isToolResultEnvelope(result: unknown): result is ToolResultEnvelope<unknown> {
  if (!isPlainObject(result) || !('data' in (result as object))) return false
  return Object.entries(result as Record<string, unknown>).every(([key, value]) => {
    const check = ENVELOPE_FIELDS[key as keyof NormalizedResult] as ((v: unknown) => boolean) | undefined
    return check !== undefined && check(value)
  })
}

/** 转换为可 JSON 序列化的值（语义同 JSON.stringify：Date → 字符串，undefined → null）。 */
export function toJsonValue(value: unknown): unknown {
  if (value === undefined) return null
  const text = JSON.stringify(value)
  return text === undefined ? null : JSON.parse(text)
}

/**
 * handler 返回值 → 结果：结构化结果（{@link import('./types').ToolResultEnvelope}）被拆开，其他值整体作为 `data`。
 * `data` 与 `annotations` 转为 JSON 值；空 `stateHints` 省略。
 *
 * @error 返回值无法序列化为 JSON 时抛出（调用方转为 `HANDLER_ERROR`）。
 */
export function normalizeToolResult(result: unknown): NormalizedResult {
  if (!isToolResultEnvelope(result)) return { data: toJsonValue(result) }
  const out: NormalizedResult = { data: toJsonValue(result.data) }
  if (result.stateHints && result.stateHints.length > 0) out.stateHints = result.stateHints.map(String)
  if (result.status !== undefined) out.status = result.status
  if (result.stateResource !== undefined) out.stateResource = result.stateResource
  if (result.summary !== undefined) out.summary = result.summary
  if (result.annotations !== undefined) out.annotations = toJsonValue(result.annotations) as ContentAnnotations
  return out
}
