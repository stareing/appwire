import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import { installWebMcp, type ModelContext, type ToolAnnotations, type WebMcpUninstall } from '../src/webmcp'
import { connected, type Harness, settle } from './fakes'
import { coreTool, hostCall, startHostCall } from './webmcp-fakes'

/** 按标准写的页面代码看到的对象。 */
const mc = (): ModelContext => (document as unknown as { modelContext: ModelContext }).modelContext
const navMc = (): ModelContext => (navigator as unknown as { modelContext: ModelContext }).modelContext

let h: Harness
let uninstall: WebMcpUninstall | undefined

beforeEach(async () => {
  sessionStorage.clear()
  localStorage.clear()
  h = await connected()
  uninstall = installWebMcp(h.app)
})

afterEach(() => {
  uninstall?.()
  uninstall = undefined
  h.app.dispose()
})

describe('安装', () => {
  it('没有原生支持时在 document 与 navigator 上安装同一个 EventTarget', () => {
    expect(uninstall?.mode).toBe('polyfill')
    expect(mc()).toBe(uninstall?.modelContext)
    expect(navMc()).toBe(mc())
    expect(mc()).toBeInstanceOf(EventTarget)
    for (const m of ['registerTool', 'getTools', 'executeTool', 'unregisterTool', 'provideContext', 'clearContext']) {
      expect(typeof (mc() as unknown as Record<string, unknown>)[m]).toBe('function')
    }
    expect(Object.prototype.toString.call(mc())).toBe('[object ModelContext]')
  })

  it('卸载后移除 modelContext，并从 app-mcp 注销经标准注册的工具', async () => {
    await mc().registerTool({ name: 'a', description: 'A', execute: () => 'x' })
    expect(coreTool(h, 'a')).toBeDefined()
    uninstall?.()
    uninstall = undefined
    expect('modelContext' in document).toBe(false)
    expect('modelContext' in navigator).toBe(false)
    expect(coreTool(h, 'a')).toBeUndefined()
  })

  it('同一文档不能重复安装', () => {
    expect(() => installWebMcp(h.app)).toThrow(/已安装/)
  })
})

describe('按标准写的页面代码', () => {
  it('registerTool 进入 appMcp 注册（名称、描述、标题、schema、risk）', async () => {
    const p = mc().registerTool({
      name: 'add_todo',
      title: '添加待办',
      description: 'Add a new todo item',
      inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
      execute: async ({ text }: { text: string }) => ({ content: [{ type: 'text', text: `Added: ${text}` }] }),
    })
    expect(p).toBeInstanceOf(Promise)
    await expect(p).resolves.toBeUndefined()
    await settle()
    expect(coreTool(h, 'add_todo')).toEqual({
      name: 'add_todo',
      title: '添加待办',
      description: 'Add a new todo item',
      inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
      risk: 'write',
    })
  })

  it('缺省 inputSchema 为空对象 schema；字符串形式的 schema 会被解析', async () => {
    await mc().registerTool({ name: 'a', description: 'A', execute: () => null })
    await mc().registerTool({ name: 'b', description: 'B', inputSchema: '{"properties":{}}', execute: () => null })
    await settle()
    expect(coreTool(h, 'a')?.inputSchema).toEqual({ type: 'object', properties: {} })
    expect(coreTool(h, 'b')?.inputSchema).toEqual({ type: 'object', properties: {} })
  })

  it('调用往返：execute 收到输入与 { signal, requestUserInteraction }', async () => {
    const execute = vi.fn(async (input: { text: string }, client: { signal: AbortSignal; requestUserInteraction: <T>(cb: () => T) => Promise<T> }) => {
      const confirmed = await client.requestUserInteraction(() => 'ok')
      return { content: [{ type: 'text', text: JSON.stringify({ added: input.text, confirmed }) }] }
    })
    await mc().registerTool({ name: 'add_todo', description: '添加', execute })
    await settle()
    const outcome = await hostCall(h, 'add_todo', { text: '买牛奶' })
    expect(outcome).toEqual({ data: { added: '买牛奶', confirmed: 'ok' } })
    expect(execute.mock.calls[0]?.[1].signal).toBeInstanceOf(AbortSignal)
  })

  it('结果转换：纯文本 → { text }；多段文本合并；structuredContent 优先；字符串与任意值', async () => {
    const cases: [unknown, unknown][] = [
      [{ content: [{ type: 'text', text: 'Added todo item' }] }, { text: 'Added todo item' }],
      [{ content: [{ type: 'text', text: '[1,2]' }] }, [1, 2]],
      [{ content: [{ type: 'text', text: 'a' }, { type: 'text', text: 'b' }] }, { text: 'a\nb' }],
      [{ content: [{ type: 'text', text: 'x' }], structuredContent: { n: 1 } }, { n: 1 }],
      [{ content: [{ type: 'image', data: 'AAAA', mimeType: 'image/png' }] }, { content: [{ type: 'image', data: 'AAAA', mimeType: 'image/png' }] }],
      [{ content: [] }, null],
      ['plain', { text: 'plain' }],
      ['{"ok":true}', { ok: true }],
      [{ office: 'Building 4' }, { office: 'Building 4' }],
      [{ data: 1, stateHints: ['x'] }, { data: 1, stateHints: ['x'] }],
      [undefined, null],
      [42, 42],
    ]
    for (const [i, [result, data]] of cases.entries()) {
      await mc().registerTool({ name: `t${i}`, description: 'd', execute: () => result })
      await settle()
      expect(await hostCall(h, `t${i}`), JSON.stringify(result)).toEqual({ data })
    }
  })

  it('isError: true → HANDLER_ERROR；execute 抛错 → HANDLER_ERROR', async () => {
    await mc().registerTool({
      name: 'bad',
      description: 'd',
      execute: () => ({ content: [{ type: 'text', text: '库存不足' }], isError: true }),
    })
    await mc().registerTool({
      name: 'throws',
      description: 'd',
      execute: () => {
        throw new Error('boom')
      },
    })
    await settle()
    expect(await hostCall(h, 'bad')).toEqual({ error: { kind: 'HANDLER_ERROR', message: '库存不足' } })
    expect(await hostCall(h, 'throws')).toEqual({ error: { kind: 'HANDLER_ERROR', message: 'boom' } })
  })

  it('annotations → risk', async () => {
    const cases: [ToolAnnotations | undefined, string][] = [
      [{ readOnlyHint: true }, 'read'],
      [{ destructiveHint: true }, 'destructive'],
      [{ consequentialHint: true }, 'destructive'],
      [{ readOnlyHint: true, destructiveHint: true }, 'destructive'],
      [{ untrustedContentHint: true }, 'write'],
      [undefined, 'write'],
    ]
    for (const [i, [annotations, risk]] of cases.entries()) {
      await mc().registerTool({ name: `r${i}`, description: 'd', execute: () => null, ...(annotations && { annotations }) })
    }
    await settle()
    for (const [i, [, risk]] of cases.entries()) expect(coreTool(h, `r${i}`)?.risk).toBe(risk)
  })

  it('navigator.modelContext 入口同样可用（旧写法）', async () => {
    await navMc().registerTool({ name: 'legacy', description: 'L', execute: () => 'hi' })
    await settle()
    expect(await hostCall(h, 'legacy')).toEqual({ data: { text: 'hi' } })
    navMc().unregisterTool('legacy')
    await settle()
    expect(coreTool(h, 'legacy')).toBeUndefined()
  })
})

describe('注销与上下文', () => {
  it('unregisterTool（旧接口）', async () => {
    await mc().registerTool({ name: 'a', description: 'A', execute: () => null })
    await settle()
    expect(coreTool(h, 'a')).toBeDefined()
    mc().unregisterTool('a')
    await settle()
    expect(coreTool(h, 'a')).toBeUndefined()
    // 可以再次注册
    await mc().registerTool({ name: 'a', description: 'A2', execute: () => null })
    await settle()
    expect(coreTool(h, 'a')?.description).toBe('A2')
  })

  it('registerTool 的 signal：abort 即注销（标准写法）', async () => {
    const ac = new AbortController()
    await mc().registerTool({ name: 'a', description: 'A', execute: () => null }, { signal: ac.signal })
    await settle()
    ac.abort()
    await settle()
    expect(coreTool(h, 'a')).toBeUndefined()
  })

  it('已 abort 的 signal：拒绝并返回 abort reason', async () => {
    const ac = new AbortController()
    ac.abort(new Error('早就取消了'))
    await expect(mc().registerTool({ name: 'a', description: 'A', execute: () => null }, { signal: ac.signal })).rejects.toThrow('早就取消了')
    await settle()
    expect(coreTool(h, 'a')).toBeUndefined()
  })

  it('返回值的 unregister()（兼容旧版 MCP-B polyfill）', async () => {
    const reg = mc().registerTool({ name: 'a', description: 'A', execute: () => null })
    await reg
    reg.unregister()
    await settle()
    expect(coreTool(h, 'a')).toBeUndefined()
  })

  it('provideContext 替换经 modelContext 注册的工具，不影响 appMcp.tool()', async () => {
    h.app.tool('own', { description: '自有', handler: () => 1 })
    await mc().registerTool({ name: 'a', description: 'A', execute: () => null })
    mc().provideContext({
      tools: [
        { name: 'b', description: 'B', execute: () => null },
        { name: 'c', description: 'C', execute: () => null },
      ],
    })
    await settle()
    expect(coreTool(h, 'a')).toBeUndefined()
    expect(coreTool(h, 'b')).toBeDefined()
    expect(coreTool(h, 'c')).toBeDefined()
    expect(coreTool(h, 'own')).toBeDefined()

    mc().clearContext()
    await settle()
    expect(coreTool(h, 'b')).toBeUndefined()
    expect(coreTool(h, 'c')).toBeUndefined()
    expect(coreTool(h, 'own')).toBeDefined()
  })

  it('provideContext 中的工具不能与 appMcp.tool() 重名', () => {
    h.app.tool('own', { description: '自有', handler: () => 1 })
    expect(() => mc().provideContext({ tools: [{ name: 'own', description: 'x', execute: () => null }] })).toThrow(
      expect.objectContaining({ name: 'InvalidStateError' }),
    )
  })
})

describe('校验与冲突', () => {
  it('按标准拒绝：重名 / 非法名 / 空描述 → InvalidStateError；缺 execute → TypeError', async () => {
    await mc().registerTool({ name: 'a', description: 'A', execute: () => null })
    const reject = (p: Promise<unknown>, name: string) => expect(p).rejects.toMatchObject({ name })
    await reject(mc().registerTool({ name: 'a', description: 'A', execute: () => null }), 'InvalidStateError')
    await reject(mc().registerTool({ name: 'has space', description: 'A', execute: () => null }), 'InvalidStateError')
    await reject(mc().registerTool({ name: '', description: 'A', execute: () => null }), 'InvalidStateError')
    await reject(mc().registerTool({ name: 'b', description: '', execute: () => null }), 'InvalidStateError')
    await reject(mc().registerTool({ name: 'c', description: 'C' } as never), 'TypeError')
    const circular: Record<string, unknown> = { type: 'object' }
    circular.self = circular
    await reject(mc().registerTool({ name: 'd', description: 'D', inputSchema: circular, execute: () => null }), 'TypeError')
    await reject(
      mc().registerTool({ name: 'e', description: 'E', execute: () => null }, { exposedTo: ['http://evil.example'] }),
      'SecurityError',
    )
  })

  it('标准侧与 appMcp.tool() 重名：appMcp 已有时标准侧 InvalidStateError', async () => {
    h.app.tool('cart.clear', { description: '清空', handler: () => 1 })
    await expect(mc().registerTool({ name: 'cart.clear', description: 'x', execute: () => 2 })).rejects.toMatchObject({
      name: 'InvalidStateError',
    })
    expect(await hostCall(h, 'cart.clear')).toEqual({ data: 1 })
  })

  it('标准侧先注册、appMcp.tool() 后注册：appMcp 优先，标准侧版本被移除', async () => {
    await mc().registerTool({ name: 'cart.clear', description: 'x', execute: () => 2 })
    await settle()
    h.app.tool('cart.clear', { description: '清空', handler: () => 1 })
    await settle()
    expect(h.logger.warn).toHaveBeenCalledWith(expect.stringContaining('appMcp 优先'))
    expect(coreTool(h, 'cart.clear')?.description).toBe('清空')
    expect(await hostCall(h, 'cart.clear')).toEqual({ data: 1 })
    expect((await mc().getTools()).filter((t) => t.name === 'cart.clear')).toHaveLength(1)
  })

  it('65–128 个字符的名字：标准侧接受，但不暴露给 Host', async () => {
    const name = 'x'.repeat(100)
    await mc().registerTool({ name, description: 'long', execute: () => 'ok' })
    expect(h.logger.warn).toHaveBeenCalledWith(expect.stringContaining('超过 64 个字符'))
    expect((await mc().getTools()).map((t) => t.name)).toContain(name)
  })
})

describe('事件', () => {
  it('toolchange：注册与注销时派发（ontoolchange 与 addEventListener）', async () => {
    const onchange = vi.fn()
    const listener = vi.fn()
    mc().ontoolchange = onchange
    mc().addEventListener('toolchange', listener)
    await mc().registerTool({ name: 'a', description: 'A', execute: () => null })
    expect(onchange).toHaveBeenCalledTimes(1)
    expect(listener).toHaveBeenCalledTimes(1)
    mc().unregisterTool('a')
    await settle()
    expect(onchange).toHaveBeenCalledTimes(2)
    mc().ontoolchange = null
    await mc().registerTool({ name: 'b', description: 'B', execute: () => null })
    expect(onchange).toHaveBeenCalledTimes(2)
    expect(listener).toHaveBeenCalledTimes(3)
  })

  it('Host 调用时派发 toolactivated；取消时派发 toolcancel 并 abort execute 的 signal', async () => {
    const events: string[] = []
    mc().addEventListener('toolactivated', (e) => events.push(`activated:${(e as unknown as { toolName: string }).toolName}`))
    mc().ontoolcancel = (e) => events.push(`cancel:${(e as unknown as { toolName: string }).toolName}`)
    let seen: AbortSignal | undefined
    await mc().registerTool({
      name: 'slow',
      description: 'S',
      execute: (_: unknown, { signal }: { signal: AbortSignal }) => {
        seen = signal
        return new Promise(() => {})
      },
    })
    await settle()
    const callId = startHostCall(h, 'slow')
    await settle()
    expect(events).toEqual(['activated:slow'])
    h.socket().script({ type: 'cancelTool', callId, reason: 'requested' })
    await settle()
    expect(events).toEqual(['activated:slow', 'cancel:slow'])
    expect(seen?.aborted).toBe(true)
    // 事件对象与标准一致
    const Ctor = (globalThis as unknown as { ToolActivatedEvent: new (t: string, i: object) => Event }).ToolActivatedEvent
    expect(new Ctor('toolactivated', { toolName: 'x' })).toMatchObject({ type: 'toolactivated', toolName: 'x' })
  })
})

describe('getTools / executeTool（页内智能体）', () => {
  it('getTools 列出标准侧工具与 appMcp.tool() 的工具（按名称排序）', async () => {
    h.app.tool('own.read', {
      description: '读取',
      risk: 'read',
      input: z.object({ id: z.string() }),
      handler: ({ id }) => ({ id }),
    })
    h.app.tool('own.hidden', { description: '禁用', enabled: false, handler: () => null })
    await mc().registerTool({
      name: 'add_todo',
      description: '添加',
      inputSchema: { type: 'object', properties: {} },
      annotations: { readOnlyHint: false },
      execute: () => null,
    })
    const tools = await mc().getTools()
    expect(tools.map((t) => t.name)).toEqual(['add_todo', 'own.read'])
    expect(tools[0]).toMatchObject({ description: '添加', origin: location.origin, window, annotations: { readOnlyHint: false } })
    expect(tools[1]).toMatchObject({ annotations: { readOnlyHint: true } })
    expect(tools[1]?.inputSchema).toMatchObject({ type: 'object', properties: { id: { type: 'string' } } })
  })

  it('executeTool 返回 JSON 字符串；appMcp.tool() 的工具返回 MCP 风格结果', async () => {
    h.app.tool('own.echo', { description: '回显', handler: (x) => ({ echo: x }) })
    await mc().registerTool({ name: 'add', description: '加法', execute: ({ a, b }: { a: number; b: number }) => a + b })
    const [add, own] = await mc().getTools()
    expect(await mc().executeTool(add!, { a: 1, b: 2 })).toBe('3')
    expect(JSON.parse((await mc().executeTool(own!, { v: 1 })) as string)).toEqual({
      content: [{ type: 'text', text: '{"echo":{"v":1}}' }],
    })
    await expect(mc().executeTool({ name: 'ghost' })).rejects.toMatchObject({ name: 'UnknownError' })
  })

  it('executeTool 的 signal：拒绝并派发 toolcancel', async () => {
    const cancel = vi.fn()
    mc().addEventListener('toolcancel', cancel)
    await mc().registerTool({ name: 'slow', description: 'S', execute: () => new Promise(() => {}) })
    const ac = new AbortController()
    const p = mc().executeTool({ name: 'slow' }, {}, { signal: ac.signal })
    ac.abort(new Error('stop'))
    await expect(p).rejects.toThrow('stop')
    expect(cancel).toHaveBeenCalledTimes(1)
  })

  it('mirrorOwnTools: false 时 getTools 不含 appMcp.tool() 的工具', async () => {
    uninstall?.()
    uninstall = installWebMcp(h.app, { mirrorOwnTools: false })
    h.app.tool('own', { description: '自有', handler: () => 1 })
    expect(await mc().getTools()).toEqual([])
  })
})

describe('enabled: false', () => {
  it('空操作实例也能安装，标准侧 API 仍可用', async () => {
    const { createAppMcp } = await import('../src/index')
    uninstall?.()
    const disabled = createAppMcp({ appId: 'off', appName: 'off', enabled: false })
    uninstall = installWebMcp(disabled)
    await mc().registerTool({ name: 'a', description: 'A', execute: () => 7 })
    const [tool] = await mc().getTools()
    expect(await mc().executeTool(tool!, {})).toBe('7')
  })
})
