import { describe, expect, it, vi } from 'vitest'
import {
  handleAnthropicToolUses,
  handleGeminiFunctionCalls,
  handleOpenAiResponsesCalls,
  handleOpenAiToolCalls,
  HubToolCallError,
  toAnthropicTools,
  toGeminiTools,
  toOpenAiResponsesTools,
  toOpenAiTools,
  toVercelAiTools,
  type HubLike,
} from './adapters.js'
import type { ToolFormat } from './types.js'

/** 替身：exportTools 返回按格式的固定形态，dispatch 记录调用并回显。 */
function fakeHub() {
  const dispatched: Array<{ format: ToolFormat; call: any; session: string | null | undefined }> = []
  const cancelled: string[] = []
  let release: (() => void) | undefined
  const hub = {
    dispatched,
    cancelled,
    hold() {
      return new Promise<void>((r) => (release = r))
    },
    exportTools(format: ToolFormat) {
      switch (format) {
        case 'anthropic':
          return [{ name: 'shop__cart__add', description: '加入', input_schema: { type: 'object' } }]
        case 'openai-chat':
          return [{ type: 'function', function: { name: 'shop__cart__add', description: '加入', parameters: {} } }]
        case 'openai-responses':
          return [{ type: 'function', name: 'shop__cart__add', description: '加入', parameters: {} }]
        case 'gemini':
          return { functionDeclarations: [{ name: 'shop__cart__add', description: '加入' }] }
        default:
          return []
      }
    },
    async dispatch(format: ToolFormat, call: any, session?: string | null) {
      dispatched.push({ format, call, session })
      if (call.input?.wait) await new Promise<void>((r) => (release = r))
      switch (format) {
        case 'anthropic':
          return call.input?.fail
            ? { type: 'tool_result', tool_use_id: call.id, content: 'USER_REJECTED: 不', is_error: true }
            : { type: 'tool_result', tool_use_id: call.id, content: '{"ok":1}' }
        case 'openai-chat':
          return { role: 'tool', tool_call_id: call.id, content: 'ok' }
        case 'openai-responses':
          return { type: 'function_call_output', call_id: call.call_id, output: 'ok' }
        case 'gemini':
          return { functionResponse: { name: call.name, response: { output: 'ok' } } }
        default:
          return {}
      }
    },
    cancelCall(id: string) {
      cancelled.push(id)
      release?.()
    },
  }
  return hub as typeof hub & HubLike
}

describe('OpenAI 适配', () => {
  it('导出与批量 dispatch（按原顺序，过滤非 function）', async () => {
    const hub = fakeHub()
    expect(toOpenAiTools(hub)[0]?.function.name).toBe('shop__cart__add')
    const out = await handleOpenAiToolCalls(
      hub,
      [
        { id: 'a', type: 'function', function: { name: 'x', arguments: '{}' } },
        { id: 'b', function: { name: 'y', arguments: '{}' } },
        { id: 'c', type: 'custom' as 'function', function: { name: 'z', arguments: '' } },
      ],
      { session: 's' },
    )
    expect(out.map((m) => m.tool_call_id)).toEqual(['a', 'b'])
    expect(hub.dispatched.every((d) => d.format === 'openai-chat' && d.session === 's')).toBe(true)
    expect(await handleOpenAiToolCalls(hub, null)).toEqual([])
  })

  it('Responses API：只处理 function_call 项', async () => {
    const hub = fakeHub()
    expect(toOpenAiResponsesTools(hub)[0]?.name).toBe('shop__cart__add')
    const out = await handleOpenAiResponsesCalls(hub, [
      { type: 'message', content: [] },
      { type: 'function_call', call_id: 'c1', name: 'x', arguments: '{}' },
    ])
    expect(out).toEqual([{ type: 'function_call_output', call_id: 'c1', output: 'ok' }])
  })
})

describe('Anthropic / Gemini 适配', () => {
  it('只处理 tool_use 块；sequential 逐个执行', async () => {
    const hub = fakeHub()
    expect(toAnthropicTools(hub)[0]?.input_schema).toEqual({ type: 'object' })
    const out = await handleAnthropicToolUses(
      hub,
      [
        { type: 'text', text: 'hi' },
        { type: 'tool_use', id: 't1', name: 'x', input: {} },
        { type: 'tool_use', id: 't2', name: 'y', input: { fail: true } },
        null,
      ],
      { sequential: true },
    )
    expect(out.map((r) => [r.tool_use_id, r.is_error ?? false])).toEqual([
      ['t1', false],
      ['t2', true],
    ])
    expect(hub.dispatched.map((d) => d.session)).toEqual([null, null])
  })

  it('Gemini：接受 parts 与裸 functionCalls', async () => {
    const hub = fakeHub()
    expect(toGeminiTools(hub).functionDeclarations).toHaveLength(1)
    const out = await handleGeminiFunctionCalls(hub, [
      { text: 'x' },
      { functionCall: { name: 'a', args: {} } },
      { name: 'b', args: {} },
    ])
    expect(out.map((p) => p.functionResponse.name)).toEqual(['a', 'b'])
  })
})

describe('Vercel AI 适配', () => {
  it('形态、jsonSchema 包装、execute 成功与失败', async () => {
    const hub = fakeHub()
    const raw = toVercelAiTools(hub)
    expect(raw.shop__cart__add).toMatchObject({ description: '加入', inputSchema: { type: 'object' } })

    const wrap = vi.fn((s: unknown) => ({ jsonSchema: s }))
    const tools = toVercelAiTools(hub, undefined, { jsonSchema: wrap, session: 'conv' })
    expect(tools.shop__cart__add?.inputSchema).toEqual({ jsonSchema: { type: 'object' } })
    expect(await tools.shop__cart__add!.execute({ a: 1 }, { toolCallId: 'tc1' })).toBe('{"ok":1}')
    expect(hub.dispatched.at(-1)).toEqual({
      format: 'anthropic',
      call: { type: 'tool_use', id: 'tc1', name: 'shop__cart__add', input: { a: 1 } },
      session: 'conv',
    })
    await expect(tools.shop__cart__add!.execute({ fail: true })).rejects.toThrow(HubToolCallError)
    // 未提供 toolCallId 时自动生成
    expect(hub.dispatched.at(-1)?.call.id).toMatch(/^vai-/)
  })

  it('abortSignal → cancelCall', async () => {
    const hub = fakeHub()
    const tools = toVercelAiTools(hub)
    const ac = new AbortController()
    const p = tools.shop__cart__add!.execute({ wait: true }, { toolCallId: 'x1', abortSignal: ac.signal })
    await new Promise((r) => setTimeout(r, 5))
    ac.abort()
    await p
    expect(hub.cancelled).toEqual(['x1'])

    const aborted = new AbortController()
    aborted.abort()
    await expect(tools.shop__cart__add!.execute({}, { abortSignal: aborted.signal })).rejects.toThrow('CANCELLED')
  })
})
