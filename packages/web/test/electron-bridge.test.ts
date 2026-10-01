import { afterEach, describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import {
  type AppMcpBridge,
  BRIDGE_VERSION,
  createBridgeAppMcp,
  findElectronBridge,
  type MainEvent,
  type OpReply,
  type RendererOp,
} from '../src/electron-bridge'
import { createAppMcp, ToolCallError } from '../src/index'
import { silentLogger } from './fakes'

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

/** 假主进程：记录收到的 op，按 op 回复；`emit` 向页面推送事件。 */
function fakeBridge(reply: (op: RendererOp) => OpReply = () => ({ ok: true })) {
  const ops: RendererOp[] = []
  const listeners = new Set<(event: MainEvent) => void>()
  const bridge: AppMcpBridge = {
    version: BRIDGE_VERSION,
    request: vi.fn(async (op: RendererOp): Promise<OpReply> => {
      ops.push(structuredClone(op))
      if (op.op === 'hello') return { ok: true, value: { instanceId: 'main-1', state: { status: 'connected' } } }
      return reply(op)
    }),
    onMessage(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
  }
  return {
    bridge,
    ops,
    emit: (event: MainEvent) => {
      for (const l of listeners) l(structuredClone(event))
    },
    listenerCount: () => listeners.size,
    results: () => ops.filter((o) => o.op === 'call.result'),
  }
}

const g = globalThis as Record<string, unknown>

afterEach(() => {
  delete g.appMcpBridge
  delete g.getAppMcpBridge
})

describe('findElectronBridge', () => {
  it('识别 window.appMcpBridge 与 window.getAppMcpBridge()', () => {
    expect(findElectronBridge()).toBeUndefined()
    const { bridge } = fakeBridge()
    g.appMcpBridge = bridge
    expect(findElectronBridge()).toBe(bridge)
    delete g.appMcpBridge
    g.getAppMcpBridge = () => bridge
    expect(findElectronBridge()).toBe(bridge)
    g.getAppMcpBridge = () => undefined
    expect(findElectronBridge()).toBeUndefined()
    g.getAppMcpBridge = () => {
      throw new Error('boom')
    }
    expect(findElectronBridge()).toBeUndefined()
  })

  it('形状或版本不对时忽略', () => {
    expect(findElectronBridge({ appMcpBridge: { request: () => {} } })).toBeUndefined()
    const { bridge } = fakeBridge()
    expect(findElectronBridge({ appMcpBridge: { ...bridge, version: 2 } })).toBeUndefined()
    expect(findElectronBridge({ custom: bridge }, 'custom')).toBe(bridge)
  })
})

describe('createAppMcp 在 Electron 页面中', () => {
  it('检测到桥接时走 IPC，不加载 WASM', async () => {
    const fake = fakeBridge()
    g.appMcpBridge = fake.bridge
    const app = createAppMcp({ appId: 'shop', appName: 'Shop', logger: silentLogger(), wasmUrl: 'data:,x' })
    app.tool('math.double', {
      description: '翻倍',
      risk: 'read',
      input: { type: 'object', properties: { n: { type: 'number' } } },
      handler: (i: { n: number }) => ({ data: i.n * 2, stateHints: ['counter'] }),
    })
    await settle()
    // 走 WebSocket + WASM 时，无效的 wasmUrl 会让状态变为 rejected；这里状态来自主进程
    expect(fake.ops.map((o) => o.op)).toEqual(['hello', 'tool.register'])
    expect(fake.ops[1]).toEqual({
      op: 'tool.register',
      id: 1,
      name: 'math.double',
      spec: { description: '翻倍', risk: 'read', inputSchema: { type: 'object', properties: { n: { type: 'number' } } } },
    })
    expect(app.instanceId).toBe('main-1')
    expect(app.state).toEqual({ status: 'connected' })

    fake.emit({ type: 'call', callId: 'c1', toolId: 1, input: { n: 4 } })
    await settle()
    expect(fake.results()).toEqual([{ op: 'call.result', callId: 'c1', ok: true, data: 8, stateHints: ['counter'] }])
    app.dispose()
    await settle()
    expect(fake.ops.at(-1)).toEqual({ op: 'reset' })
    expect(app.state).toEqual({ status: 'stopped' })
    expect(fake.listenerCount()).toBe(0)
  })

  it('enabled: false 时不使用桥接', () => {
    const fake = fakeBridge()
    g.appMcpBridge = fake.bridge
    const app = createAppMcp({ appId: 'shop', appName: 'Shop', enabled: false })
    expect(app.state).toEqual({ status: 'disabled' })
    expect(fake.bridge.request).not.toHaveBeenCalled()
  })
})

describe('createBridgeAppMcp', () => {
  function page() {
    const fake = fakeBridge()
    const logger = silentLogger()
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop', logger }, fake.bridge)
    return { ...fake, app, logger }
  }

  it('注解与输出 schema 随定义发送；结构化结果的字段原样回传', async () => {
    const { app, emit, results, ops } = page()
    const t = app.tool('order.submit', {
      description: '提交订单',
      annotations: { destructiveHint: false, openWorldHint: true },
      outputSchema: z.object({ orderId: z.string() }),
      handler: () => ({
        data: { orderId: 'o1' },
        status: 'pending',
        stateResource: 'order.state',
        summary: '等待付款',
        annotations: { audience: ['user'], priority: 1 },
      }),
    })
    await settle()
    expect(ops.find((o) => o.op === 'tool.register')).toMatchObject({
      spec: {
        annotations: { destructiveHint: false, openWorldHint: true },
        outputSchema: { type: 'object', properties: { orderId: { type: 'string' } } },
      },
    })
    emit({ type: 'call', callId: 'c1', toolId: 1, input: {} })
    await settle()
    expect(results()).toEqual([
      {
        op: 'call.result',
        callId: 'c1',
        ok: true,
        data: { orderId: 'o1' },
        status: 'pending',
        stateResource: 'order.state',
        summary: '等待付款',
        annotations: { audience: ['user'], priority: 1 },
      },
    ])
    // 更新时清除：整体发送的定义里不再带这两项
    t.update({ annotations: undefined, outputSchema: undefined })
    await settle()
    expect(ops.find((o) => o.op === 'tool.update')).toEqual({ op: 'tool.update', id: 1, spec: { description: '提交订单' } })
  })

  it('惰性 handler：首次调用加载并缓存，失败返回 HANDLER_ERROR 并可重试', async () => {
    const { app, emit, results, ops } = page()
    let attempt = 0
    const load = vi.fn(async () => {
      if (++attempt === 1) throw new Error('chunk 404')
      return { default: (i: { q: string }) => i.q.toUpperCase() }
    })
    app.tool('lazy', { description: '惰性', input: z.object({ q: z.string() }), load })
    await settle()
    expect(ops.find((o) => o.op === 'tool.register')).toMatchObject({ spec: { description: '惰性' } })
    expect(load).not.toHaveBeenCalled()

    emit({ type: 'call', callId: 'c0', toolId: 1, input: { q: 1 } })
    await settle()
    expect(results()[0]).toMatchObject({ callId: 'c0', ok: false, kind: 'INVALID_INPUT' })
    expect(load).not.toHaveBeenCalled()

    emit({ type: 'call', callId: 'c1', toolId: 1, input: { q: 'a' } })
    await settle()
    expect(results()[1]).toEqual({
      op: 'call.result',
      callId: 'c1',
      ok: false,
      kind: 'HANDLER_ERROR',
      message: '加载工具 lazy 的 handler 失败：chunk 404',
    })
    emit({ type: 'call', callId: 'c2', toolId: 1, input: { q: 'b' } })
    emit({ type: 'call', callId: 'c3', toolId: 1, input: { q: 'c' } })
    await settle()
    expect(results().slice(2)).toEqual([
      { op: 'call.result', callId: 'c2', ok: true, data: 'B' },
      { op: 'call.result', callId: 'c3', ok: true, data: 'C' },
    ])
    expect(load).toHaveBeenCalledTimes(2)
    expect(() => app.tool('none', { description: 'x' } as never)).toThrow(/缺少 handler/)
  })

  it('错误类别、取消、资源、scope、update 与 setHandler', async () => {
    const { app, emit, results, ops } = page()
    app.tool('reject', {
      description: 'r',
      handler: () => {
        throw new ToolCallError('USER_REJECTED', '不行')
      },
    })
    let signal: AbortSignal | undefined
    app.tool('slow', {
      description: 's',
      handler: (_i, ctx) => {
        signal = ctx.signal
        return new Promise(() => {})
      },
    })
    const scope = app.scope('dialog')
    scope.resource('dialog.state', { description: 'd', read: () => ({ open: true }) })
    const t = scope.tool('dialog.close', { description: '关闭', handler: () => 'old' })
    await settle()
    t.update({ description: '关闭对话框', enabled: false })
    t.setHandler(() => 'new')
    await settle()
    expect(ops.find((o) => o.op === 'tool.update')).toEqual({
      op: 'tool.update',
      id: 5,
      spec: { description: '关闭对话框', enabled: false },
    })
    expect(ops.find((o) => o.op === 'scope.create')).toEqual({ op: 'scope.create', id: 3, name: 'dialog' })
    expect(ops.find((o) => o.op === 'resource.register')).toMatchObject({ id: 4, scopeId: 3, name: 'dialog.state' })

    emit({ type: 'call', callId: 'c1', toolId: 1, input: {} })
    emit({ type: 'call', callId: 'c2', toolId: 2, input: {} })
    emit({ type: 'call', callId: 'c3', toolId: 5, input: {} })
    emit({ type: 'call', callId: 'c4', toolId: 99, input: {} })
    emit({ type: 'read', readId: 7, resourceId: 4 })
    await settle()
    emit({ type: 'cancel', callId: 'c2', kind: 'CANCELLED', message: '已取消' })
    expect(signal?.aborted).toBe(true)
    expect((signal?.reason as { kind?: string }).kind).toBe('CANCELLED')
    expect(results()).toEqual([
      { op: 'call.result', callId: 'c4', ok: false, kind: 'TOOL_NOT_FOUND', message: '工具已注销' },
      { op: 'call.result', callId: 'c1', ok: false, kind: 'USER_REJECTED', message: '不行' },
      { op: 'call.result', callId: 'c3', ok: true, data: 'new' },
    ])
    expect(ops.find((o) => o.op === 'read.result')).toEqual({ op: 'read.result', readId: 7, ok: true, data: { open: true } })

    emit({ type: 'state', state: { status: 'backoff', retryAt: 1 } })
    expect(app.state).toEqual({ status: 'backoff', retryAt: 1 })
    scope.dispose()
    await settle()
    expect(ops.at(-1)).toEqual({ op: 'scope.dispose', id: 3 })
    // 已注销 scope 上的注册为空操作
    scope.tool('late', { description: 'x', handler: () => 1 })
    await settle()
    expect(ops.at(-1)).toEqual({ op: 'scope.dispose', id: 3 })
  })

  it('主进程拒绝时记录错误', async () => {
    const fake = fakeBridge((op) => (op.op === 'tool.register' ? { ok: false, message: 'already registered' } : { ok: true }))
    const logger = silentLogger()
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop', logger }, fake.bridge)
    app.tool('dup', { description: 'x', handler: () => 1 })
    await settle()
    expect(logger.error).toHaveBeenCalledWith(expect.stringMatching(/tool\.register 失败：already registered/))
  })

  it('连接 ID：取自 hello 回复与 state 事件，缺省（旧主进程）时为 undefined', async () => {
    const fake = fakeBridge()
    const request = fake.bridge.request as ReturnType<typeof vi.fn>
    request.mockImplementationOnce(async () => ({
      ok: true,
      value: { instanceId: 'main-1', state: { status: 'connected' }, connectionId: 'ab12cd-3' },
    }))
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop', logger: silentLogger() }, fake.bridge)
    expect(app.connectionId).toBeUndefined()
    await settle()
    expect(app.connectionId).toBe('ab12cd-3')

    const seen: (string | undefined)[] = []
    app.onStateChange(() => seen.push(app.connectionId))
    fake.emit({ type: 'state', state: { status: 'backoff', retryAt: 1, code: 'CONNECT_FAILED' } })
    fake.emit({ type: 'state', state: { status: 'connected' }, connectionId: 'ab12cd-4' })
    // 旧主进程的 state 事件不带 connectionId；非字符串值忽略。
    fake.emit({ type: 'state', state: { status: 'connected' } })
    fake.emit({ type: 'state', state: { status: 'connected' }, connectionId: 42 as unknown as string })
    expect(seen).toEqual([undefined, 'ab12cd-4', undefined, undefined])
    app.dispose()
    expect(app.connectionId).toBeUndefined()
  })

  it('旧主进程的 hello 回复不带 connectionId', async () => {
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop', logger: silentLogger() }, fakeBridge().bridge)
    await settle()
    expect(app.state).toEqual({ status: 'connected' })
    expect(app.connectionId).toBeUndefined()
  })

  it('bridge 为 null 时为 disabled 空操作', () => {
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop' }, null)
    expect(app.state).toEqual({ status: 'disabled' })
    expect(app.connectionId).toBeUndefined()
    app.tool('t', { description: 't', handler: () => 1 }).dispose()
    app.dispose()
  })
})

describe('生命周期（桥接模式）', () => {
  it('wake / sleep / connectNow / hold 经 op 转发给主进程；release 幂等；dormant / waking 状态透传', async () => {
    const fake = fakeBridge()
    const app = createBridgeAppMcp({ appId: 'shop', appName: 's', logger: silentLogger() }, fake.bridge)
    await settle()
    const before = fake.ops.length
    app.wake()
    app.sleep()
    app.connectNow()
    const held = app.hold()
    const held2 = app.hold()
    held.release()
    held.release()
    await settle()
    const lifecycle = fake.ops.slice(before)
    expect(lifecycle.map((o) => o.op)).toEqual([
      'lifecycle.wake',
      'lifecycle.sleep',
      'lifecycle.connectNow',
      'lifecycle.hold',
      'lifecycle.hold',
      'lifecycle.release',
    ])
    const [h1, h2] = lifecycle.filter((o) => o.op === 'lifecycle.hold') as { holdId: number }[]
    expect(h1!.holdId).not.toBe(h2!.holdId)
    expect(lifecycle[5]).toEqual({ op: 'lifecycle.release', holdId: h1!.holdId })

    const states: string[] = []
    app.onStateChange((s) => states.push(s.status))
    fake.emit({ type: 'state', state: { status: 'dormant' } })
    fake.emit({ type: 'state', state: { status: 'waking' } })
    expect(states).toEqual(['dormant', 'waking'])
    expect(app.state).toEqual({ status: 'waking' })

    // dispose 后：reset 由主进程释放全部持有；之后的 release / hold / wake 不再发送。
    app.dispose()
    await settle()
    const afterDispose = fake.ops.length
    held2.release()
    app.hold().release()
    app.wake()
    await settle()
    expect(fake.ops.at(-1)).toEqual({ op: 'reset' })
    expect(fake.ops).toHaveLength(afterDispose)
  })

  it('handler 的 context.hold 转发为 lifecycle.hold / release', async () => {
    const fake = fakeBridge()
    const app = createBridgeAppMcp({ appId: 'shop', appName: 's', logger: silentLogger() }, fake.bridge)
    app.tool('t', {
      description: 't',
      handler: (_: unknown, ctx) => {
        const r = ctx.hold?.()
        r?.release()
        return typeof r?.release
      },
    })
    await settle()
    const reg = fake.ops.find((o) => o.op === 'tool.register') as { id: number }
    fake.emit({ type: 'call', callId: 'c1', toolId: reg.id, input: {} })
    await settle()
    expect(fake.results()).toEqual([{ op: 'call.result', callId: 'c1', ok: true, data: 'function' }])
    expect(fake.ops.filter((o) => o.op.startsWith('lifecycle.')).map((o) => o.op)).toEqual([
      'lifecycle.hold',
      'lifecycle.release',
    ])
    app.dispose()
  })

  it('无桥接（disabled）时生命周期为空操作', () => {
    const app = createBridgeAppMcp({ appId: 'shop', appName: 's', logger: silentLogger() }, null)
    app.wake()
    app.sleep()
    app.connectNow()
    app.hold().release()
    app.dispose()
  })
})
