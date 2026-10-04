import { describe, expect, it } from 'vitest'
import { isToolResultEnvelope, normalizeToolResult } from '../src/result'
import * as web from '../src/index'

describe('normalizeToolResult', () => {
  it.each([
    ['普通值', 5, { data: 5 }],
    ['undefined → null', undefined, { data: null }],
    ['数组', [{ data: 1 }], { data: [{ data: 1 }] }],
    ['缺少 data 键', { summary: 's' }, { data: { summary: 's' } }],
    ['未知键', { data: 1, other: 2 }, { data: { data: 1, other: 2 } }],
    ['stateHints 不是数组', { data: 1, stateHints: 'x' }, { data: { data: 1, stateHints: 'x' } }],
    ['status 取值未知', { data: 1, status: 'ok' }, { data: { data: 1, status: 'ok' } }],
    ['stateResource 不是字符串', { data: 1, stateResource: 2 }, { data: { data: 1, stateResource: 2 } }],
    ['summary 不是字符串', { data: 1, summary: null }, { data: { data: 1, summary: null } }],
    ['annotations 不是对象', { data: 1, annotations: [] }, { data: { data: 1, annotations: [] } }],
    ['undo 不是对象', { data: 1, undo: 'u.remove' }, { data: { data: 1, undo: 'u.remove' } }],
    [
      'undo 原样带出（内容不校验，交给核心）',
      { data: 1, undo: { tool: 'bad name', arguments: [1] } },
      { data: 1, undo: { tool: 'bad name', arguments: [1] } },
    ],
    ['只有 data', { data: undefined }, { data: null }],
    ['空 stateHints 省略', { data: 1, stateHints: [] }, { data: 1 }],
    ['值为 undefined 的可选键', { data: 1, summary: undefined, status: undefined }, { data: 1 }],
    ['status done', { data: 1, status: 'done' }, { data: 1, status: 'done' }],
    ['status partial + summary', { data: 1, status: 'partial', summary: '完成 2/3' }, { data: 1, status: 'partial', summary: '完成 2/3' }],
    [
      'pending + stateResource + 内容注解',
      { data: null, status: 'pending', stateResource: 'order.state', annotations: { audience: ['assistant'], priority: 0 } },
      { data: null, status: 'pending', stateResource: 'order.state', annotations: { audience: ['assistant'], priority: 0 } },
    ],
  ])('%s', (_name, input, expected) => {
    expect(normalizeToolResult(input)).toEqual(expected)
  })

  it('stateHints 转为字符串；data 与注解按 JSON 序列化', () => {
    const at = new Date(0)
    expect(normalizeToolResult({ data: { at }, stateHints: [1], annotations: { lastModified: at } })).toEqual({
      data: { at: at.toJSON() },
      stateHints: ['1'],
      annotations: { lastModified: at.toJSON() },
    })
  })

  it('无法序列化时抛出', () => {
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    expect(() => normalizeToolResult({ data: cyclic })).toThrow()
  })
})

describe('isToolResultEnvelope', () => {
  it('从包入口导出，与 normalizeToolResult 的拆分规则一致', () => {
    expect(web.isToolResultEnvelope).toBe(isToolResultEnvelope)
  })

  it.each([
    ['只有 data', { data: 1 }, true],
    ['data 为 undefined', { data: undefined }, true],
    ['完整信封', { data: null, status: 'pending', stateResource: 'o.state', summary: 's', stateHints: ['a'] }, true],
    ['普通值', 5, false],
    ['undefined', undefined, false],
    ['null', null, false],
    ['数组', [{ data: 1 }], false],
    ['缺少 data 键', { summary: 's' }, false],
    ['未知键', { data: 1, other: 2 }, false],
    ['status 取值未知', { data: 1, status: 'ok' }, false],
  ])('%s', (_name, input, expected) => {
    expect(isToolResultEnvelope(input)).toBe(expected)
  })
})
