/**
 * WebMCP 标准 ↔ app-mcp 之间的数据转换：结果格式、annotations ↔ risk、输入 schema、错误。
 */

import type { CoreOutcome } from '../core'
import { EMPTY_INPUT_SCHEMA } from '../schema'
import { type JsonSchema, type Risk, ToolCallError, type ToolAnnotations as McpToolAnnotations } from '../types'
import type { ToolAnnotations, ToolExecuteOptions } from './types'

/** 标准的工具名规则：1–128 个 ASCII 字母数字、`_`、`-`、`.`。 */
export const STANDARD_NAME_RE = /^[A-Za-z0-9_.-]{1,128}$/
/** app-mcp 的工具名规则（更严格：最长 64）。 */
export const APP_MCP_NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/

/** 创建 DOMException（缺少时退化为带 name 的 Error）。 */
export function domException(message: string, name: string): Error {
  if (typeof DOMException === 'function') return new DOMException(message, name)
  const e = new Error(message)
  e.name = name
  return e
}

/** annotations → risk：破坏性 / 重大后果优先，其次只读，否则 write。 */
export function annotationsToRisk(annotations: ToolAnnotations | undefined): Risk {
  if (!annotations) return 'write'
  if (annotations.destructiveHint === true || annotations.consequentialHint === true) return 'destructive'
  if (annotations.readOnlyHint === true) return 'read'
  return 'write'
}

/** risk → annotations（标准字段 + MCP 的 `destructiveHint`，原生实现会忽略未知字段）。 */
export function riskToAnnotations(risk: Risk | undefined): ToolAnnotations {
  switch (risk) {
    case 'read':
      return { readOnlyHint: true }
    case 'destructive':
      return { readOnlyHint: false, consequentialHint: true, destructiveHint: true }
    case 'payment':
    case 'os-sensitive':
      return { readOnlyHint: false, consequentialHint: true }
    default:
      return { readOnlyHint: false }
  }
}

/** 两套注解共有的 MCP 提示字段（WebMCP 的 `ToolAnnotations` 与 app-mcp 工具注解同名同义）。 */
const MCP_HINTS = ['readOnlyHint', 'destructiveHint', 'idempotentHint', 'openWorldHint'] as const

/** 只取取值为布尔的 MCP 提示字段；一个都没有时返回 undefined。 */
function pickHints(source: Record<string, unknown> | undefined): McpToolAnnotations | undefined {
  if (!source) return undefined
  const entries = MCP_HINTS.filter((k) => typeof source[k] === 'boolean').map((k) => [k, source[k]])
  return entries.length > 0 ? (Object.fromEntries(entries) as McpToolAnnotations) : undefined
}

/** 标准侧注解 → app-mcp 工具注解（只保留 MCP 提示字段，原样转发给 Agent）。 */
export function standardToToolAnnotations(annotations: ToolAnnotations | undefined): McpToolAnnotations | undefined {
  return pickHints(annotations)
}

/** app-mcp 工具的 risk 与声明的注解 → 标准注解：声明的 MCP 提示字段覆盖按 risk 推导的值。 */
export function toolToStandardAnnotations(
  risk: Risk | undefined,
  annotations: McpToolAnnotations | undefined,
): ToolAnnotations {
  return { ...riskToAnnotations(risk), ...pickHints(annotations as Record<string, unknown> | undefined) }
}

/** 标准 `inputSchema` → app-mcp 输入 schema。字符串形式（旧版 Chrome 接受）先解析。 */
export function standardInputSchema(inputSchema: unknown): JsonSchema {
  let schema = inputSchema
  if (schema === undefined || schema === null) return EMPTY_INPUT_SCHEMA
  if (typeof schema === 'string') schema = JSON.parse(schema)
  // 与标准一致：必须能序列化为 JSON（循环引用等会抛错）
  const text = JSON.stringify(schema)
  if (text === undefined) throw new TypeError('inputSchema 无法序列化为 JSON')
  const parsed = JSON.parse(text) as unknown
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new TypeError('inputSchema 必须是 JSON 对象')
  }
  const obj = parsed as Record<string, unknown>
  return (obj.type === undefined ? { type: 'object', ...obj } : obj) as JsonSchema
}

function parseText(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return { text }
  }
}

interface ContentItem {
  type?: unknown
  text?: unknown
}

function isContentResult(v: unknown): v is { content: ContentItem[]; isError?: unknown; structuredContent?: unknown } {
  return typeof v === 'object' && v !== null && Array.isArray((v as { content?: unknown }).content)
}

/**
 * 标准 `execute` 的返回值 → app-mcp handler 结果（data）。
 *
 * - MCP 风格 `{ content: [{ type: 'text', text }] }`：text 是 JSON 则解析为 data，否则 `{ text }`；
 *   多个 text 以换行连接；含非文本内容时整体 `{ content }` 作为 data；有 `structuredContent` 时优先用它；
 * - `isError: true` → 抛出 `ToolCallError('HANDLER_ERROR', text)`；
 * - 字符串：同 text 的规则；`undefined` → null；其他值原样作为 data。
 */
export function fromStandardResult(result: unknown): unknown {
  if (result === undefined) return null
  if (typeof result === 'string') return parseText(result)
  if (!isContentResult(result)) return result
  const items = result.content
  const texts = items.filter((i) => i?.type === 'text').map((i) => String(i.text ?? ''))
  const message = texts.join('\n')
  if (result.isError === true) throw new ToolCallError('HANDLER_ERROR', message || '工具执行失败')
  const structured = result.structuredContent
  if (typeof structured === 'object' && structured !== null) return structured
  if (texts.length === items.length) return texts.length === 0 ? null : parseText(message)
  return { content: items }
}

/** 标准结果格式（MCP 风格）。 */
export interface StandardResult {
  content: { type: 'text'; text: string }[]
  isError?: true
}

/** app-mcp 的执行结果 → 标准 `execute` 返回值（MCP 风格 content）。错误以 `isError` 返回，便于智能体自我纠正。 */
export function toStandardResult(outcome: CoreOutcome): StandardResult {
  if ('error' in outcome) {
    return { content: [{ type: 'text', text: `${outcome.error.kind}: ${outcome.error.message}` }], isError: true }
  }
  const data = outcome.data
  const text = typeof data === 'string' ? data : (JSON.stringify(data) ?? 'null')
  return { content: [{ type: 'text', text }] }
}

/**
 * 构造 `execute` 的第二个参数。`requestUserInteraction(cb)` 直接执行 cb：
 * 风险确认由 Host 在调用前完成（见 README）。若原生提供了该方法则优先使用原生实现。
 */
export function executeOptions(signal: AbortSignal, native?: Partial<ToolExecuteOptions> | null): ToolExecuteOptions {
  const nativeRequest = native?.requestUserInteraction
  return {
    signal,
    requestUserInteraction: <T>(callback: () => T | Promise<T>): Promise<T> => {
      if (typeof nativeRequest === 'function') return nativeRequest.call(native, callback) as Promise<T>
      try {
        return Promise.resolve(callback())
      } catch (e) {
        return Promise.reject(e)
      }
    },
  }
}

/** 标准 `exposedTo` / `fromOrigins` 校验：必须是可信（potentially trustworthy）来源。 */
export function assertTrustworthyOrigins(origins: readonly string[] | undefined): void {
  if (!origins) return
  for (const origin of origins) {
    let url: URL
    try {
      url = new URL(origin)
    } catch {
      throw domException(`无效的来源 ${JSON.stringify(origin)}`, 'SecurityError')
    }
    const host = url.hostname
    const trustworthy =
      url.protocol === 'https:' ||
      url.protocol === 'wss:' ||
      url.protocol === 'file:' ||
      host === 'localhost' ||
      host.endsWith('.localhost') ||
      host === '127.0.0.1' ||
      host === '[::1]'
    if (!trustworthy) throw domException(`来源 ${JSON.stringify(origin)} 不是安全来源`, 'SecurityError')
  }
}
