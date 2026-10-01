import { isToolResultEnvelope } from '@app-mcp/web'
import { describe, expect, it } from 'vitest'
import { toToolResult, type PageOutcome } from '../src/result'

const none = { hints: [], summary: undefined }
const settled: PageOutcome = { kind: 'settled' }
const result = (value: unknown): PageOutcome => ({ kind: 'result', value })

describe('toToolResult', () => {
  it.each<[string, PageOutcome, { hints: string[]; summary: string | undefined }, boolean, unknown]>([
    ['默认：无结果 → { ok: true }', settled, none, false, { data: { ok: true } }],
    ['默认：信封形状也整体作为 data', result({ data: 1, summary: 's' }), none, false, { data: { data: 1, summary: 's' } }],
    ['默认：hints 与 summary 附加', result(5), { hints: ['a'], summary: '好了' }, false, { data: 5, stateHints: ['a'], summary: '好了' }],
    ['结构化：无结果 → 无返回值', settled, none, true, { data: undefined }],
    ['结构化：无结果 + summary', settled, { hints: [], summary: '已清空' }, true, { data: undefined, summary: '已清空' }],
    [
      '结构化：信封透传，hints 去重合并，信封 summary 优先',
      result({ data: null, status: 'partial', summary: '完成 2/3', stateHints: ['a'] }),
      { hints: ['a', 'b'], summary: '静态' },
      true,
      { data: null, status: 'partial', summary: '完成 2/3', stateHints: ['a', 'b'] },
    ],
    ['结构化：信封无 summary 时用静态 summary', result({ data: 1 }), { hints: [], summary: '静态' }, true, { data: 1, summary: '静态' }],
    ['结构化：非信封值作为 data', result({ data: 1, other: 2 }), { hints: ['h'], summary: undefined }, true, { data: { data: 1, other: 2 }, stateHints: ['h'] }],
  ])('%s', (_name, outcome, fields, envelope, expected) => {
    const out = toToolResult(outcome, fields, envelope)
    expect(out).toEqual(expected)
    expect(isToolResultEnvelope(out)).toBe(true)
  })

  it('没有需要补齐的字段时原样返回信封对象', () => {
    const env = { data: 1, status: 'noop' as const }
    expect(toToolResult(result(env), none, true)).toBe(env)
  })
})
