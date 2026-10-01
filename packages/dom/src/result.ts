/**
 * 页面结果 → 工具 handler 返回值（spec/protocol.md 3.2）。
 *
 * 两种模式（{@link import('./attach').AttachDomOptions.resultEnvelope}）：
 * - 默认：结果始终整体作为 `data`（避免页面结果本身带 `data` 字段时被 SDK 拆开），无结果时为 `{ ok: true }`；
 * - 结构化：页面结果形如信封（判定同 `isToolResultEnvelope`）时原样透传，无结果时为无返回值（Hub 输出"已完成"）。
 * 两种模式都附带 `data-mcp-hints`（并入信封自身的 `stateHints`）与 `data-mcp-summary`（信封自带 `summary` 时以信封为准）。
 */

import { isToolResultEnvelope } from '@app-mcp/web'

/** invoke 的结果：页面给出了结果，或只是等待稳定后结束（没有结果）。 */
export type PageOutcome = { kind: 'result'; value: unknown } | { kind: 'settled' }

/** 元素上声明的静态结果字段。 */
export interface StaticResultFields {
  hints: string[]
  summary: string | undefined
}

/** @compat 默认模式下无结果时的返回值（既有行为）。 */
const LEGACY_SETTLED_DATA = { ok: true } as const

type Envelope = Record<string, unknown> & { data: unknown; stateHints?: string[]; summary?: string }

function mergeHints(own: string[] | undefined, extra: string[]): string[] | undefined {
  if (extra.length === 0) return own
  return [...new Set([...(own ?? []), ...extra])]
}

/** 在信封上补齐静态字段；字段都未变化时返回原对象。 */
function withStaticFields(envelope: Envelope, fields: StaticResultFields): Envelope {
  const stateHints = mergeHints(envelope.stateHints, fields.hints)
  const summary = envelope.summary ?? fields.summary
  if (stateHints === envelope.stateHints && summary === envelope.summary) return envelope
  const out: Envelope = { ...envelope }
  if (stateHints !== undefined) out.stateHints = stateHints
  if (summary !== undefined) out.summary = summary
  return out
}

export function toToolResult(outcome: PageOutcome, fields: StaticResultFields, envelopeMode: boolean): Envelope {
  if (!envelopeMode) {
    const data = outcome.kind === 'result' ? outcome.value : LEGACY_SETTLED_DATA
    return withStaticFields({ data }, fields)
  }
  if (outcome.kind === 'settled') return withStaticFields({ data: undefined }, fields)
  if (isToolResultEnvelope(outcome.value)) return withStaticFields(outcome.value as Envelope, fields)
  return withStaticFields({ data: outcome.value }, fields)
}
