import { EventEmitter } from 'node:events'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, type AppMcp as NodeAppMcp } from '@app-mcp/node'
import { ToolCallError } from '@app-mcp/web'
import { FakeNativeClient, fakeBinding } from '../../node/src/testing/fake-native.js'
import { attachAppMcp, ToolCallError as MainToolCallError, type IpcMainInvokeEventLike, type IpcMainLike, type WebContentsLike } from './main.js'
import { exposeAppMcpBridge, type IpcRendererLike } from './preload.js'
import { CHANNEL_OP } from './protocol.js'
import {
  attachBridgeNavigation,
  createRendererAppMcp,
  getAppMcpBridge,
  ToolCallError as RendererToolCallError,
  type AppMcpBridge,
} from './renderer.js'

// ---------------------------------------------------------------------------
// 假 Electron：ipcMain / webContents / ipcRenderer（消息经 structuredClone，模拟进程边界）
// ---------------------------------------------------------------------------

class FakeIpcMain implements IpcMainLike {
  readonly handlers = new Map<string, (event: IpcMainInvokeEventLike, ...args: any[]) => unknown>()
  handle(channel: string, listener: (event: IpcMainInvokeEventLike, ...args: any[]) => unknown): void {
    if (this.handlers.has(channel)) throw new Error(`重复的 handler ${channel}`)
    this.handlers.set(channel, listener)
  }
  removeHandler(channel: string): void {
    this.handlers.delete(channel)
  }
  async invoke(sender: WebContentsLike, channel: string, ...args: unknown[]): Promise<unknown> {
    const handler = this.handlers.get(channel)
    if (!handler) throw new Error(`No handler registered for '${channel}'`)
    return structuredClone(await handler({ sender }, ...structuredClone(args)))
  }
}

class FakeWebContents extends EventEmitter implements WebContentsLike {
  private destroyed = false
  readonly toRenderer = new EventEmitter()
  constructor(readonly id: number) {
    super()
  }
  send(channel: string, ...args: unknown[]): void {
    if (this.destroyed) throw new Error('Object has been destroyed')
    this.toRenderer.emit(channel, {}, ...structuredClone(args))
  }
  isDestroyed(): boolean {
    return this.destroyed
  }
  destroy(): void {
    this.destroyed = true
    this.emit('destroyed')
  }
}

function fakeIpcRenderer(ipcMain: FakeIpcMain, wc: FakeWebContents): IpcRendererLike {
  return {
    invoke: (channel, ...args) => ipcMain.invoke(wc, channel, ...args),
    on: (channel, listener) => wc.toRenderer.on(channel, listener),
    removeListener: (channel, listener) => wc.toRenderer.removeListener(channel, listener),
  }
}

const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0))
}

const cleanups: (() => void)[] = []
afterEach(() => {
  for (const fn of cleanups.splice(0).reverse()) fn()
})

function setupMain(options: { webContents?: Parameters<typeof attachAppMcp>[0]['webContents'] } = {}) {
  const ipcMain = new FakeIpcMain()
  const appMcp: NodeAppMcp = createAppMcp({
    appId: 'shop',
    appName: 'Shop',
    clientKind: 'hybrid',
    binding: fakeBinding,
    keepAlive: false,
  })
  const native = FakeNativeClient.last as FakeNativeClient
  const logger = { warn: vi.fn(), error: vi.fn() }
  const attachment = attachAppMcp({ appMcp, ipcMain, logger, ...options })
  cleanups.push(() => {
    attachment.dispose()
    appMcp.dispose()
  })
  return { ipcMain, appMcp, native, attachment, logger }
}

function setupPage(ipcMain: FakeIpcMain, wc: FakeWebContents) {
  const target: Record<string, unknown> = {}
  exposeAppMcpBridge(null, fakeIpcRenderer(ipcMain, wc), { target, resetOnPageHide: false })
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const page = createRendererAppMcp({
    appId: 'shop',
    appName: 'Shop',
    bridge: target.appMcpBridge as never,
    logger,
  })
  cleanups.push(() => page.dispose())
  return { page, logger }
}

describe('Electron 桥接', () => {
  it('渲染进程登记的工具在主进程注册，调用往返', async () => {
    const { ipcMain, native } = setupMain()
    const wc = new FakeWebContents(1)
    const { page } = setupPage(ipcMain, wc)
    const handler = vi.fn(async (input: { n: number }, ctx: { callId: string }) => ({
      data: { doubled: input.n * 2, callId: ctx.callId },
      stateHints: ['counter'],
    }))
    page.tool('math.double', {
      description: '翻倍',
      risk: 'read',
      input: { type: 'object', properties: { n: { type: 'number' } }, required: ['n'] },
      handler,
    })
    page.tool('noop', { description: 'noop', handler: () => undefined })
    await flush()

    expect(native.tools.get('math.double')?.spec).toMatchObject({
      name: 'math.double',
      description: '翻倍',
      risk: 'read',
      inputSchemaJson: '{"type":"object","properties":{"n":{"type":"number"}},"required":["n"]}',
    })
    expect(await native.call('math.double', { n: 4 })).toEqual({
      ok: true,
      data: { doubled: 8, callId: 'c1' },
      stateHints: ['counter'],
    })
    expect(await native.call('noop')).toEqual({ ok: true, data: null, stateHints: [] })
    expect(page.instanceId).toBe('fake-instance')
  })

  it('Agent 的幂等键经桥接到达渲染进程 handler 上下文（spec/protocol.md 3.3），没有时缺省', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.tool('order.create', {
      description: '下单',
      handler: (_input, ctx) => ({ key: ctx.idempotencyKey ?? null, has: 'idempotencyKey' in ctx }),
    })
    await flush()
    expect(await native.call('order.create', {}, { idempotencyKey: 'order-7' })).toEqual({
      ok: true,
      data: { key: 'order-7', has: true },
      stateHints: [],
    })
    expect(await native.call('order.create', {})).toEqual({ ok: true, data: { key: null, has: false }, stateHints: [] })
  })

  it('注解、outputSchema 与结构化结果经桥接往返；update 可清除声明', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    const t = page.tool('order.submit', {
      description: '下单',
      annotations: { idempotentHint: false, openWorldHint: true },
      outputSchema: { type: 'object', properties: { orderId: { type: 'string' } } },
      handler: () => ({
        data: { orderId: 'o1' },
        status: 'pending' as const,
        stateResource: 'order.state',
        summary: '等待付款',
        annotations: { audience: ['user' as const], priority: 0.5 },
      }),
    })
    page.tool<unknown, unknown>('plain', { description: 'p', handler: () => ({ data: 1, status: 'success' }) })
    await flush()
    expect(native.tools.get('order.submit')?.spec).toMatchObject({
      annotations: { idempotentHint: false, openWorldHint: true },
      outputSchemaJson: '{"type":"object","properties":{"orderId":{"type":"string"}}}',
    })
    expect(await native.call('order.submit')).toEqual({
      ok: true,
      data: { orderId: 'o1' },
      stateHints: [],
      status: 'pending',
      stateResource: 'order.state',
      summary: '等待付款',
      annotations: { audience: ['user'], priority: 0.5 },
    })
    expect(await native.call('plain')).toEqual({ ok: true, data: { data: 1, status: 'success' }, stateHints: [] })
    t.update({ annotations: undefined, outputSchema: undefined })
    await flush()
    const spec = native.tools.get('order.submit')?.spec
    expect(spec?.annotations).toBeUndefined()
    expect(spec?.outputSchemaJson).toBeUndefined()
  })

  it('zod 输入在渲染进程 parse', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    const zodLike = {
      _zod: {},
      toJSONSchema: () => ({ type: 'object', properties: { q: { type: 'string' } } }),
      parse(v: unknown) {
        const q = (v as { q?: unknown }).q
        if (typeof q !== 'string') throw Object.assign(new Error('bad'), { issues: [{ path: ['q'], message: '需要字符串' }] })
        return { q: q.toUpperCase() }
      },
    }
    page.tool('search', { description: 's', input: zodLike, handler: (i: { q: string }) => i.q })
    await flush()
    expect(native.tools.get('search')?.spec.inputSchemaJson).toBe('{"type":"object","properties":{"q":{"type":"string"}}}')
    expect(await native.call('search', { q: 'hi' })).toMatchObject({ ok: true, data: 'HI' })
    expect(await native.call('search', { q: 1 })).toEqual({
      ok: false,
      kind: 'INVALID_INPUT',
      message: '参数校验失败：q: 需要字符串',
    })
  })

  it('错误类别透传；重名注册在页面中报错', async () => {
    const { ipcMain, native } = setupMain()
    const { page, logger } = setupPage(ipcMain, new FakeWebContents(1))
    page.tool('reject', {
      description: 'r',
      handler: () => {
        throw new ToolCallError('USER_REJECTED', '不行')
      },
    })
    page.tool('boom', {
      description: 'b',
      handler: () => {
        throw new Error('炸了')
      },
    })
    page.tool('boom', { description: 'dup', handler: () => 1 })
    await flush()
    expect(await native.call('reject')).toEqual({ ok: false, kind: 'USER_REJECTED', message: '不行' })
    expect(await native.call('boom')).toEqual({ ok: false, kind: 'HANDLER_ERROR', message: '炸了' })
    expect(logger.error).toHaveBeenCalledWith(expect.stringMatching(/tool\.register 失败.*already registered/))
  })

  it('USER_ACTION_REQUIRED 的 reason / uri 经 IPC 到达主进程客户端；缺省字段省略', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.tool('login', {
      description: 'l',
      handler: () => {
        throw RendererToolCallError.userActionRequired('请先登录', { reason: 'login', uri: 'shop://login' })
      },
    })
    page.tool('foreground', {
      description: 'f',
      handler: () => {
        throw RendererToolCallError.userActionRequired('请切到前台')
      },
    })
    page.tool('deny', {
      description: 'd',
      handler: () => {
        throw new ToolCallError('POLICY_DENIED', '策略拒绝')
      },
    })
    await flush()
    expect(await native.call('login')).toEqual({
      ok: false,
      kind: 'USER_ACTION_REQUIRED',
      message: '请先登录',
      details: { reason: 'login', uri: 'shop://login' },
    })
    expect(await native.call('foreground')).toEqual({ ok: false, kind: 'USER_ACTION_REQUIRED', message: '请切到前台' })
    expect(await native.call('deny')).toEqual({ ok: false, kind: 'POLICY_DENIED', message: '策略拒绝' })
    expect(MainToolCallError.userActionRequired('x', { reason: 'confirm' })).toMatchObject({
      kind: 'USER_ACTION_REQUIRED',
      details: { reason: 'confirm' },
    })
  })

  it('取消经 IPC 传到页面的 AbortSignal', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    let signal: AbortSignal | undefined
    page.tool('slow', {
      description: 'slow',
      handler: (_i, ctx) => {
        signal = ctx.signal
        return new Promise(() => {})
      },
    })
    await flush()
    const { callId, result } = native.invoke('slow')
    await flush()
    expect(signal?.aborted).toBe(false)
    native.cancel(callId, 'requested')
    expect(await result).toMatchObject({ ok: false, kind: 'CANCELLED' })
    await flush()
    expect(signal?.aborted).toBe(true)
    expect((signal?.reason as { kind?: string }).kind).toBe('CANCELLED')
  })

  it('页面 handler 的 ctx.progress 经 IPC 转给主进程的 @app-mcp/node 调用', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    let release!: () => void
    page.tool('export', {
      description: '导出',
      handler: async (_i, ctx) => {
        ctx.progress?.(1, 3, '第 1 页')
        ctx.progress?.(2)
        await new Promise<void>((r) => (release = r))
        return 'ok'
      },
    })
    await flush()
    const { callId, result } = native.invoke('export')
    await flush()
    expect(native.progressReports).toEqual([
      { callId, progress: 1, total: 3, message: '第 1 页' },
      { callId, progress: 2, total: null, message: null },
    ])
    release()
    expect(await result).toMatchObject({ ok: true, data: 'ok' })
  })

  it('资源读取、变更通知与 scope', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    const r = page.resource('cart', { description: '购物车', read: () => ({ items: [1, 2] }) })
    const scope = page.scope('dialog')
    scope.tool('dialog.close', { description: '关闭', handler: () => 'closed' })
    scope.scope('inner').resource('inner.res', { description: 'x', read: () => 1 })
    await flush()
    expect(await native.read('cart')).toMatchObject({ ok: true, data: { items: [1, 2] } })
    r.notifyChanged()
    await flush()
    expect(native.resourceChanges('cart')).toBe(1)
    expect(await native.call('dialog.close')).toMatchObject({ ok: true, data: 'closed' })
    expect(native.resources.has('inner.res')).toBe(true)
    scope.dispose()
    await flush()
    expect(native.tools.has('dialog.close')).toBe(false)
    expect(native.resources.has('inner.res')).toBe(false)
    expect(native.resources.has('cart')).toBe(true)
  })

  it('资源 realtime 经桥转给 @app-mcp/node；未声明时不传', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.resource('order.status', { description: '订单状态', realtime: true, read: () => 'paid' })
    page.resource('cart', { description: '购物车', read: () => [] })
    await flush()
    expect(native.resources.get('order.status')?.spec).toMatchObject({ name: 'order.status', realtime: true })
    expect(native.resources.get('cart')?.spec).not.toHaveProperty('realtime')
    expect(native.resources.get('cart')?.spec).not.toHaveProperty('annotations')
  })

  it('资源的内容标注经桥转给 @app-mcp/node；页面读取失败的类别与详情（reason / uri）到达主进程客户端', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.resource('profile', {
      description: '个人资料',
      annotations: { audience: ['user'], priority: 0.5 },
      read: () => {
        throw RendererToolCallError.userActionRequired('登录已过期', { reason: 'login', uri: 'shop://login' })
      },
    })
    page.resource('stock', {
      description: '库存',
      read: () => {
        throw new ToolCallError('RESOURCE_NOT_FOUND', '仓库离线')
      },
    })
    await flush()
    expect(native.resources.get('profile')?.spec).toMatchObject({ annotations: { audience: ['user'], priority: 0.5 } })
    expect(await native.read('profile')).toEqual({
      ok: false,
      kind: 'USER_ACTION_REQUIRED',
      message: '登录已过期',
      details: { reason: 'login', uri: 'shop://login' },
    })
    expect(await native.read('stock')).toEqual({ ok: false, kind: 'RESOURCE_NOT_FOUND', message: '仓库离线' })
  })

  it('update / dispose 同步到主进程', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    const t = page.tool('x', { description: 'x', handler: () => 'old' })
    t.update({ description: 'y', enabled: false })
    t.setHandler(() => 'new')
    await flush()
    expect(native.tools.get('x')?.spec).toMatchObject({ description: 'y', enabled: false })
    expect(await native.call('x')).toMatchObject({ data: 'new' })
    t.dispose()
    await flush()
    expect(native.tools.has('x')).toBe(false)
  })

  it('webContents 销毁时注销其工具，进行中的调用以 APP_DISCONNECTED 失败', async () => {
    const { ipcMain, native, attachment } = setupMain()
    const wc1 = new FakeWebContents(1)
    const wc2 = new FakeWebContents(2)
    setupPage(ipcMain, wc1).page.tool('a', { description: 'a', handler: () => new Promise(() => {}) })
    setupPage(ipcMain, wc2).page.tool('b', { description: 'b', handler: () => 'b' })
    await flush()
    expect(attachment.sessionCount).toBe(2)
    const pending = native.call('a')
    await flush()
    wc1.destroy()
    expect(await pending).toMatchObject({ ok: false, kind: 'APP_DISCONNECTED' })
    expect(native.tools.has('a')).toBe(false)
    expect(native.tools.has('b')).toBe(true)
    expect(attachment.sessionCount).toBe(1)
  })

  it('页面刷新（新的 hello）丢弃旧登记，dispose 发送 reset', async () => {
    const { ipcMain, native, attachment } = setupMain()
    const wc = new FakeWebContents(1)
    const first = setupPage(ipcMain, wc).page
    first.tool('old', { description: 'old', handler: () => 1 })
    await flush()
    expect(native.tools.has('old')).toBe(true)
    // 刷新：旧页面没有机会 dispose，新页面直接 hello
    setupPage(ipcMain, wc).page.tool('new', { description: 'new', handler: () => 2 })
    await flush()
    expect(native.tools.has('old')).toBe(false)
    expect(native.tools.has('new')).toBe(true)
    expect(attachment.sessionCount).toBe(1)
  })

  it('状态变化转发给页面', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.tool('t', { description: 't', handler: () => 1 }) // 建立会话
    await flush()
    expect(page.state).toEqual({ status: 'idle' })
    const seen: string[] = []
    page.onStateChange((s) => seen.push(s.status))
    native.emit({ type: 'state', state: { status: 'connected' } })
    await flush()
    expect(seen).toEqual(['connected'])
    expect(page.state).toEqual({ status: 'connected' })
  })

  it('连接 ID 随 hello 与状态事件转发给页面', async () => {
    const { ipcMain, native } = setupMain()
    // FakeNativeClient 未声明 connectionId（NativeClient 中为可选），在这里模拟原生客户端已握手。
    Object.assign(native, { connectionId: '3f9a1c-7' })
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.tool('t', { description: 't', handler: () => 1 }) // 建立会话（状态事件只发给有登记的页面）
    await flush()
    expect(page.connectionId).toBe('3f9a1c-7')

    const seen: (string | undefined)[] = []
    page.onStateChange(() => seen.push(page.connectionId))
    Object.assign(native, { connectionId: null })
    native.emit({ type: 'state', state: { status: 'backoff', retryInMs: 1000 } })
    await flush()
    Object.assign(native, { connectionId: '3f9a1c-8' })
    native.emit({ type: 'state', state: { status: 'connected' } })
    await flush()
    expect(seen).toEqual([undefined, '3f9a1c-8'])
    expect(page.connectionId).toBe('3f9a1c-8')
  })

  it('旧主进程（消息不带 connectionId）时页面的连接 ID 为 undefined', async () => {
    const listeners: ((event: unknown) => void)[] = []
    const bridge = {
      version: 1,
      request: async (op: { op: string }) =>
        op.op === 'hello'
          ? { ok: true as const, value: { instanceId: 'old', state: { status: 'connected' } } }
          : { ok: true as const },
      onMessage: (listener: (event: unknown) => void) => {
        listeners.push(listener)
        return () => {}
      },
    }
    const page = createRendererAppMcp({ appId: 'shop', appName: 'Shop', bridge: bridge as never })
    cleanups.push(() => page.dispose())
    await flush()
    expect(page.state).toEqual({ status: 'connected' })
    expect(page.connectionId).toBeUndefined()
    for (const l of listeners) l({ type: 'state', state: { status: 'connected' } })
    expect(page.connectionId).toBeUndefined()
  })

  it('webContents 过滤：不允许的页面收到 FORBIDDEN', async () => {
    const allowed = new FakeWebContents(1)
    const { ipcMain, native } = setupMain({ webContents: allowed })
    const { page, logger } = setupPage(ipcMain, new FakeWebContents(2))
    page.tool('t', { description: 't', handler: () => 1 })
    await flush()
    expect(native.tools.size).toBe(0)
    expect(logger.error).toHaveBeenCalledWith(expect.stringContaining('不允许'))
  })

  it('attachment.dispose 移除 IPC handler 并注销全部登记', async () => {
    const { ipcMain, native, attachment } = setupMain()
    setupPage(ipcMain, new FakeWebContents(1)).page.tool('t', { description: 't', handler: () => 1 })
    await flush()
    attachment.dispose()
    expect(ipcMain.handlers.has(CHANNEL_OP)).toBe(false)
    expect(native.tools.size).toBe(0)
  })

  it('惰性 handler（load）经主进程调用时才加载', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    const load = vi.fn(async () => ({ default: (i: { n: number }) => i.n + 1 }))
    page.tool('lazy.inc', { description: '加一', input: { type: 'object', properties: { n: { type: 'number' } } }, load })
    await flush()
    expect(native.tools.get('lazy.inc')?.spec).toMatchObject({ description: '加一' })
    expect(load).not.toHaveBeenCalled()
    expect(await native.call('lazy.inc', { n: 1 })).toMatchObject({ ok: true, data: 2 })
    expect(await native.call('lazy.inc', { n: 2 })).toMatchObject({ ok: true, data: 3 })
    expect(load).toHaveBeenCalledTimes(1)
  })

  it('enabled: false 时不需要桥接，注册为空操作', () => {
    const page = createRendererAppMcp({ appId: 'shop', appName: 'Shop', enabled: false })
    expect(page.state).toEqual({ status: 'disabled' })
    page.tool('t', { description: 't', handler: () => 1 }).dispose()
    page.scope('s').resource('r', { description: 'r', read: () => 1 }).notifyChanged()
    page.dispose()
  })

  it('没有桥接时抛出说明性错误', () => {
    expect(getAppMcpBridge()).toBeUndefined()
    expect(() => createRendererAppMcp({ appId: 'shop', appName: 'Shop' })).toThrow(/exposeAppMcpBridge/)
  })
})

describe('生命周期转发', () => {
  it('页面的 wake / sleep / connectNow 转给主进程客户端', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    await flush()
    page.wake()
    page.sleep()
    page.connectNow()
    await flush()
    expect(native.lifecycleCalls).toEqual(['wake:app', 'sleep:app', 'connectNow'])
  })

  it('hold 按页面管理：release 释放；页面刷新 / webContents 销毁时释放该页全部持有', async () => {
    const { ipcMain, native } = setupMain()
    const wc1 = new FakeWebContents(1)
    const wc2 = new FakeWebContents(2)
    const { page: p1 } = setupPage(ipcMain, wc1)
    const { page: p2 } = setupPage(ipcMain, wc2)
    const a = p1.hold()
    p1.hold()
    p2.hold()
    await flush()
    expect(native.activeHolds).toBe(3)
    a.release()
    a.release()
    await flush()
    expect(native.activeHolds).toBe(2)

    // 页面 1 刷新：新页面 hello → 旧页面的持有释放
    setupPage(ipcMain, wc1)
    await flush()
    expect(native.activeHolds).toBe(1)

    // 页面 2 的 webContents 销毁
    wc2.destroy()
    await flush()
    expect(native.activeHolds).toBe(0)
  })

  it('页面 dispose（reset）释放其持有', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.hold()
    await flush()
    expect(native.activeHolds).toBe(1)
    page.dispose()
    await flush()
    expect(native.activeHolds).toBe(0)
  })

  it('主进程客户端不提供生命周期方法时为空操作', async () => {
    const ipcMain = new FakeIpcMain()
    const scope = { dispose: vi.fn(), tool: vi.fn(), resource: vi.fn(), scope: vi.fn(), name: 's' }
    const attachment = attachAppMcp({
      appMcp: {
        scope: () => scope as never,
        instanceId: 'x',
        state: { status: 'connected' },
        onStateChange: () => () => {},
      },
      ipcMain,
    })
    cleanups.push(() => attachment.dispose())
    const wc = new FakeWebContents(1)
    const reply = async (op: unknown) => ipcMain.invoke(wc, CHANNEL_OP, op)
    expect(await reply({ op: 'lifecycle.wake' })).toEqual({ ok: true, value: false })
    expect(await reply({ op: 'lifecycle.hold', holdId: 1 })).toEqual({ ok: true })
    expect(await reply({ op: 'lifecycle.hold', holdId: 'x' })).toMatchObject({ ok: false })
    expect(await reply({ op: 'lifecycle.release', holdId: 1 })).toEqual({ ok: true })
  })
})

describe('preload', () => {
  it('通过 contextBridge 暴露；pagehide 时通知主进程 reset', async () => {
    const { ipcMain, native } = setupMain()
    const wc = new FakeWebContents(1)
    const exposed: Record<string, unknown> = {}
    const contextBridge = { exposeInMainWorld: vi.fn((key: string, api: unknown) => (exposed[key] = api)) }
    const events = new EventTarget()
    const g = globalThis as { addEventListener?: unknown }
    const original = g.addEventListener
    g.addEventListener = events.addEventListener.bind(events)
    try {
      exposeAppMcpBridge(contextBridge, fakeIpcRenderer(ipcMain, wc))
    } finally {
      g.addEventListener = original
    }
    expect(contextBridge.exposeInMainWorld).toHaveBeenCalledWith('appMcpBridge', expect.anything())
    const bridge = exposed.appMcpBridge as Parameters<typeof createRendererAppMcp>[0]['bridge']
    const page = createRendererAppMcp({ appId: 'shop', appName: 'Shop', bridge })
    cleanups.push(() => page.dispose())
    page.tool('t', { description: 't', handler: () => 1 })
    await flush()
    expect(native.tools.has('t')).toBe(true)
    events.dispatchEvent(new Event('pagehide'))
    await flush()
    expect(native.tools.has('t')).toBe(false)
  })
})

describe('导航（spec/protocol.md 3.4）', () => {
  function setupNav(navigation = true, raiseWindow?: (wc: WebContentsLike) => void) {
    const ipcMain = new FakeIpcMain()
    const appMcp: NodeAppMcp = createAppMcp({ appId: 'shop', appName: 'Shop', clientKind: 'hybrid', binding: fakeBinding, keepAlive: false })
    const native = FakeNativeClient.last as FakeNativeClient
    const logger = { warn: vi.fn(), error: vi.fn() }
    const attachment = attachAppMcp({ appMcp, ipcMain, logger, navigation, ...(raiseWindow && { raiseWindow }) })
    cleanups.push(() => {
      attachment.dispose()
      appMcp.dispose()
    })
    return { ipcMain, native, attachment, appMcp, logger }
  }

  it('navigation: true 时接入即声明；转给开启导航的页面并回传结果', async () => {
    const { ipcMain, native } = setupNav()
    expect(native.navigationHandler).toBeDefined()
    // 没有页面开启导航：失败
    expect(await native.navigate('cart')).toMatchObject({ ok: false, kind: 'fail', message: expect.stringContaining('没有页面处理导航') })

    const wc = new FakeWebContents(1)
    const { page } = setupPage(ipcMain, wc)
    page.tool('cart.view', { description: '看购物车', handler: () => 1 })
    const seen: unknown[] = []
    const nav = attachBridgeNavigation(getBridge(ipcMain, wc), async (request) => {
      seen.push(request)
      if (request.page === 'login') throw new RendererToolCallError('NAVIGATION_DENIED', '需要先登录')
      if (request.page === 'broken') throw new Error('页面加载失败')
    })
    await nav.ready
    expect(await native.navigate('cart', { sku: 'A-42' })).toEqual({ ok: true })
    expect(seen).toEqual([{ page: 'cart', params: { sku: 'A-42' } }])
    expect(await native.navigate('login')).toEqual({ ok: false, kind: 'deny', message: '需要先登录' })
    expect(await native.navigate('broken')).toEqual({ ok: false, kind: 'fail', message: '页面加载失败' })

    nav.dispose()
    await flush()
    expect(await native.navigate('cart')).toMatchObject({ ok: false, kind: 'fail' })
  })

  it('页面回调抛出 userActionRequired → USER_ACTION_REQUIRED（带 reason / uri）', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(5)
    setupPage(ipcMain, wc)
    const nav = attachBridgeNavigation(getBridge(ipcMain, wc), async ({ page }) => {
      if (page === 'bare') throw RendererToolCallError.userActionRequired('请切到前台')
      throw RendererToolCallError.userActionRequired('已发通知，请点开', { reason: 'foreground', uri: 'shop://cart' })
    })
    await nav.ready
    expect(await native.navigate('cart')).toEqual({
      ok: false, kind: 'userAction', message: '已发通知，请点开', reason: 'foreground', uri: 'shop://cart',
    })
    expect(await native.navigate('bare')).toEqual({ ok: false, kind: 'userAction', message: '请切到前台', reason: null, uri: null })
  })

  it('navigateInBackground：未给 raiseWindow 时为 false（后台导航立即 USER_ACTION_REQUIRED）', async () => {
    const { ipcMain, native, appMcp } = setupNav()
    expect(native.navigateInBackground).toBe(false)
    const wc = new FakeWebContents(6)
    setupPage(ipcMain, wc)
    const handler = vi.fn()
    const nav = attachBridgeNavigation(getBridge(ipcMain, wc), handler)
    await nav.ready
    appMcp.setVisibility('hidden', false)
    expect(await native.navigate('cart')).toMatchObject({ ok: false, kind: 'userAction', reason: 'foreground' })
    expect(handler).not.toHaveBeenCalled()
  })

  it('raiseWindow：navigateInBackground 为 true，转给页面前调用；抛错只记警告', async () => {
    const raised: number[] = []
    const raise = vi.fn((wc: WebContentsLike) => {
      raised.push(wc.id)
      if (raised.length === 2) throw new Error('窗口已销毁')
    })
    const { ipcMain, native, appMcp, logger } = setupNav(true, raise)
    expect(native.navigateInBackground).toBe(true)
    const wc = new FakeWebContents(7)
    setupPage(ipcMain, wc)
    const nav = attachBridgeNavigation(getBridge(ipcMain, wc), () => {})
    await nav.ready
    appMcp.setVisibility('hidden', false)
    expect(await native.navigate('cart')).toEqual({ ok: true })
    expect(await native.navigate('cart')).toEqual({ ok: true })
    expect(raised).toEqual([7, 7])
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('带到前台'), expect.any(Error))
  })

  it('页面关闭时进行中的导航失败', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(2)
    setupPage(ipcMain, wc)
    const nav = attachBridgeNavigation(getBridge(ipcMain, wc), () => new Promise(() => {}))
    await nav.ready
    const pending = native.navigate('cart')
    await flush()
    wc.destroy()
    expect(await pending).toMatchObject({ ok: false, kind: 'fail', message: expect.stringContaining('页面已关闭') })
  })

  it('未开启 navigation 时不声明，页面开启被拒绝', async () => {
    const { ipcMain, native } = setupNav(false)
    expect(native.navigationHandler).toBeUndefined()
    const wc = new FakeWebContents(3)
    setupPage(ipcMain, wc)
    const nav = attachBridgeNavigation(getBridge(ipcMain, wc), () => {})
    await expect(nav.ready).rejects.toThrow('navigation: true')
  })

  it('页面工具的 surface / page 转到主进程（update 缺省清除）', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(4)
    const bridge = getBridge(ipcMain, wc)
    const spec = { description: '结算', surface: 'view', page: 'cart' }
    await bridge.request({ op: 'tool.register', id: 1, name: 'cart.checkout', spec } as never)
    expect(native.tools.get('cart.checkout')?.spec).toMatchObject({ surface: 'view', page: 'cart' })
    await bridge.request({ op: 'tool.update', id: 1, spec: { description: '结算' } } as never)
    expect(native.tools.get('cart.checkout')?.spec).not.toHaveProperty('page')
  })

  it('页面工具的 backgroundTool 转到主进程（update 缺省清除）', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(8)
    const bridge = getBridge(ipcMain, wc)
    const spec = { description: '结算', surface: 'view', page: 'cart', backgroundTool: 'cart.checkoutBg' }
    await bridge.request({ op: 'tool.register', id: 1, name: 'cart.checkout', spec } as never)
    expect(native.tools.get('cart.checkout')?.spec).toMatchObject({ backgroundTool: 'cart.checkoutBg' })
    await bridge.request({ op: 'tool.update', id: 1, spec: { description: '结算' } } as never)
    expect(native.tools.get('cart.checkout')?.spec).not.toHaveProperty('backgroundTool')
  })

  it('页面工具的 implements 转到主进程（update 缺省清除）', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(10)
    const bridge = getBridge(ipcMain, wc)
    const spec = { description: '打开', implements: ['link.open@1'] }
    await bridge.request({ op: 'tool.register', id: 1, name: 'web.open', spec } as never)
    expect(native.tools.get('web.open')?.spec).toMatchObject({ implements: ['link.open@1'] })
    await bridge.request({ op: 'tool.update', id: 1, spec: { description: '打开' } } as never)
    expect(native.tools.get('web.open')?.spec).not.toHaveProperty('implements')
  })

  it('页面工具与资源的 cache 转到主进程（update 缺省清除）', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(11)
    const bridge = getBridge(ipcMain, wc)
    const spec = { description: '列表', risk: 'read', cache: { ttlMs: 5000, scope: 'shared' } }
    await bridge.request({ op: 'tool.register', id: 1, name: 'feed.list', spec } as never)
    expect(native.tools.get('feed.list')?.spec).toMatchObject({ cache: { ttlMs: 5000, scope: 'shared' } })
    await bridge.request({ op: 'tool.update', id: 1, spec: { description: '列表', risk: 'read' } } as never)
    expect(native.tools.get('feed.list')?.spec).not.toHaveProperty('cache')
    await bridge.request({ op: 'resource.register', id: 2, name: 'feed', description: '订阅', cache: { ttlMs: 30000 } } as never)
    await bridge.request({ op: 'resource.register', id: 3, name: 'cart', description: '购物车' } as never)
    expect(native.resources.get('feed')?.spec).toMatchObject({ cache: { ttlMs: 30000 } })
    expect(native.resources.get('cart')?.spec).not.toHaveProperty('cache')
  })

  it('页面工具的 concurrency / exclusive 转到主进程（update 缺省清除）', async () => {
    const { ipcMain, native } = setupNav()
    const wc = new FakeWebContents(9)
    const bridge = getBridge(ipcMain, wc)
    const spec = { description: '改文档', concurrency: 2, exclusive: 'doc' }
    await bridge.request({ op: 'tool.register', id: 1, name: 'doc.edit', spec } as never)
    expect(native.tools.get('doc.edit')?.spec).toMatchObject({ concurrency: 2, exclusive: 'doc' })
    await bridge.request({ op: 'tool.update', id: 1, spec: { description: '改文档' } } as never)
    expect(native.tools.get('doc.edit')?.spec).not.toHaveProperty('concurrency')
    expect(native.tools.get('doc.edit')?.spec).not.toHaveProperty('exclusive')
  })
})

function getBridge(ipcMain: FakeIpcMain, wc: FakeWebContents) {
  const target: Record<string, unknown> = {}
  exposeAppMcpBridge(null, fakeIpcRenderer(ipcMain, wc), { target, resetOnPageHide: false })
  return target.appMcpBridge as AppMcpBridge
}

describe('用户正在操作（busy.set，spec/protocol.md 5.3）', () => {
  it('各页声明之或：任一页面 busy 即 busy，只在汇总值变化时改变客户端', async () => {
    const { ipcMain, native } = setupMain()
    const a = setupPage(ipcMain, new FakeWebContents(1)).page
    const b = setupPage(ipcMain, new FakeWebContents(2)).page
    a.setBusy(true)
    await flush()
    expect(native.busy).toBe(true)
    b.beginBusy()
    a.setBusy(false)
    await flush()
    expect(native.busy).toBe(true)
    b.dispose()
    await flush()
    expect(native.busy).toBe(false)
    expect(native.busyCalls).toEqual([true, false])
  })

  it('页面刷新（新的 hello）、卸载（reset）或 webContents 销毁后该页声明失效', async () => {
    const { ipcMain, native } = setupMain()
    const wc = new FakeWebContents(1)
    setupPage(ipcMain, wc).page.setBusy(true)
    await flush()
    expect(native.busy).toBe(true)
    // 刷新：同一 webContents 上的新页面
    const reloaded = setupPage(ipcMain, wc).page
    await flush()
    expect(native.busy).toBe(false)
    reloaded.setBusy(true)
    await flush()
    expect(native.busy).toBe(true)
    reloaded.dispose()
    await flush()
    expect(native.busy).toBe(false)
    const other = new FakeWebContents(2)
    setupPage(ipcMain, other).page.setBusy(true)
    await flush()
    expect(native.busy).toBe(true)
    other.destroy()
    expect(native.busy).toBe(false)
  })

  it('页面声明与主进程的显式 setBusy 互不清除；接入 dispose 时撤销页面的声明', async () => {
    const { ipcMain, appMcp, native, attachment } = setupMain()
    appMcp.setBusy(true)
    const wc = new FakeWebContents(1)
    setupPage(ipcMain, wc).page.setBusy(true)
    await flush()
    wc.destroy()
    expect(native.busy).toBe(true)
    appMcp.setBusy(false)
    expect(native.busy).toBe(false)
    setupPage(ipcMain, new FakeWebContents(2)).page.setBusy(true)
    await flush()
    appMcp.setBusy(true)
    appMcp.setBusy(false)
    expect(native.busy).toBe(true)
    attachment.dispose()
    expect(native.busy).toBe(false)
  })

  it('appMcp 不支持 beginBusy：op 回复错误，页面记警告', async () => {
    const ipcMain = new FakeIpcMain()
    const appMcp = createAppMcp({ appId: 'shop', appName: 'Shop', clientKind: 'hybrid', binding: fakeBinding, keepAlive: false })
    // 只有必需部分（旧版 @app-mcp/node 没有 beginBusy）
    const attachment = attachAppMcp({
      appMcp: { scope: (name) => appMcp.scope(name), instanceId: '', state: appMcp.state, onStateChange: () => () => {} },
      ipcMain,
      logger: { warn: vi.fn(), error: vi.fn() },
    })
    cleanups.push(() => {
      attachment.dispose()
      appMcp.dispose()
    })
    const { page, logger } = setupPage(ipcMain, new FakeWebContents(1))
    page.setBusy(true)
    await flush()
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('beginBusy'))
  })
})

describe('事件（event.*，spec/protocol.md 3.5）', () => {
  const shipped = { name: 'order.shipped', description: '订单已发货', payloadSchema: { type: 'object' } }

  it('页面声明转给主进程客户端；已连接时发出到达原生，未连接时页面丢弃', async () => {
    const { ipcMain, native } = setupMain()
    const { page } = setupPage(ipcMain, new FakeWebContents(1))
    page.declareEvent(shipped)
    await flush()
    expect(native.events.get('order.shipped')).toEqual({
      name: 'order.shipped',
      description: '订单已发货',
      payloadSchemaJson: '{"type":"object"}',
    })
    expect(page.emitEvent('order.shipped', { orderId: 'o0' })).toBe(false)
    native.emit({ type: 'state', state: { status: 'connected' } })
    await flush()
    expect(page.emitEvent('order.shipped', { orderId: 'o1' })).toBe(true)
    await flush()
    expect(native.emittedEvents).toEqual([{ name: 'order.shipped', payload: { orderId: 'o1' } }])
    expect(page.removeEvent('order.shipped')).toBe(true)
    await flush()
    expect(native.events.has('order.shipped')).toBe(false)
  })

  it('声明归页面：刷新 / 销毁后撤销；另一页仍声明同名事件时保留（以其声明为准）', async () => {
    const { ipcMain, native } = setupMain()
    const wc1 = new FakeWebContents(1)
    const wc2 = new FakeWebContents(2)
    const other = setupPage(ipcMain, wc2).page
    other.declareEvent({ name: 'order.shipped', description: '另一页的说明' })
    other.declareEvent({ name: 'only.two', description: 'x' })
    await flush()
    setupPage(ipcMain, wc1).page.declareEvent(shipped)
    await flush()
    expect(native.events.get('order.shipped')?.description).toBe('订单已发货')
    setupPage(ipcMain, wc1) // 刷新
    await flush()
    expect(native.events.get('order.shipped')?.description).toBe('另一页的说明')
    wc2.destroy()
    expect(native.events.has('order.shipped')).toBe(false)
    expect(native.events.has('only.two')).toBe(false)
  })

  it('主进程拒绝（名称不合法 / 未声明）：页面记警告；appMcp 不支持事件时回复 UNSUPPORTED', async () => {
    const { ipcMain } = setupMain()
    const wc = new FakeWebContents(1)
    // 绕过页面本地校验，直接发 op：未声明的事件
    const reply = (await ipcMain.invoke(wc, CHANNEL_OP, { op: 'event.emit', name: 'nope' })) as { ok: boolean; code?: string }
    expect(reply).toMatchObject({ ok: false, code: 'INVALID_NAME' })

    const ipcMain2 = new FakeIpcMain()
    const appMcp = createAppMcp({ appId: 'shop', appName: 'Shop', clientKind: 'hybrid', binding: fakeBinding, keepAlive: false })
    const attachment = attachAppMcp({
      appMcp: { scope: (name) => appMcp.scope(name), instanceId: '', state: appMcp.state, onStateChange: () => () => {} },
      ipcMain: ipcMain2,
      logger: { warn: vi.fn(), error: vi.fn() },
    })
    cleanups.push(() => {
      attachment.dispose()
      appMcp.dispose()
    })
    const { page, logger } = setupPage(ipcMain2, new FakeWebContents(3))
    page.declareEvent(shipped)
    await flush()
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('event.declare'))
  })
})
