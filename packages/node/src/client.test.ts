import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, ToolCallError, type AppMcp, type ConnectionState } from './index.js'
import { setZodImporter } from './schema.js'
import { FakeNativeClient, fakeBinding } from './testing/fake-native.js'

const created: AppMcp[] = []

function setup(extra: Partial<Parameters<typeof createAppMcp>[0]> = {}) {
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const app = createAppMcp({ appId: 'demo', appName: 'Demo', binding: fakeBinding, logger, keepAlive: false, ...extra })
  created.push(app)
  const native = FakeNativeClient.last as FakeNativeClient
  return { app, native, logger }
}

afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

/** 最小的 zod v4 形状（不依赖 zod 包）。 */
function fakeZod<T>(schema: object, parse: (v: unknown) => T, withMethod = true) {
  return {
    _zod: {},
    parse,
    ...(withMethod && { toJSONSchema: () => schema }),
  }
}

describe('createAppMcp', () => {
  it('把选项传给原生客户端并自动 start', () => {
    const { native, app } = setup({
      clientKind: 'hybrid',
      hostUrl: 'ws://127.0.0.1:1',
      token: 't0',
      maxConcurrentCalls: 2,
      overview: { summary: '演示', locale: 'zh-CN' },
    })
    expect(native.config).toMatchObject({
      appId: 'demo',
      appName: 'Demo',
      clientKind: 'hybrid',
      hostUrl: 'ws://127.0.0.1:1',
      token: 't0',
      maxConcurrentCalls: 2,
      overview: { summary: '演示', locale: 'zh-CN' },
    })
    expect(native.started).toBe(1)
    expect(app.instanceId).toBe('fake-instance')
    expect(app.token).toBe('t0')
    app.start()
    expect(native.started).toBe(2) // 原生侧保证幂等
  })

  it('autoStart: false 时不连接', () => {
    const { native } = setup({ autoStart: false })
    expect(native.started).toBe(0)
  })

  it('enabled: false 时不加载原生模块，注册为空操作', () => {
    const app = createAppMcp({ appId: 'demo', appName: 'Demo', enabled: false })
    expect(app.state).toEqual({ status: 'disabled' })
    const t = app.tool('a', { description: 'a', handler: () => 1 })
    expect(t.name).toBe('a')
    t.update({ description: 'b' })
    t.dispose()
    app.scope('s').resource('r', { description: 'r', read: () => 1 }).notifyChanged()
    expect(app.token).toBeNull()
    app.dispose()
  })

  it('状态事件映射为 ConnectionState 并通知订阅者', () => {
    const { app, native } = setup()
    const seen: ConnectionState[] = []
    const off = app.onStateChange((s) => seen.push(s))
    native.emit({ type: 'state', state: { status: 'connecting' } })
    native.emit({ type: 'state', state: { status: 'pending-pairing' } })
    const before = Date.now()
    native.emit({ type: 'state', state: { status: 'backoff', retryInMs: 500 } })
    native.emit({ type: 'state', state: { status: 'rejected', reason: 'nope', code: 'PAIRING_REJECTED' } })
    off()
    native.emit({ type: 'state', state: { status: 'connected' } })
    expect(seen.map((s) => s.status)).toEqual(['connecting', 'pending-pairing', 'backoff', 'rejected'])
    const backoff = seen[2] as { retryAt: number }
    expect(backoff.retryAt).toBeGreaterThanOrEqual(before + 500)
    expect(seen[3]).toEqual({ status: 'rejected', reason: 'nope', code: 'PAIRING_REJECTED' })
    expect(app.state).toEqual({ status: 'connected' })
  })

  it('配对事件调用 onPaired，日志事件转给 logger', () => {
    const onPaired = vi.fn()
    const { native, logger, app } = setup({ onPaired })
    native.emit({ type: 'paired', token: 'tok' })
    expect(onPaired).toHaveBeenCalledWith('tok')
    expect(app.token).toBe('tok')
    native.emit({ type: 'log', level: 'warn', message: 'w' })
    native.emit({ type: 'log', level: 'info', message: 'i' })
    expect(logger.warn).toHaveBeenCalledWith('[app-mcp] w')
    expect(logger.debug).toHaveBeenCalledWith('[app-mcp] i')
  })

  it('setVisibility 转发给原生客户端', () => {
    const { app, native } = setup()
    app.setVisibility('hidden')
    expect(native.visibility).toEqual(['hidden', true])
    app.setVisibility('visible', false)
    expect(native.visibility).toEqual(['visible', false])
  })

  it('dispose 停止客户端、状态变为 stopped，之后注册为空操作', async () => {
    const { app, native } = setup()
    const states: string[] = []
    app.onStateChange((s) => states.push(s.status))
    let started!: () => void
    const running = new Promise<void>((r) => (started = r))
    app.tool('slow', {
      description: 'slow',
      handler: (_: unknown, { signal }) =>
        new Promise((_resolve, reject) => {
          started()
          signal.addEventListener('abort', () => reject(signal.reason))
        }),
    })
    const pending = native.call('slow')
    await running
    app.dispose()
    expect(native.stopped).toBe(true)
    expect(states).toEqual(['stopped'])
    expect(await pending).toMatchObject({ ok: false, kind: 'CANCELLED' })
    const t = app.tool('late', { description: 'late', handler: () => 1 })
    t.dispose()
    expect(native.tools.has('late')).toBe(false)
    app.dispose() // 幂等
  })

  it('keepAlive 定时器在 dispose 后清除', () => {
    const set = vi.spyOn(globalThis, 'setInterval')
    const clear = vi.spyOn(globalThis, 'clearInterval')
    const { app } = setup({ keepAlive: true })
    expect(set).toHaveBeenCalledTimes(1)
    app.dispose()
    expect(clear).toHaveBeenCalledTimes(1)
    set.mockRestore()
    clear.mockRestore()
  })
})

describe('工具', () => {
  it('注册 JSON Schema 工具并返回结果', async () => {
    const { app, native } = setup()
    const schema = { type: 'object', properties: { n: { type: 'number' } }, required: ['n'] } as const
    const handler = vi.fn((input: { n: number }, _ctx: { callId: string }) => ({ doubled: input.n * 2 }))
    app.tool('math.double', { description: '翻倍', input: schema, risk: 'read', activation: 'headless', title: 'Double', handler })
    const rec = native.tools.get('math.double')
    expect(rec?.spec).toEqual({
      name: 'math.double',
      description: '翻倍',
      enabled: true,
      inputSchemaJson: JSON.stringify(schema),
      risk: 'read',
      activation: 'headless',
      title: 'Double',
    })
    expect(await native.call('math.double', { n: 21 })).toEqual({ ok: true, data: { doubled: 42 }, stateHints: [] })
    expect(handler.mock.calls[0]?.[1]?.callId).toBe('c1')
  })

  it('无 input 时不带 schema；返回 undefined 时数据为 null', async () => {
    const { app, native } = setup()
    app.tool('noop', { description: 'noop', handler: () => undefined })
    expect(native.tools.get('noop')?.spec.inputSchemaJson).toBeUndefined()
    expect(await native.call('noop')).toEqual({ ok: true, data: null, stateHints: [] })
  })

  it('支持 { data, stateHints } 结果与异步 handler', async () => {
    const { app, native } = setup()
    app.tool('cart.add', {
      description: 'add',
      handler: async () => ({ data: { count: 1 }, stateHints: ['cart'] }),
    })
    // 含其他字段的对象按原样作为数据
    app.tool('raw', { description: 'raw', handler: () => ({ data: 1, other: 2 }) })
    expect(await native.call('cart.add')).toEqual({ ok: true, data: { count: 1 }, stateHints: ['cart'] })
    expect(await native.call('raw')).toEqual({ ok: true, data: { data: 1, other: 2 }, stateHints: [] })
  })

  it('注解与 outputSchema 随注册下发；update 可替换与清除', async () => {
    const { app, native } = setup()
    const output = { type: 'object', properties: { orderId: { type: 'string' } } }
    const t = app.tool('order.submit', {
      description: '下单',
      risk: 'payment',
      annotations: { idempotentHint: false, openWorldHint: true, title: '提交订单' },
      outputSchema: output,
      handler: () => ({ orderId: 'o1' }),
    })
    expect(native.tools.get('order.submit')?.spec).toMatchObject({
      risk: 'payment',
      annotations: { idempotentHint: false, openWorldHint: true, title: '提交订单' },
      outputSchemaJson: JSON.stringify(output),
    })
    // zod：按输出形态转换（根类型不限于 object）
    t.update({ outputSchema: fakeZod({ type: 'array' }, (v) => v) })
    expect(native.tools.get('order.submit')?.spec.outputSchemaJson).toBe('{"type":"array"}')
    t.update({ annotations: undefined, outputSchema: undefined })
    const spec = native.tools.get('order.submit')?.spec
    expect(spec?.annotations).toBeUndefined()
    expect(spec?.outputSchemaJson).toBeUndefined()
    expect(() => app.tool('bad', { description: 'b', outputSchema: [] as never, handler: () => 1 })).toThrow(
      /outputSchema/,
    )
  })

  it('旧版原生模块没有 updateWith 时 update 保留注册时的声明', () => {
    const { app, native } = setup()
    const t = app.tool('x', { description: 'x', annotations: { readOnlyHint: true }, handler: () => 1 })
    const rec = native.tools.get('x')!
    const tool = (t as unknown as { native: { updateWith?: unknown } }).native
    tool.updateWith = undefined
    t.update({ annotations: undefined, description: 'y' })
    expect(rec.spec).toMatchObject({ description: 'y', annotations: { readOnlyHint: true } })
  })

  it('结构化结果：status / stateResource / summary / 内容注解', async () => {
    const { app, native, logger } = setup()
    app.tool('order.pay', {
      description: '付款',
      handler: () => ({
        data: { orderId: 'o1' },
        status: 'pending' as const,
        stateResource: 'order.state',
        summary: '已提交，等待用户在 App 内付款',
        annotations: { audience: ['user' as const], priority: 0.5 },
        stateHints: ['cart'],
      }),
    })
    app.tool('order.none', { description: 'n', handler: () => ({ data: undefined, status: 'noop' as const }) })
    // 键属于信封但取值不合法：整体作为数据（不误判）
    app.tool<unknown, unknown>('plain', { description: 'p', handler: () => ({ data: [1], status: 'success' }) })
    app.tool('bad.audience', {
      description: 'b',
      handler: () => ({ data: 1, annotations: { audience: ['robot'] } }) as unknown as number,
    })
    expect(await native.call('order.pay')).toEqual({
      ok: true,
      data: { orderId: 'o1' },
      stateHints: ['cart'],
      status: 'pending',
      stateResource: 'order.state',
      summary: '已提交，等待用户在 App 内付款',
      annotations: { audience: ['user'], priority: 0.5 },
    })
    expect(await native.call('order.none')).toEqual({ ok: true, data: null, stateHints: [], status: 'noop' })
    expect(await native.call('plain')).toEqual({ ok: true, data: { data: [1], status: 'success' }, stateHints: [] })
    expect(await native.call('bad.audience')).toMatchObject({ ok: false, kind: 'HANDLER_ERROR' })
    expect(logger.error).not.toHaveBeenCalled()
  })

  it('旧版原生模块没有 completeWith：只提交 data / stateHints 并警告', async () => {
    const { app, native, logger } = setup()
    app.tool('t', { description: 't', handler: () => ({ data: 1, summary: '完成' }) })
    const { callId, result } = native.invoke('t', {})
    void callId
    const call = [...(native as unknown as { calls: Map<string, { completeWith?: unknown }> }).calls.values()][0]!
    call.completeWith = undefined
    expect(await result).toEqual({ ok: true, data: 1, stateHints: [] })
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('原生模块版本过旧'))
  })

  it('ToolCallError 映射为对应类别，其他异常为 HANDLER_ERROR', async () => {
    const { app, native } = setup()
    app.tool('reject', {
      description: 'r',
      handler: () => {
        throw new ToolCallError('USER_REJECTED', '用户拒绝')
      },
    })
    app.tool('boom', {
      description: 'b',
      handler: async () => {
        throw new Error('炸了')
      },
    })
    // 其他副本（如 @app-mcp/web）的 ToolCallError 按结构识别
    app.tool('foreign', {
      description: 'f',
      handler: () => {
        throw Object.assign(new Error('x'), { name: 'ToolCallError', kind: 'TOOL_DISABLED' })
      },
    })
    app.tool('cyclic', {
      description: 'c',
      handler: () => {
        const o: Record<string, unknown> = {}
        o.self = o
        return o
      },
    })
    expect(await native.call('reject')).toEqual({ ok: false, kind: 'USER_REJECTED', message: '用户拒绝' })
    expect(await native.call('boom')).toEqual({ ok: false, kind: 'HANDLER_ERROR', message: '炸了' })
    expect(await native.call('foreign')).toEqual({ ok: false, kind: 'TOOL_DISABLED', message: 'x' })
    expect(await native.call('cyclic')).toMatchObject({ ok: false, kind: 'HANDLER_ERROR' })
  })

  it('取消时触发 AbortSignal，之后的完成被忽略', async () => {
    const { app, native, logger } = setup()
    let signal!: AbortSignal
    let resolveHandler!: (v: unknown) => void
    let started!: () => void
    const running = new Promise<void>((r) => (started = r))
    app.tool('slow', {
      description: 'slow',
      handler: (_input, ctx) => {
        signal = ctx.signal
        started()
        return new Promise<unknown>((r) => (resolveHandler = r))
      },
    })
    const { callId, result } = native.invoke('slow')
    await running
    native.cancel(callId, 'timeout')
    expect(await result).toMatchObject({ ok: false, kind: 'TIMEOUT' })
    await Promise.resolve()
    expect(signal.aborted).toBe(true)
    expect((signal.reason as ToolCallError).kind).toBe('TIMEOUT')
    resolveHandler(1)
    await new Promise((r) => setTimeout(r, 0))
    expect(logger.error).not.toHaveBeenCalled()
  })

  it('zod schema：转换为 JSON Schema，调用前 parse', async () => {
    const { app, native } = setup()
    const schema = { type: 'object', properties: { q: { type: 'string' } } }
    const parse = vi.fn((v: unknown) => {
      const q = (v as { q?: unknown }).q
      if (typeof q !== 'string') throw new Error('q 必须是字符串')
      return { q: q.trim() }
    })
    const handler = vi.fn((input: { q: string }) => input.q)
    app.tool('search', { description: 's', input: fakeZod(schema, parse), handler })
    expect(native.tools.get('search')?.spec.inputSchemaJson).toBe(JSON.stringify(schema))
    expect(await native.call('search', { q: '  hi ' })).toEqual({ ok: true, data: 'hi', stateHints: [] })
    expect(await native.call('search', { q: 1 })).toEqual({ ok: false, kind: 'INVALID_INPUT', message: '参数校验失败：q 必须是字符串' })
    expect(handler).toHaveBeenCalledTimes(1)
  })

  it('zod schema 无 toJSONSchema 方法时动态加载 zod 后注册', async () => {
    const schema = { type: 'object', properties: {} }
    const toJSONSchema = vi.fn(() => schema)
    setZodImporter(async () => ({ toJSONSchema }))
    try {
      const { app, native } = setup()
      const z = fakeZod(schema, (v) => v, false)
      const t = app.tool('lazy', { description: 'lazy', input: z, handler: () => 'ok' })
      expect(native.tools.has('lazy')).toBe(false)
      t.update({ description: 'lazy2' })
      await vi.waitFor(() => expect(native.tools.has('lazy')).toBe(true))
      expect(toJSONSchema).toHaveBeenCalledWith(z, expect.objectContaining({ io: 'input' }))
      expect(native.tools.get('lazy')?.spec.description).toBe('lazy2')
      expect(await native.call('lazy')).toMatchObject({ ok: true, data: 'ok' })

      // 注册完成前 dispose：不再注册
      app.tool('gone', { description: 'g', input: z, handler: () => 1 }).dispose()
      await new Promise((r) => setTimeout(r, 0))
      expect(native.tools.has('gone')).toBe(false)
    } finally {
      setZodImporter(async () => {
        throw new Error('reset')
      })
    }
  })

  it('带 toJSONSchema() 的对象', () => {
    const { app, native } = setup()
    app.tool('t', { description: 't', input: { toJSONSchema: () => ({ type: 'object' as const }) }, handler: () => 1 })
    expect(native.tools.get('t')?.spec.inputSchemaJson).toBe('{"type":"object"}')
  })

  it('非 object 的 schema 同步抛错；重名由原生层报错', () => {
    const { app } = setup()
    expect(() =>
      app.tool('bad', { description: 'b', input: { type: 'string' } as never, handler: () => 1 }),
    ).toThrow(TypeError)
    app.tool('dup', { description: 'd', handler: () => 1 })
    expect(() => app.tool('dup', { description: 'd', handler: () => 1 })).toThrow(/already registered/)
  })

  it('update / setHandler / dispose', async () => {
    const { app, native } = setup()
    const t = app.tool('x', { description: 'x', risk: 'write', handler: () => 'old' })
    t.update({ enabled: false, input: { type: 'object', properties: { a: { type: 'string' } } } })
    expect(native.tools.get('x')?.spec).toMatchObject({
      description: 'x',
      risk: 'write',
      enabled: false,
      inputSchemaJson: '{"type":"object","properties":{"a":{"type":"string"}}}',
    })
    t.update({ input: undefined })
    expect(native.tools.get('x')?.spec.inputSchemaJson).toBeUndefined()
    t.setHandler(() => 'new')
    expect(await native.call('x')).toMatchObject({ data: 'new' })
    t.dispose()
    t.dispose()
    expect(native.tools.has('x')).toBe(false)
    t.update({ description: 'ignored' })
  })
})

describe('资源与 scope', () => {
  it('读取资源并支持 setReader / notifyChanged', async () => {
    const { app, native } = setup()
    const r = app.resource('cart', { description: '购物车', read: () => ({ items: 2 }) })
    expect(native.resources.get('cart')?.spec).toEqual({ name: 'cart', description: '购物车', mimeType: 'application/json' })
    expect(await native.read('cart')).toMatchObject({ ok: true, data: { items: 2 } })
    r.setReader(async () => 'text')
    expect(await native.read('cart')).toMatchObject({ ok: true, data: 'text' })
    r.notifyChanged()
    expect(native.resourceChanges('cart')).toBe(1)
    r.setReader(() => {
      throw new ToolCallError('RESOURCE_NOT_FOUND', '没有')
    })
    expect(await native.read('cart')).toEqual({ ok: false, kind: 'RESOURCE_NOT_FOUND', message: '没有' })
    r.dispose()
    expect(native.resources.has('cart')).toBe(false)
    // realtime 只在声明时传给原生侧（spec/lifecycle.md 第 13 节 B3）
    app.resource('order', { description: '订单', realtime: true, read: () => 1 })
    expect(native.resources.get('order')?.spec).toMatchObject({ name: 'order', realtime: true })
  })

  it('scope 注销时递归注销子项，子句柄变为空操作', () => {
    const { app, native } = setup()
    const s = app.scope('page')
    expect(s.name).toBe('page')
    s.tool('a', { description: 'a', handler: () => 1 })
    const inner = s.scope('dialog')
    const b = inner.tool('b', { description: 'b', handler: () => 1 })
    inner.resource('r', { description: 'r', read: () => 1 })
    app.tool('root', { description: 'root', handler: () => 1 })
    expect([...native.tools.keys()].sort()).toEqual(['a', 'b', 'root'])
    s.dispose()
    expect([...native.tools.keys()]).toEqual(['root'])
    expect(native.resources.size).toBe(0)
    b.update({ description: 'x' }) // 已随 scope 注销：不抛错
    b.dispose()
    // 已注销 scope 上的注册为空操作
    s.tool('c', { description: 'c', handler: () => 1 })
    expect(native.tools.has('c')).toBe(false)
  })
})

describe('生命周期', () => {
  it('lifecycle 与 connectTimeoutMs 传给原生客户端', () => {
    const { native } = setup({
      lifecycle: {
        mode: 'idle',
        idleTimeoutMs: 1000,
        residency: 'exit-when-idle',
        wake: { kind: 'uri', target: 'demo://' },
        hostAbsentRetries: 5,
        legacyTimers: true,
        mergeWindowMs: 500,
        sleepOnBackground: true,
      },
      connectTimeoutMs: 2000,
      heartbeat: 'off',
    })
    expect(native.config).toMatchObject({
      lifecycle: {
        mode: 'idle',
        idleTimeoutMs: 1000,
        residency: 'exit-when-idle',
        wake: { kind: 'uri', target: 'demo://' },
        hostAbsentRetries: 5,
        legacyTimers: true,
        mergeWindowMs: 500,
        sleepOnBackground: true,
      },
      connectTimeoutMs: 2000,
      heartbeat: 'off',
    })
  })

  it('handleWake 接受字符串或数组，识别到即返回 true', () => {
    const { app, native } = setup()
    expect(app.handleWake(['/usr/bin/app', '--flag'])).toBe(false)
    expect(app.handleWake(['/usr/bin/app', 'app-mcp-wake:tok'])).toBe(true)
    expect(app.handleWake('demo://app-mcp/wake?token=x')).toBe(true)
    expect(app.handleWake('')).toBe(false)
    expect(native.lifecycleCalls).toEqual([
      'handleWake:/usr/bin/app',
      'handleWake:--flag',
      'handleWake:/usr/bin/app',
      'handleWake:app-mcp-wake:tok',
      'handleWake:demo://app-mcp/wake?token=x',
    ])
  })

  it('wake / connectNow / sleep / toolsHash 转发给原生客户端', () => {
    const { app, native } = setup()
    expect(app.wake()).toBe(true)
    expect(app.wake('visible')).toBe(true)
    expect(app.connectNow()).toBe(true)
    expect(app.sleep()).toBe(true)
    expect(native.lifecycleCalls).toEqual(['wake:app', 'wake:visible', 'connectNow', 'sleep:app'])
    app.tool('t', { description: 't', handler: () => 1 })
    expect(app.toolsHash()).toBe('fake-hash-1-0')
  })

  it('hold 句柄幂等释放；ToolContext.hold 在 handler 返回后仍保持', async () => {
    const { app, native } = setup()
    const h = app.hold()
    expect(native.activeHolds).toBe(1)
    h.release()
    h.release()
    expect(native.activeHolds).toBe(0)

    let held: { release(): void } | undefined
    app.tool('bg', {
      description: 'bg',
      handler: (_input, ctx) => {
        held = ctx.hold()
        return 'ok'
      },
    })
    expect(await native.call('bg')).toMatchObject({ ok: true })
    expect(native.activeHolds).toBe(1)
    held!.release()
    expect(native.activeHolds).toBe(0)
  })

  it('ToolContext.progress 交给原生层；调用结束后无副作用、不抛出', async () => {
    const { app, native } = setup()
    let saved: { progress(p: number, t?: number, m?: string): void } | undefined
    app.tool('export', {
      description: '导出',
      handler: (_input, ctx) => {
        ctx.progress(1, 3, '第 1 页')
        ctx.progress(2)
        saved = ctx
        return 'ok'
      },
    })
    expect(await native.call('export')).toMatchObject({ ok: true })
    expect(native.progressReports.map(({ progress, total, message }) => [progress, total, message])).toEqual([
      [1, 3, '第 1 页'],
      [2, null, null],
    ])
    expect(() => saved!.progress(3)).not.toThrow()
    expect(native.progressReports).toHaveLength(2)
  })

  it('idle-exit 事件调用 onIdleExit 与订阅者；dispose 后清除', () => {
    const onIdleExit = vi.fn()
    const { app, native, logger } = setup({ onIdleExit })
    const listener = vi.fn(() => {
      throw new Error('x')
    })
    const off = app.onIdleExit(listener)
    native.emit({ type: 'idle-exit' })
    expect(onIdleExit).toHaveBeenCalledTimes(1)
    expect(listener).toHaveBeenCalledTimes(1)
    expect(logger.error).toHaveBeenCalled()
    off()
    native.emit({ type: 'idle-exit' })
    expect(listener).toHaveBeenCalledTimes(1)
    expect(onIdleExit).toHaveBeenCalledTimes(2)
  })

  it('enabled: false 或 dispose 后生命周期方法为空操作', () => {
    const app = createAppMcp({ appId: 'demo', appName: 'Demo', enabled: false })
    expect(app.handleWake('app-mcp-wake:t')).toBe(false)
    expect(app.wake()).toBe(false)
    expect(app.connectNow()).toBe(false)
    expect(app.sleep()).toBe(false)
    expect(app.toolsHash()).toBe('')
    app.hold().release()

    const { app: app2, native } = setup()
    app2.dispose()
    expect(app2.wake()).toBe(false)
    app2.hold().release()
    expect(native.lifecycleCalls).toEqual([])
    expect(native.activeHolds).toBe(0)
  })
})

describe('错误详情', () => {
  it('ToolCallError 的 details 与 zod 校验问题随错误发送', async () => {
    const { app, native } = setup()
    app.tool('conflict', {
      description: 'c',
      handler: () => {
        throw new ToolCallError('HANDLER_ERROR', '冲突', { retryAfterMs: 100 })
      },
    })
    const schema = fakeZod({ type: 'object', properties: { n: { type: 'number' } } }, () => {
      throw Object.assign(new Error('bad'), { issues: [{ path: ['n'], message: '应为数字' }] })
    })
    app.tool('validated', { description: 'v', input: schema as never, handler: () => 1 })
    expect(await native.call('conflict')).toEqual({
      ok: false,
      kind: 'HANDLER_ERROR',
      message: '冲突',
      details: { retryAfterMs: 100 },
    })
    expect(await native.call('validated', { n: 'x' })).toEqual({
      ok: false,
      kind: 'INVALID_INPUT',
      message: '参数校验失败：n: 应为数字',
      details: { issues: [{ path: 'n', message: '应为数字' }] },
    })
  })
})

describe('惰性 handler', () => {
  it('首次调用时加载一次，支持 { default } 与并发调用', async () => {
    const { app, native } = setup()
    const load = vi.fn(async () => ({ default: (input: { a: number }) => input.a * 2 }))
    app.tool<{ a: number }, number>('lazy', { description: 'l', load })
    expect(native.tools.has('lazy')).toBe(true)
    expect(load).not.toHaveBeenCalled()
    const [r1, r2] = await Promise.all([native.call('lazy', { a: 1 }), native.call('lazy', { a: 2 })])
    expect(r1).toMatchObject({ ok: true, data: 2 })
    expect(r2).toMatchObject({ ok: true, data: 4 })
    expect(await native.call('lazy', { a: 3 })).toMatchObject({ ok: true, data: 6 })
    expect(load).toHaveBeenCalledTimes(1)
  })

  it('加载失败返回 HANDLER_ERROR，下次调用重试', async () => {
    const { app, native } = setup()
    let n = 0
    app.tool('lazy', {
      description: 'l',
      load: async () => {
        if (n++ === 0) throw new Error('网络错误')
        return () => 'ok'
      },
    })
    expect(await native.call('lazy')).toEqual({
      ok: false,
      kind: 'HANDLER_ERROR',
      message: '加载工具 lazy 的 handler 失败：网络错误',
    })
    expect(await native.call('lazy')).toMatchObject({ ok: true, data: 'ok' })
  })

  it('handler 与 load 必须恰好给出一个；setHandler 之后不再加载', async () => {
    const { app, native } = setup()
    expect(() => app.tool('both', { description: 'b', handler: () => 1, load: async () => () => 1 } as never)).toThrow(
      /二选一/,
    )
    expect(() => app.tool('none', { description: 'n' } as never)).toThrow(/缺少 handler/)
    const load = vi.fn(async () => () => 'loaded')
    const handle = app.tool('lazy', { description: 'l', load })
    handle.setHandler(() => 'direct')
    expect(await native.call('lazy')).toMatchObject({ ok: true, data: 'direct' })
    expect(load).not.toHaveBeenCalled()
  })
})
