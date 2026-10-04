/**
 * 真实 WASM 核心的冒烟测试：驱动层 + WasmClient + 模拟 Host 的 WebSocket。
 * 需要先运行 `pnpm --filter @app-mcp/web build:wasm`，否则跳过。
 */
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { describe, expect, it } from 'vitest'
import type { CoreFactory } from '../src/core'
import { AppMcpDriver } from '../src/driver'
import { ToolCallError, type ToolContext } from '../src/types'
import { wasmCoreFactory, type WasmBindings } from '../src/wasm-loader'
import { FakeSocket, settle, silentLogger } from './fakes'

// 测试从包目录运行（pnpm --filter / vitest 默认 root）
const glue = resolve(process.cwd(), 'src/wasm/app_mcp_wasm.js')
const wasm = resolve(process.cwd(), 'src/wasm/app_mcp_wasm_bg.wasm')
const available = existsSync(glue) && existsSync(wasm)

async function loadRealCore(): Promise<CoreFactory> {
  const mod = (await import(/* @vite-ignore */ pathToFileURL(glue).href)) as WasmBindings
  await mod.default({ module_or_path: readFileSync(wasm) })
  return wasmCoreFactory(mod)
}

type Json = { id?: number | string; method?: string; params?: any; result?: any; error?: any }

describe.skipIf(!available)('真实 WASM 核心', () => {
  it('握手、同步、调用、资源读取、错误处理', async () => {
    const sockets: FakeSocket[] = []
    const clock = { now: 10_000 }
    const logger = silentLogger()
    const app = new AppMcpDriver(
      {
        appId: 'shop',
        appName: '示例商城',
        logger,
        overview: { summary: '演示商城' },
      },
      {
        loadCore: loadRealCore,
        createWebSocket: (url) => {
          const s = new FakeSocket(url)
          sockets.push(s)
          return s
        },
        now: () => clock.now,
      },
    )
    const sent = (): Json[] => (sockets[0]?.sent ?? []).map((t) => JSON.parse(t) as Json)

    let adds = 0
    app.tool('cart.add', {
      description: '加入购物车',
      input: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] },
      risk: 'write',
      handler: ({ id }: { id: string }, ctx?: ToolContext) => {
        adds++
        ctx?.progress?.(1, 2, '校验库存')
        return { data: { added: id }, stateHints: ['cart.state'] }
      },
    })
    app.tool('order.submit', {
      description: '提交订单',
      risk: 'payment',
      annotations: { idempotentHint: false, openWorldHint: true },
      outputSchema: { type: 'object', properties: { orderId: { type: 'string' } } },
      handler: () => ({
        data: { orderId: 'o1' },
        status: 'pending',
        stateResource: 'order.state',
        summary: '已提交，等待付款',
        annotations: { audience: ['user', 'assistant'], priority: 0.5 },
      }),
    })
    app.tool('cart.fail', {
      description: '总是失败',
      handler: () => {
        throw new ToolCallError('USER_REJECTED', '用户拒绝')
      },
    })
    app.tool('cart.checkout', {
      description: '需要登录',
      handler: () => {
        throw ToolCallError.userActionRequired('登录已过期，请重新登录', { reason: 'login', uri: 'shop://login' })
      },
    })
    app.tool('cart.scan', {
      description: '需要切到前台',
      handler: () => {
        throw ToolCallError.userActionRequired('请切到前台')
      },
    })
    app.resource('cart.state', { description: '购物车', read: () => ({ items: 1 }) })

    for (let i = 0; i < 50 && sockets.length === 0; i++) await new Promise((r) => setTimeout(r, 10))
    expect(sockets).toHaveLength(1)
    expect(app.state.status).toBe('connecting')
    const ws = sockets[0] as FakeSocket
    ws.open()
    expect(app.state.status).toBe('handshaking')

    const hello = sent()[0] as Json
    expect(hello.method).toBe('app/hello')
    expect(hello.params).toMatchObject({
      appId: 'shop',
      clientKind: 'web',
      instanceId: app.instanceId,
      overview: { summary: '演示商城' },
    })

    ws.receive(
      JSON.stringify({
        jsonrpc: '2.0',
        id: hello.id,
        result: { status: 'paired', token: 'tk', protocolVersion: '1', hostVersion: '0.1.0' },
      }),
    )
    expect(app.state.status).toBe('connected')
    const methods = sent().map((m) => m.method)
    expect(methods).toEqual(['app/hello', 'tools/sync', 'resources/sync', 'app/visibility', 'app/ready'])
    const sync = sent()[1] as Json
    expect(sync.params.tools.map((t: { name: string }) => t.name).sort()).toEqual(['cart.add', 'cart.checkout', 'cart.fail', 'cart.scan', 'order.submit'])
    const submit = sync.params.tools.find((t: { name: string }) => t.name === 'order.submit')
    expect(submit).toMatchObject({
      risk: 'payment',
      annotations: { idempotentHint: false, openWorldHint: true },
      outputSchema: { type: 'object', properties: { orderId: { type: 'string' } } },
    })
    const add = sync.params.tools.find((t: { name: string }) => t.name === 'cart.add')
    expect(add).not.toHaveProperty('annotations')
    expect(add).not.toHaveProperty('outputSchema')
    expect(localStorage.getItem('app-mcp:shop:token')).toBe('tk')

    // 调用成功
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h1', method: 'tools/invoke', params: { callId: 'c1', name: 'cart.add', arguments: { id: 'p1' } } }))
    await settle()
    const r1 = sent().find((m) => m.id === 'h1') as Json
    expect(r1.result).toEqual({ data: { added: 'p1' }, stateHints: ['cart.state'] })
    // 进度（spec/protocol.md 3.3）
    expect(sent().find((m) => m.method === 'tools/progress')?.params).toEqual({
      callId: 'c1',
      progress: 1,
      total: 2,
      message: '校验库存',
    })
    // 同一 callId 再次到达：重放首次结果，handler 不再执行
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h1b', method: 'tools/invoke', params: { callId: 'c1', name: 'cart.add', arguments: { id: 'p1' } } }))
    await settle()
    expect((sent().find((m) => m.id === 'h1b') as Json).result).toEqual(r1.result)
    expect(adds).toBe(1)

    // 结构化结果
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h4', method: 'tools/invoke', params: { callId: 'c4', name: 'order.submit', arguments: {} } }))
    await settle()
    expect((sent().find((m) => m.id === 'h4') as Json).result).toEqual({
      data: { orderId: 'o1' },
      status: 'pending',
      stateResource: 'order.state',
      summary: '已提交，等待付款',
      annotations: { audience: ['user', 'assistant'], priority: 0.5 },
    })

    // ToolCallError
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h2', method: 'tools/invoke', params: { callId: 'c2', name: 'cart.fail', arguments: {} } }))
    await settle()
    const r2 = sent().find((m) => m.id === 'h2') as Json
    expect(r2.error).toMatchObject({ code: -32004, message: '用户拒绝', data: { kind: 'USER_REJECTED' } })

    // USER_ACTION_REQUIRED：详情经 WASM 核心并入 data；缺省字段省略
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h5', method: 'tools/invoke', params: { callId: 'c5', name: 'cart.checkout', arguments: {} } }))
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h6', method: 'tools/invoke', params: { callId: 'c6', name: 'cart.scan', arguments: {} } }))
    await settle()
    expect((sent().find((m) => m.id === 'h5') as Json).error).toEqual({
      code: -32019,
      message: '登录已过期，请重新登录',
      data: { kind: 'USER_ACTION_REQUIRED', reason: 'login', uri: 'shop://login' },
    })
    expect((sent().find((m) => m.id === 'h6') as Json).error).toEqual({
      code: -32019,
      message: '请切到前台',
      data: { kind: 'USER_ACTION_REQUIRED' },
    })

    // 资源读取
    ws.receive(JSON.stringify({ jsonrpc: '2.0', id: 'h3', method: 'resources/read', params: { name: 'cart.state' } }))
    await settle()
    const r3 = sent().find((m) => m.id === 'h3') as Json
    expect(r3.result).toMatchObject({ contents: { items: 1 } })

    // 注册变更 → tools/changed
    const t = app.tool('cart.clear', { description: '清空', handler: () => null })
    expect(sent().at(-1)).toMatchObject({ method: 'tools/changed', params: { upserted: [{ name: 'cart.clear' }] } })
    t.dispose()
    expect(sent().at(-1)).toMatchObject({ method: 'tools/changed', params: { removed: ['cart.clear'] } })

    // 断开 → backoff，定时器到期后重连
    ws.fail()
    expect(app.state.status).toBe('backoff')

    // 核心错误以 JS Error 抛出，由驱动层记录
    expect(logger.error).not.toHaveBeenCalled()
    app.dispose()
    expect(app.state.status).toBe('stopped')
  })

  it('WasmClient 直接使用：错误转为 JS Error，事件为 tagged 对象', async () => {
    const factory = await loadRealCore()
    const core = factory({ appId: 'shop', appName: 's', instanceId: 'i' })
    expect(core.state()).toEqual({ status: 'idle' })
    expect(core.pollEvent()).toBeUndefined()
    expect(core.pollTimeout()).toBeUndefined()
    const id = core.registerTool({ name: 'a', description: '', inputSchema: { type: 'object' } })
    expect(typeof id).toBe('number')
    expect(() => core.registerTool({ name: 'a', description: '', inputSchema: { type: 'object' } })).toThrow(Error)
    expect(() => core.registerTool({ name: 'bad name', description: '', inputSchema: { type: 'object' } })).toThrow(/invalid name/)
    expect(() => core.unregisterTool(9999)).toThrow(/unknown tool/)
    expect(() => core.setVisibility('gone' as never, true, 0)).toThrow(/可见性/)
    const drain = (): unknown[] => {
      const out: unknown[] = []
      for (let e = core.pollEvent(); e; e = core.pollEvent()) out.push(e)
      return out
    }
    core.start(0)
    expect(drain()).toEqual(
      expect.arrayContaining([{ type: 'connect' }, { type: 'stateChanged', state: { status: 'connecting' } }]),
    )
    core.handleDisconnected(0)
    const events = drain()
    expect(events).toContainEqual({ type: 'stateChanged', state: { status: 'backoff', retryAt: 500 } })
    expect(core.pollTimeout()).toBe(500)
    core.free?.()
  })

  it('握手结果的 service 不是 app-mcp → host-mismatch，不再定时重连', async () => {
    const factory = await loadRealCore()
    const core = factory({ appId: 'shop', appName: 's', instanceId: 'i' })
    const drain = (): any[] => {
      const out: any[] = []
      for (let e = core.pollEvent(); e; e = core.pollEvent()) out.push(e)
      return out
    }
    core.start(0)
    core.handleConnected(0)
    const hello = drain().find((e) => e.type === 'send')
    const id = JSON.parse(hello.text).id
    core.handleMessage(
      JSON.stringify({
        jsonrpc: '2.0',
        id,
        result: { status: 'paired', protocolVersion: '1', hostVersion: 'x', service: 'other' },
      }),
      1,
    )
    const state = core.state()
    expect(state.status).toBe('host-mismatch')
    expect((state as { reason: string }).reason).toMatch(/不是 app-mcp/)
    expect((state as { code: string }).code).toBe('HOST_NOT_APP_MCP')
    expect(drain()).toContainEqual({ type: 'disconnect' })
    expect(core.pollTimeout()).toBeUndefined()
    // 网页不核对用户（浏览器不知道操作系统用户）：带 user 的 app-mcp 结果照常连接
    core.connectNow(2)
    core.handleConnected(2)
    const hello2 = drain().find((e) => e.type === 'send')
    core.handleMessage(
      JSON.stringify({
        jsonrpc: '2.0',
        id: JSON.parse(hello2.text).id,
        result: { status: 'paired', protocolVersion: '1', hostVersion: 'x', service: 'app-mcp', user: '0', pid: 1 },
      }),
      3,
    )
    expect(core.state()).toEqual({ status: 'connected' })
    core.free?.()
  })

  it('诊断（spec/protocol.md 第 10 节）：连接失败带错误码，记下的问题在握手成功后以 app/diagnostic 上报，连接 ID', async () => {
    const factory = await loadRealCore()
    const core = factory({ appId: 'shop', appName: 's', instanceId: 'i' })
    const drain = (): any[] => {
      const out: any[] = []
      for (let e = core.pollEvent(); e; e = core.pollEvent()) out.push(e)
      return out
    }
    core.start(0)
    drain()
    core.handleConnectFailed('CONNECT_FAILED', '无法连接', 0)
    expect(core.state()).toEqual({ status: 'backoff', retryAt: 500, reason: '无法连接', code: 'CONNECT_FAILED' })
    expect(() => core.handleConnectFailed('NOPE', 'x', 0)).toThrow(/未知错误码/)
    // 已建立连接的断开：handleDisconnectedWith 同样带码进入 backoff
    core.connectNow(0)
    core.handleConnected(0)
    core.handleDisconnectedWith('CONNECTION_CLOSED', 'Host 关闭了连接（关闭码 1001）', 0)
    expect(core.state()).toMatchObject({ status: 'backoff', reason: 'Host 关闭了连接（关闭码 1001）', code: 'CONNECTION_CLOSED' })
    expect(() => core.handleDisconnectedWith('NOPE', 'x', 0)).toThrow(/未知错误码/)
    drain()
    core.reportIssue('BLOCKED_CSP', 'CSP 不允许')
    core.connectNow(1)
    core.handleConnected(1)
    const hello = drain().find((e) => e.type === 'send')
    core.handleMessage(
      JSON.stringify({
        jsonrpc: '2.0',
        id: JSON.parse(hello.text).id,
        result: { status: 'paired', protocolVersion: '1', hostVersion: 'x', service: 'app-mcp', connectionId: 'ab12cd-3' },
      }),
      2,
    )
    expect(core.connectionId()).toBe('ab12cd-3')
    const sent = drain()
      .filter((e) => e.type === 'send')
      .map((e) => JSON.parse(e.text))
    expect(sent.at(-1)).toEqual({
      jsonrpc: '2.0',
      method: 'app/diagnostic',
      params: { code: 'BLOCKED_CSP', message: 'CSP 不允许', count: 1 },
    })
    core.free?.()
  })

  it('标准意图 implements（spec/intents.md）：注册进 tools/sync，格式不合法时注册抛错，更新为空即清除', async () => {
    const factory = await loadRealCore()
    const core = factory({ appId: 'mail', appName: 'm', instanceId: 'i' })
    const drain = (): any[] => {
      const out: any[] = []
      for (let e = core.pollEvent(); e; e = core.pollEvent()) out.push(e)
      return out
    }
    const send = core.registerTool({
      name: 'compose.send',
      description: '发信',
      inputSchema: { type: 'object', properties: { to: { type: 'array', items: { type: 'string' } }, text: { type: 'string' } } },
      implements: ['message.send@1'],
    })
    core.registerTool({ name: 'plain', description: '普通', inputSchema: { type: 'object', properties: {} } })
    expect(() =>
      core.registerTool({ name: 'bad', description: 'x', inputSchema: { type: 'object', properties: {} }, implements: ['no-version'] }),
    ).toThrow()
    core.start(0)
    core.connectNow(0)
    core.handleConnected(0)
    const hello = drain().find((e) => e.type === 'send')
    core.handleMessage(
      JSON.stringify({ jsonrpc: '2.0', id: JSON.parse(hello.text).id, result: { status: 'paired', protocolVersion: '1', hostVersion: 'x', service: 'app-mcp' } }),
      1,
    )
    const sync = drain().map((e) => (e.type === 'send' ? JSON.parse(e.text) : null)).find((m) => m?.method === 'tools/sync')
    const byName = (n: string) => sync.params.tools.find((t: { name: string }) => t.name === n)
    expect(byName('compose.send').implements).toEqual(['message.send@1'])
    expect(byName('plain')).not.toHaveProperty('implements')
    expect(byName('bad')).toBeUndefined()
    core.updateTool(send, { implements: [] })
    const changed = JSON.stringify(drain().filter((e) => e.type === 'send').map((e) => JSON.parse(e.text)))
    expect(changed).toContain('compose.send')
    expect(changed).not.toContain('message.send@1')
    core.free?.()
  })
})
