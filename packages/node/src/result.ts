/**
 * handler 返回值 → 调用结果（spec/protocol.md 3.2）。与 @app-mcp/web 的 `src/result.ts` 语义一致
 * （本包不依赖 @app-mcp/web，按结构重复实现；两边的测试覆盖同一组用例）。
 */

import type { ContentAnnotations, ResultStatus, UndoAction } from './types.js'

/** 规范化后的成功结果；`data` 为原始值（提交时再序列化），可选字段只在给出时出现。 */
export interface NormalizedResult {
  data: unknown
  stateHints?: string[]
  status?: ResultStatus
  stateResource?: string
  summary?: string
  annotations?: ContentAnnotations
  undo?: UndoAction
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
  undo: isOptional(isPlainObject),
}

function isEnvelope(result: unknown): result is NormalizedResult {
  if (!isPlainObject(result) || !('data' in (result as object))) return false
  return Object.entries(result as Record<string, unknown>).every(([key, value]) => {
    const check = ENVELOPE_FIELDS[key as keyof NormalizedResult] as ((v: unknown) => boolean) | undefined
    return check !== undefined && check(value)
  })
}

/** handler 返回值 → 结果：结构化结果（`ToolResultEnvelope`）被拆开，其他值整体作为 `data`；空 `stateHints` 省略。 */
export function normalizeToolResult(result: unknown): NormalizedResult {
  if (!isEnvelope(result)) return { data: result }
  const out: NormalizedResult = { data: result.data }
  if (result.stateHints && result.stateHints.length > 0) out.stateHints = result.stateHints.map(String)
  if (result.status !== undefined) out.status = result.status
  if (result.stateResource !== undefined) out.stateResource = result.stateResource
  if (result.summary !== undefined) out.summary = result.summary
  if (result.annotations !== undefined) out.annotations = result.annotations
  if (result.undo !== undefined) out.undo = result.undo
  return out
}

/** 是否带有 `data` / `stateHints` 以外的字段（旧版原生模块的 `complete` 无法携带）。 */
export function hasResultExtras(result: NormalizedResult): boolean {
  return (
    result.status !== undefined ||
    result.stateResource !== undefined ||
    result.summary !== undefined ||
    result.annotations !== undefined ||
    result.undo !== undefined
  )
}
