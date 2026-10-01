import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import { AppMcpDriver } from '../src/driver'
import { instanceIdKey, tokenKey } from '../src/storage'
import { type ConnectionState, ToolCallError, type ToolContext } from '../src/types'
import { connected, deferred, setup, settle } from './fakes'

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('创建与加载', () => {
  it('把配置交给核心，并在启动前设置可见性', async () => {
    localStorage.setItem(tokenKey('shop'), 'tk-1')
    const h = setup({
      appVersion: '1.2.3',
      maxConcurrentCalls: 3,
      overview: { summary: '演示商城', body: '## 能力范围', locale: 'zh-CN' },
    })
    await settle()
    expect(h.core.config).toMatchObject({
      appId: 'shop',
      appName: '示例商城',
      instanceId: h.app.instanceId,
      clientKind: 'web',
      appVersion: '1.2.3',
      maxConcurrentCalls: 3,
      token: 'tk-1',
      overview: { summary: '演示商城', body: '## 能力范围', locale: 'zh-CN' },
    })
    expect(h.core.methods().slice(0, 2)).toEqual(['setVisibility', 'start'])
    expect(h.sockets).toHaveLength(1)
    expect(h.socket().url).toBe('ws://127.0.0.1:7717/app')
  })

  it('hostUrl 可覆盖', async () => {
    const h = setup({ hostUrl: 'ws://127.0.0.1:9999' })
    await settle()
    expect(h.socket().url).toBe('ws://127.0.0.1:9999')
  })

  it('无效 appId 抛错', () => {
    expect(() => setup({ appId: 'Shop!' })).toThrow(/appId/)
  })

  it('加载失败进入 rejected 状态', async () => {
    const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
    const app = new AppMcpDriver(
      { appId: 'shop', appName: 'x', logger },
      { loadCore: () => Promise.reject(new Error('404')) },
    )
    const states: ConnectionState[] = []
    app.onStateChange((s) => states.push(s))
    await settle()
    expect(app.state).toEqual({ status: 'rejected', reason: 'WASM 核心加载失败：404' })
    expect(states).toHaveLength(1)
    expect(logger.error).toHaveBeenCalled()
  })

  it('把 wasmUrl 交给加载器', async () => {
    const loadCore = vi.fn(() => new Promise<never>(() => {}))
    new AppMcpDriver({ appId: 'shop', appName: 'x', wasmUrl: '/x.wasm' }, { loadCore })
    expect(loadCore).toHaveBeenCalledWith('/x.wasm')
  })
})

describe('注册缓存与回放', () => {
  it('加载前的注册在加载后按顺序交给核心，然后启动', async () => {
    const h = setup({}, true)
    const scope = h.app.scope('cart')
    h.app.tool('todo.add', { description: '添加待办', handler: () => {} })
    const inner = scope.tool('cart.clear', { description: '清空', risk: 'destructive', handler: () => {} })
    scope.resource('cart.state', { description: '购物车', read: () => [] })
    const dropped = h.app.tool('temp', { description: '临时', handler: () => {} })
    dropped.dispose()
    inner.update({ enabled: false })
    expect(h.core.calls).toHaveLength(0)

    await h.load()
    expect(h.core.methods()).toEqual([
      'setVisibility',
      'createScope',
      'registerTool',
      'registerTool',
      'registerResource',
      'updateTool',
      'start',
    ])
    const scopeId = 1
    expect(h.core.callsOf('createScope')[0]).toEqual(['cart', undefined])
    expect(h.core.callsOf('registerTool')[0]?.[0]).toEqual({
      name: 'todo.add',
      description: '添加待办',
      inputSchema: { type: 'object', properties: {} },
    })
    expect(h.core.callsOf('registerTool')[1]?.[0]).toMatchObject({ name: 'cart.clear', risk: 'destructive', scope: scopeId })
    expect(h.core.callsOf('registerResource')[0]?.[0]).toEqual({
      name: 'cart.state',
      description: '购物车',
      mimeType: 'application/json',
      scope: scopeId,
    })
    expect(h.core.callsOf('updateTool')[0]).toEqual([3, { enabled: false }])
  })

  it('加载后的注册立即执行，handle 行为一致', async () => {
    const h = setup()
    await settle()
    h.core.calls = []
    const t = h.app.tool('a', { description: 'A', title: 'T', activation: 'background', handler: () => {} })
    expect(h.core.methods()).toEqual(['registerTool'])
    t.update({ description: 'B', title: undefined })
    expect(h.core.callsOf('updateTool')[0]?.[1]).toEqual({ description: 'B', title: null })
    t.dispose()
    t.dispose()
    expect(h.core.methods()).toEqual(['registerTool', 'updateTool', 'unregisterTool'])
  })

  it('空更新不产生核心调用', async () => {
    const h = setup()
    await settle()
    const t = h.app.tool('a', { description: 'A', handler: () => {} })
    h.core.calls = []
    t.update({})
    expect(h.core.calls).toHaveLength(0)
  })

  it('scope.dispose 注销核心 scope，JS 侧名称可重新注册', async () => {
    const h = setup()
    await settle()
    const s = h.app.scope('cart')
    const child = s.scope('inner')
    const t = child.tool('x', { description: 'x', handler: () => {} })
    s.dispose()
    expect(h.core.callsOf('disposeScope')).toEqual([[1]])
    expect(h.core.callsOf('createScope')[1]).toEqual(['inner', 1])
    h.core.calls = []
    t.dispose()
    child.tool('y', { description: 'y', handler: () => {} })
    expect(h.core.calls).toHaveLength(0)
    expect(() => h.app.tool('x', { description: 'x', handler: () => {} })).not.toThrow()
  })

  it('父 scope 先 dispose 后，子 scope / 工具 / 资源的 dispose 安全且幂等', async () => {
    const h = setup()
    await settle()
    const parent = h.app.scope('page')
    const child = parent.scope('panel')
    const t = child.tool('t', { description: '', handler: () => {} })
    const r = parent.resource('r', { description: '', read: () => 1 })
    parent.dispose()
    h.core.calls = []
    for (let i = 0; i < 2; i++) {
      t.dispose()
      r.dispose()
      child.dispose()
      parent.dispose()
    }
    expect(h.core.calls).toHaveLength(0)
    expect(h.logger.error).not.toHaveBeenCalled()
  })

  it('名称校验与重名', async () => {
    const h = setup()
    expect(() => h.app.tool('bad name', { description: '', handler: () => {} })).toThrow(/工具名/)
    h.app.tool('a', { description: '', handler: () => {} })
    expect(() => h.app.tool('a', { description: '', handler: () => {} })).toThrow(/已注册/)
    h.app.resource('r', { description: '', read: () => 1 })
    expect(() => h.app.resource('r', { description: '', read: () => 1 })).toThrow(/已注册/)
  })

  it('核心拒绝注册时记录错误，不影响后续注册', async () => {
    const h = setup({}, true)
    h.core.rejectNames.add('a')
    h.app.tool('a', { description: '', handler: () => {} })
    h.app.tool('b', { description: '', handler: () => {} })
    await h.load()
    expect(h.core.callsOf('registerTool')).toHaveLength(2)
    expect(h.logger.error).toHaveBeenCalledWith(expect.stringContaining('注册工具 a 失败'), expect.anything())
    expect(h.core.methods()).toContain('start')
  })

  it('zod 输入转换为 JSON Schema', async () => {
    const h = setup()
    h.app.tool('a', {
      description: '',
      input: z.object({ id: z.string().describe('商品 ID'), qty: z.number().default(1) }),
      handler: () => {},
    })
    h.app.tool('b', { description: '', input: { type: 'object', properties: { x: { type: 'string' } } }, handler: () => {} })
    h.app.tool('c', {
      description: '',
      input: { toJSONSchema: () => ({ type: 'object', properties: { y: { type: 'number' } } }) },
      handler: () => {},
    })
    await settle()
    const schemas = h.core.callsOf('registerTool').map((c) => (c[0] as { inputSchema: unknown }).inputSchema)
    expect(schemas[0]).toMatchObject({
      type: 'object',
      properties: { id: { type: 'string', description: '商品 ID' }, qty: { type: 'number', default: 1 } },
      required: ['id'],
    })
    expect(schemas[1]).toEqual({ type: 'object', properties: { x: { type: 'string' } } })
    expect(schemas[2]).toEqual({ type: 'object', properties: { y: { type: 'number' } } })
  })

  it('dispose 后的注册为空操作', async () => {
    const h = setup()
    await settle()
    h.app.dispose()
    h.core.calls = []
    const t = h.app.tool('a', { description: '', handler: () => {} })
    t.update({ enabled: false })
    t.dispose()
    expect(h.core.calls).toHaveLength(0)
  })
})

describe('连接', () => {
  it('Connect → 创建 WebSocket；open → handleConnected；message → handleMessage', async () => {
    const h = setup()
    const states: ConnectionState[] = []
    h.app.onStateChange((s) => states.push(s))
    await settle()
    expect(h.app.state).toEqual({ status: 'connecting' })
    h.socket().open()
    expect(h.core.methods()).toContain('handleConnected')
    expect(h.app.state).toEqual({ status: 'connected' })
    expect(states.map((s) => s.status)).toEqual(['connecting', 'connected'])
    h.socket().receive('hello')
    expect(h.core.callsOf('handleMessage')[0]).toEqual(['hello', 1000])
    h.socket().receive(new ArrayBuffer(1))
    expect(h.core.callsOf('handleMessage')).toHaveLength(1)
  })

  it('Send → ws.send', async () => {
    const h = await connected()
    h.socket().script({ type: 'send', text: '{"a":1}' })
    expect(h.socket().sent).toEqual(['{"a":1}'])
  })

  it('连接未打开时丢弃 Send 并警告', async () => {
    const h = setup()
    await settle()
    h.core.emit({ type: 'send', text: 'x' })
    h.app.tool('t', { description: '', handler: () => {} })
    expect(h.socket().sent).toEqual([])
    expect(h.logger.warn).toHaveBeenCalled()
  })

  it('Disconnect → 主动关闭，解除回调，不调用 handleDisconnected', async () => {
    const h = await connected()
    const s = h.socket()
    s.script({ type: 'disconnect' })
    expect(s.closed).toBe(true)
    expect(s.onclose).toBeNull()
    s.fail()
    expect(h.core.methods()).not.toContain('handleDisconnected')
  })

  it('error + close 只调用一次 handleDisconnected，重连创建新连接', async () => {
    const h = await connected()
    const first = h.socket()
    first.fail()
    expect(h.core.callsOf('handleDisconnected')).toHaveLength(1)
    h.core.emit({ type: 'connect' })
    h.app.tool('t', { description: '', handler: () => {} }) // 触发一次 pump
    expect(h.sockets).toHaveLength(2)
    first.receive('late')
    expect(h.core.callsOf('handleMessage')).toHaveLength(0)
  })

  it('未指定 hostUrl：连接失败时依次尝试候选端口 7717 → 7737 → 7757', async () => {
    const h = setup()
    await settle()
    const urls = () => h.sockets.map((s) => s.url)
    const retry = (): void => {
      h.core.emit({ type: 'connect' })
      h.app.tool(`t${h.sockets.length}`, { description: '', handler: () => {} }) // 触发一次 pump
    }
    h.socket().fail()
    retry()
    h.socket().fail()
    retry()
    h.socket().fail()
    retry()
    expect(urls()).toEqual([
      'ws://127.0.0.1:7717/app',
      'ws://127.0.0.1:7737/app',
      'ws://127.0.0.1:7757/app',
      'ws://127.0.0.1:7717/app',
    ])
    // 连上之后断开：先重试同一端口（连接曾建立，不是端口不对）
    h.socket().open()
    h.socket().fail()
    retry()
    expect(urls().at(-1)).toBe('ws://127.0.0.1:7717/app')
  })

  it('对端不是 app-mcp：换下一个候选端口；都不是时停在 host-mismatch', async () => {
    const h = await connected()
    const states: ConnectionState[] = []
    h.app.onStateChange((s) => states.push(s))
    h.core.setState({ status: 'host-mismatch', reason: '不是 app-mcp' })
    h.app.tool('a', { description: '', handler: () => {} }) // 触发一次 pump
    expect(h.core.callsOf('connectNow')).toHaveLength(1)
    expect(h.socket().url).toBe('ws://127.0.0.1:7737/app')
    // （假核心的 handleConnected 直接进入 connected，会清零计数；真实核心此时在 handshaking，这里不打开连接）
    h.core.setState({ status: 'host-mismatch', reason: '不是 app-mcp' })
    h.app.tool('b', { description: '', handler: () => {} })
    expect(h.socket().url).toBe('ws://127.0.0.1:7757/app')
    expect(states.some((s) => s.status === 'host-mismatch')).toBe(false)
    h.core.setState({ status: 'host-mismatch', reason: '不是 app-mcp' })
    h.app.tool('c', { description: '', handler: () => {} })
    expect(h.app.state).toEqual({ status: 'host-mismatch', reason: '不是 app-mcp' })
    expect(h.logger.warn).toHaveBeenCalledWith('[app-mcp] 不是 app-mcp')
    // connectNow：再试一轮
    h.app.connectNow()
    expect(h.core.callsOf('connectNow')).toHaveLength(3)
  })

  it('显式 hostUrl：不尝试其他端口，不是 app-mcp 时直接 host-mismatch', async () => {
    const h = await connected({ hostUrl: 'ws://127.0.0.1:9999/app' })
    h.core.setState({ status: 'host-mismatch', reason: 'x' })
    h.app.tool('a', { description: '', handler: () => {} })
    expect(h.app.state).toEqual({ status: 'host-mismatch', reason: 'x' })
    expect(h.sockets.map((s) => s.url)).toEqual(['ws://127.0.0.1:9999/app'])
    h.socket().fail()
    h.core.emit({ type: 'connect' })
    h.app.tool('b', { description: '', handler: () => {} })
    expect(h.socket().url).toBe('ws://127.0.0.1:9999/app')
  })

  it('WebSocket 构造失败视为断开', async () => {
    const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
    const h = setup({ logger })
    h.core.calls = []
    const app = new AppMcpDriver(
      { appId: 'shop', appName: 'x', logger },
      {
        loadCore: async () => () => h.core,
        createWebSocket: () => {
          throw new Error('bad url')
        },
      },
    )
    await settle()
    expect(h.core.methods()).toContain('handleDisconnected')
    app.dispose()
  })

  it('Paired 保存 token，下次创建时读入', async () => {
    const h = await connected()
    h.socket().script({ type: 'paired', token: 'new-token' })
    expect(localStorage.getItem(tokenKey('shop'))).toBe('new-token')
    const h2 = setup()
    await settle()
    expect(h2.core.config?.token).toBe('new-token')
  })

  it('Warning 写日志', async () => {
    const h = await connected()
    h.socket().script({ type: 'warning', message: '坏消息' })
    expect(h.logger.warn).toHaveBeenCalledWith('[app-mcp] 坏消息')
  })

  it('backoff 的 retryAt 转换为 Date.now() 刻度', async () => {
    const h = await connected()
    h.socket().script({ type: 'stateChanged', state: { status: 'backoff', retryAt: 1500 } })
    expect(h.app.state).toEqual({ status: 'backoff', retryAt: 1_700_000_000_000 + 1500 })
  })

  it('state 在变化前返回同一对象引用', async () => {
    const h = await connected()
    const a = h.app.state
    expect(h.app.state).toBe(a)
    h.socket().script({ type: 'stateChanged', state: { status: 'connected' } })
    expect(h.app.state).toBe(a)
    h.socket().script({ type: 'stateChanged', state: { status: 'backoff', retryAt: 2000 } })
    const b = h.app.state
    expect(b).not.toBe(a)
    expect(h.app.state).toBe(b)
  })

  it('onStateChange 可取消订阅，重复状态不通知', async () => {
    const h = await connected()
    const fn = vi.fn()
    const off = h.app.onStateChange(fn)
    h.socket().script({ type: 'stateChanged', state: { status: 'connected' } })
    expect(fn).not.toHaveBeenCalled()
    h.socket().script({ type: 'stateChanged', state: { status: 'pending-pairing' } })
    expect(fn).toHaveBeenCalledWith({ status: 'pending-pairing' })
    off()
    h.socket().script({ type: 'stateChanged', state: { status: 'rejected', reason: 'no' } })
    expect(fn).toHaveBeenCalledTimes(1)
  })

  it('dispose：停止核心、关闭连接、释放核心', async () => {
    const h = await connected()
    const s = h.socket()
    h.app.dispose()
    expect(h.core.methods()).toEqual(expect.arrayContaining(['stop', 'free']))
    expect(s.closed).toBe(true)
    expect(h.app.state).toEqual({ status: 'stopped' })
    h.core.calls = []
    document.dispatchEvent(new Event('visibilitychange'))
    window.dispatchEvent(new Event('blur'))
    expect(h.core.calls).toHaveLength(0)
  })

  it('加载完成前 dispose 不再创建核心', async () => {
    const h = setup({}, true)
    h.app.dispose()
    await h.load()
    expect(h.core.calls).toHaveLength(0)
    expect(h.app.state).toEqual({ status: 'stopped' })
  })
})

describe('定时器', () => {
  it('按 pollTimeout 重设定时器，到期调用 handleTimeout', async () => {
    vi.useFakeTimers()
    const h = setup()
    h.core.timeout = 1500
    await vi.advanceTimersByTimeAsync(0)
    h.clock.now = 1000
    expect(h.core.methods()).not.toContain('handleTimeout')
    await vi.advanceTimersByTimeAsync(499)
    expect(h.core.methods()).not.toContain('handleTimeout')
    h.clock.now = 1500
    h.core.timeout = 3000
    await vi.advanceTimersByTimeAsync(1)
    expect(h.core.callsOf('handleTimeout')).toEqual([[1500]])
    // 新的截止时间被重新安排
    h.clock.now = 2000
    h.core.timeout = 2100
    h.socket().open() // 任意输入都会重设
    await vi.advanceTimersByTimeAsync(100)
    expect(h.core.callsOf('handleTimeout')).toHaveLength(2)
    // 没有定时器时不再触发
    h.core.timeout = undefined
    h.socket().receive('x')
    await vi.advanceTimersByTimeAsync(10_000)
    expect(h.core.callsOf('handleTimeout')).toHaveLength(2)
  })
})

describe('工具调用', () => {
  async function invoke(
    handler: (input: any, ctx: any) => unknown,
    args: unknown = {},
    input?: unknown,
  ): Promise<{ h: Awaited<ReturnType<typeof connected>>; outcome: () => unknown }> {
    const h = await connected()
    h.app.tool('t', { description: '', handler, ...(input !== undefined && { input: input as never }) })
    const toolId = h.core.callsOf('registerTool').length
    h.socket().script({ type: 'invokeTool', callId: 'c1', tool: toolId, name: 't', arguments: args })
    await settle()
    return { h, outcome: () => h.core.callsOf('completeCall')[0]?.[1] }
  }

  it('成功：返回值作为 data', async () => {
    const handler = vi.fn((input: unknown, _ctx: ToolContext) => ({ echo: input }))
    const { outcome } = await invoke(handler, { a: 1 })
    expect(outcome()).toEqual({ data: { echo: { a: 1 } } })
    expect(handler.mock.calls[0]?.[1]).toMatchObject({ callId: 'c1' })
    expect(handler.mock.calls[0]?.[1].signal).toBeInstanceOf(AbortSignal)
  })

  it('拆开 { data, stateHints }', async () => {
    const { outcome } = await invoke(async () => ({ data: { ok: true }, stateHints: ['cart.state'] }))
    expect(outcome()).toEqual({ data: { ok: true }, stateHints: ['cart.state'] })
  })

  it('含其他字段的对象整体作为 data', async () => {
    const { outcome } = await invoke(() => ({ data: 1, other: 2 }))
    expect(outcome()).toEqual({ data: { data: 1, other: 2 } })
  })

  it('undefined → null；Date 按 JSON 序列化', async () => {
    expect((await invoke(() => undefined)).outcome()).toEqual({ data: null })
    const d = new Date(0)
    expect((await invoke(() => ({ at: d }))).outcome()).toEqual({ data: { at: d.toJSON() } })
  })

  it('ToolCallError 使用其 kind', async () => {
    const { outcome } = await invoke(() => {
      throw new ToolCallError('USER_REJECTED', '用户取消', { step: 2 })
    })
    expect(outcome()).toEqual({ error: { kind: 'USER_REJECTED', message: '用户取消', details: { step: 2 } } })
  })

  it('其他异常 → HANDLER_ERROR', async () => {
    const { outcome } = await invoke(async () => {
      throw new Error('炸了')
    })
    expect(outcome()).toEqual({ error: { kind: 'HANDLER_ERROR', message: '炸了' } })
  })

  it('无法序列化的返回值 → HANDLER_ERROR', async () => {
    const cyclic: Record<string, unknown> = {}
    cyclic.self = cyclic
    const { outcome } = await invoke(() => cyclic)
    expect(outcome()).toMatchObject({ error: { kind: 'HANDLER_ERROR' } })
  })

  it('zod 校验通过时 handler 收到 parse 后的值', async () => {
    const handler = vi.fn((input: unknown) => input)
    const { outcome } = await invoke(handler, { qty: '3' }, z.object({ qty: z.coerce.number() }))
    expect(handler.mock.calls[0]?.[0]).toEqual({ qty: 3 })
    expect(outcome()).toEqual({ data: { qty: 3 } })
  })

  it('zod 校验失败 → INVALID_INPUT，不调用 handler', async () => {
    const handler = vi.fn()
    const { outcome } = await invoke(handler, { id: 1 }, z.object({ id: z.string() }))
    expect(handler).not.toHaveBeenCalled()
    expect(outcome()).toMatchObject({
      error: { kind: 'INVALID_INPUT', details: { issues: [{ path: 'id' }] } },
    })
    expect((outcome() as { error: { message: string } }).error.message).toMatch(/^参数校验失败：id: /)
  })

  it('CancelTool → abort signal，结果被丢弃', async () => {
    const gate = deferred<string>()
    let signal: AbortSignal | undefined
    const { h } = await invoke((_input, ctx) => {
      signal = ctx.signal
      return gate.promise
    })
    expect(signal?.aborted).toBe(false)
    h.socket().script({ type: 'cancelTool', callId: 'c1', reason: 'timeout' })
    expect(signal?.aborted).toBe(true)
    expect(signal?.reason).toBeInstanceOf(ToolCallError)
    expect((signal?.reason as ToolCallError).kind).toBe('TIMEOUT')
    gate.resolve('late')
    await settle()
    expect(h.core.callsOf('completeCall')).toHaveLength(0)
  })

  it('setHandler 只替换 JS 函数', async () => {
    const h = await connected()
    const t = h.app.tool('t', { description: '', handler: () => 'old' })
    h.core.calls = []
    t.setHandler(() => 'new')
    expect(h.core.calls).toHaveLength(0)
    h.socket().script({ type: 'invokeTool', callId: 'c1', tool: 1, name: 't', arguments: {} })
    await settle()
    expect(h.core.callsOf('completeCall')[0]?.[1]).toEqual({ data: 'new' })
  })

  it('未知工具 → TOOL_NOT_FOUND', async () => {
    const h = await connected()
    h.socket().script({ type: 'invokeTool', callId: 'c9', tool: 42, name: 'ghost', arguments: {} })
    expect(h.core.callsOf('completeCall')[0]?.[1]).toMatchObject({ error: { kind: 'TOOL_NOT_FOUND' } })
  })

  it('completeCall 抛错（调用已结束）只记 debug', async () => {
    const h = await connected()
    h.core.completeCall = () => {
      throw new Error('unknown or finished call')
    }
    h.app.tool('t', { description: '', handler: () => 1 })
    h.socket().script({ type: 'invokeTool', callId: 'c1', tool: 1, name: 't', arguments: {} })
    await settle()
    expect(h.logger.error).not.toHaveBeenCalled()
    expect(h.logger.debug).toHaveBeenCalled()
  })
})

describe('资源', () => {
  it('ReadResource → read → completeRead', async () => {
    const h = await connected()
    const r = h.app.resource('cart.state', { description: '', read: async () => ({ items: [] }) })
    h.socket().script({ type: 'readResource', read: 5, resource: 1, name: 'cart.state' })
    await settle()
    expect(h.core.callsOf('completeRead')[0]).toEqual([5, { data: { items: [] } }])
    r.setReader(() => {
      throw new ToolCallError('RESOURCE_NOT_FOUND', '没了')
    })
    h.socket().script({ type: 'readResource', read: 6, resource: 1, name: 'cart.state' })
    await settle()
    expect(h.core.callsOf('completeRead')[1]).toEqual([6, { error: { kind: 'RESOURCE_NOT_FOUND', message: '没了' } }])
  })

  it('未知资源 → RESOURCE_NOT_FOUND', async () => {
    const h = await connected()
    h.socket().script({ type: 'readResource', read: 1, resource: 99, name: 'x' })
    expect(h.core.callsOf('completeRead')[0]?.[1]).toMatchObject({ error: { kind: 'RESOURCE_NOT_FOUND' } })
  })

  it('notifyChanged：加载前忽略，加载后交给核心；dispose 注销', async () => {
    const h = setup({}, true)
    const r = h.app.resource('r', { description: 'd', mimeType: 'text/plain', read: () => 'x' })
    r.notifyChanged()
    await h.load()
    expect(h.core.callsOf('registerResource')[0]?.[0]).toMatchObject({ mimeType: 'text/plain' })
    expect(h.core.methods()).not.toContain('notifyResourceChanged')
    r.notifyChanged()
    expect(h.core.callsOf('notifyResourceChanged')).toEqual([[1, 1000]])
    r.dispose()
    r.notifyChanged()
    expect(h.core.callsOf('unregisterResource')).toEqual([[1]])
    expect(h.core.callsOf('notifyResourceChanged')).toHaveLength(1)
  })
})

describe('可见性', () => {
  function setVisibilityState(v: 'visible' | 'hidden'): void {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => v })
  }

  afterEach(() => {
    setVisibilityState('visible')
  })

  it('映射 visibilitychange / freeze / resume / focus / blur', async () => {
    const hasFocus = vi.spyOn(document, 'hasFocus').mockReturnValue(true)
    const h = await connected()
    expect(h.core.callsOf('setVisibility')[0]).toEqual(['visible', true, 1000])
    h.core.calls = []

    setVisibilityState('hidden')
    document.dispatchEvent(new Event('visibilitychange'))
    expect(h.core.callsOf('setVisibility').at(-1)).toEqual(['hidden', false, 1000])

    document.dispatchEvent(new Event('freeze'))
    expect(h.core.callsOf('setVisibility').at(-1)).toEqual(['frozen', false, 1000])

    document.dispatchEvent(new Event('resume'))
    expect(h.core.callsOf('setVisibility').at(-1)).toEqual(['hidden', false, 1000])

    setVisibilityState('visible')
    document.dispatchEvent(new Event('visibilitychange'))
    expect(h.core.callsOf('setVisibility').at(-1)).toEqual(['visible', true, 1000])

    window.dispatchEvent(new Event('blur'))
    expect(h.core.callsOf('setVisibility').at(-1)).toEqual(['visible', false, 1000])
    window.dispatchEvent(new Event('focus'))
    expect(h.core.callsOf('setVisibility').at(-1)).toEqual(['visible', true, 1000])

    const count = h.core.callsOf('setVisibility').length
    window.dispatchEvent(new Event('focus'))
    expect(h.core.callsOf('setVisibility')).toHaveLength(count)
    hasFocus.mockRestore()
    h.app.dispose()
  })

  it('加载前的可见性变化在加载时生效', async () => {
    const hasFocus = vi.spyOn(document, 'hasFocus').mockReturnValue(false)
    const h = setup({}, true)
    setVisibilityState('hidden')
    document.dispatchEvent(new Event('visibilitychange'))
    await h.load()
    expect(h.core.calls[0]).toEqual(['setVisibility', 'hidden', false, 1000])
    hasFocus.mockRestore()
    h.app.dispose()
  })
})

describe('惰性 handler', () => {
  function call(h: Awaited<ReturnType<typeof connected>>, callId: string, toolId: number, args: unknown = {}): void {
    h.socket().script({ type: 'invokeTool', callId, tool: toolId, name: 'lazy', arguments: args })
  }
  const outcomeOf = (h: Awaited<ReturnType<typeof connected>>, callId: string) =>
    h.core.callsOf('completeCall').find((c) => c[0] === callId)?.[1]

  it('注册时只提交元数据，首次调用时加载一次并缓存；支持 { default } 与直接返回函数', async () => {
    const h = await connected()
    const handler = vi.fn((input: { n: number }) => input.n * 2)
    const load = vi.fn(async () => ({ default: handler }))
    h.app.tool('lazy', { description: '惰性', risk: 'read', load })
    expect(h.core.callsOf('registerTool')[0]?.[0]).toEqual({
      name: 'lazy',
      description: '惰性',
      risk: 'read',
      inputSchema: { type: 'object', properties: {} },
    })
    expect(load).not.toHaveBeenCalled()
    call(h, 'c1', 1, { n: 2 })
    call(h, 'c2', 1, { n: 3 })
    await settle()
    await settle()
    expect(load).toHaveBeenCalledTimes(1)
    expect(outcomeOf(h, 'c1')).toEqual({ data: 4 })
    expect(outcomeOf(h, 'c2')).toEqual({ data: 6 })
    call(h, 'c3', 1, { n: 5 })
    await settle()
    expect(outcomeOf(h, 'c3')).toEqual({ data: 10 })
    expect(load).toHaveBeenCalledTimes(1)

    h.app.tool('direct', { description: 'd', load: () => Promise.resolve(() => 'ok') })
    h.socket().script({ type: 'invokeTool', callId: 'd1', tool: 2, name: 'direct', arguments: {} })
    await settle()
    await settle()
    expect(outcomeOf(h, 'd1')).toEqual({ data: 'ok' })
  })

  it('加载失败返回 HANDLER_ERROR，下次调用重试', async () => {
    const h = await connected()
    let attempt = 0
    const load = vi.fn(async () => {
      if (++attempt === 1) throw new Error('chunk 404')
      return { default: () => 'loaded' }
    })
    h.app.tool('lazy', { description: '惰性', load })
    call(h, 'c1', 1)
    await settle()
    await settle()
    expect(outcomeOf(h, 'c1')).toEqual({
      error: { kind: 'HANDLER_ERROR', message: '加载工具 lazy 的 handler 失败：chunk 404' },
    })
    call(h, 'c2', 1)
    await settle()
    await settle()
    expect(outcomeOf(h, 'c2')).toEqual({ data: 'loaded' })
    expect(load).toHaveBeenCalledTimes(2)

    // 模块没有可用的导出
    h.app.tool('bad', { description: 'b', load: async () => ({ default: 42 }) as never })
    h.socket().script({ type: 'invokeTool', callId: 'b1', tool: 2, name: 'bad', arguments: {} })
    await settle()
    await settle()
    expect(outcomeOf(h, 'b1')).toMatchObject({ error: { kind: 'HANDLER_ERROR' } })
  })

  it('zod 校验先于加载；setHandler 之后不再加载', async () => {
    const h = await connected()
    const load = vi.fn(async () => ({ default: (i: { q: string }) => `lazy:${i.q}` }))
    const handle = h.app.tool('lazy', { description: '惰性', input: z.object({ q: z.string() }), load })
    await settle()
    call(h, 'c1', 1, { q: 1 })
    await settle()
    expect(outcomeOf(h, 'c1')).toMatchObject({ error: { kind: 'INVALID_INPUT' } })
    expect(load).not.toHaveBeenCalled()
    handle.setHandler((i: { q: string }) => `set:${i.q}`)
    call(h, 'c2', 1, { q: 'x' })
    await settle()
    expect(outcomeOf(h, 'c2')).toEqual({ data: 'set:x' })
    expect(load).not.toHaveBeenCalled()
  })

  it('加载期间 setHandler 优先', async () => {
    const h = await connected()
    const gate = deferred<{ default: () => string }>()
    const handle = h.app.tool('lazy', { description: '惰性', load: () => gate.promise })
    call(h, 'c1', 1)
    await settle()
    handle.setHandler(() => 'replaced')
    gate.resolve({ default: () => 'loaded' })
    await settle()
    await settle()
    expect(outcomeOf(h, 'c1')).toEqual({ data: 'replaced' })
    call(h, 'c2', 1)
    await settle()
    expect(outcomeOf(h, 'c2')).toEqual({ data: 'replaced' })
  })

  it('handler 与 load 必须二选一', async () => {
    const h = await connected()
    expect(() => h.app.tool('both', { description: 'x', handler: () => 1, load: async () => () => 1 } as never)).toThrow(
      /二选一/,
    )
    expect(() => h.app.tool('none', { description: 'x' } as never)).toThrow(/缺少 handler/)
    expect(h.core.callsOf('registerTool')).toHaveLength(0)
  })
})

describe('instanceId', () => {
  it('保存在 sessionStorage，同一 appId 复用', async () => {
    const a = setup()
    expect(sessionStorage.getItem(instanceIdKey('shop'))).toBe(a.app.instanceId)
    const b = setup()
    expect(b.app.instanceId).toBe(a.app.instanceId)
    const c = setup({ appId: 'other' })
    expect(c.app.instanceId).not.toBe(a.app.instanceId)
    for (const h of [a, b, c]) h.app.dispose()
  })

  it('sessionStorage 不可用时生成内存 ID', () => {
    const broken = {
      getItem: () => {
        throw new Error('SecurityError')
      },
      setItem: () => {
        throw new Error('SecurityError')
      },
    }
    vi.stubGlobal('sessionStorage', broken)
    vi.stubGlobal('localStorage', broken)
    const a = setup()
    const b = setup()
    expect(a.app.instanceId).toMatch(/.{16,}/)
    expect(b.app.instanceId).not.toBe(a.app.instanceId)
    a.app.dispose()
    b.app.dispose()
    vi.unstubAllGlobals()
  })
})
