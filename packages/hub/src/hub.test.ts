/** 封装层单元测试：用替身原生模块验证 JSON 往返、错误转换、事件多播与回调规整。 */
import { describe, expect, it, vi } from 'vitest'
import { fromNativeError, HubError } from './errors.js'
import { Hub } from './hub.js'
import type { HubBinding, NativeHub } from './native.js'

function fakeBinding() {
  const state = {
    config: undefined as unknown,
    listener: null as ((json: string) => void) | null,
    approval: undefined as ((json: string) => Promise<boolean>) | undefined,
    pairing: undefined as ((json: string) => Promise<boolean>) | undefined,
    waker: undefined as ((json: string) => Promise<string | null>) | null | undefined,
    eventHandler: undefined as ((json: string) => void) | null | undefined,
    shutdown: false,
    policy: undefined as unknown,
    agents: undefined as unknown,
    intentDefaults: undefined as unknown,
  }
  const native: NativeHub = {
    get listenAddr() {
      return '127.0.0.1:1234'
    },
    get ipcEndpoint() {
      return 'unix:/run/x/hub.sock'
    },
    get isShutdown() {
      return state.shutdown
    },
    async shutdown() {
      state.shutdown = true
    },
    apps: () => '[]',
    status: () =>
      JSON.stringify({
        service: 'app-mcp',
        version: '0.1.0',
        pid: 1,
        startedAtMs: 5,
        mcpHttp: false,
        auth: { tokenConfigured: false, tokenRequiredWithoutOrigin: false },
        mcpSessions: 0,
        apps: [{ appId: 'a', name: 'A', kind: 'app', state: 'disconnected', instances: [], lastError: { message: 'x', atMs: 1 } }],
        reports: [],
      }),
    policy: () => JSON.stringify({ rules: [], loadedAtMs: 7 }),
    setPolicy: (json) => {
      const p = JSON.parse(json) as { rules?: { id: string }[] }
      if (p.rules?.some((r) => r.id === 'bad id')) throw new Error('[INVALID_INPUT] 规则 id 不合法')
      state.policy = p
    },
    setAgents: (json) => {
      state.agents = JSON.parse(json)
    },
    intents: () => JSON.stringify({ defaults: state.intentDefaults ?? {}, lastError: 'x' }),
    setIntentDefaults: (json) => {
      const d = JSON.parse(json) as Record<string, string>
      if ('bad verb' in d) throw new Error('[INVALID_INPUT] 动词名不合法')
      state.intentDefaults = d
    },
    tools: (f) => JSON.stringify([{ name: 'a.b', filter: f ? JSON.parse(f) : null }]),
    resources: () => '[]',
    overview: (id) => (id === 'a' ? '{"appId":"a"}' : null),
    callTool: async (req) => {
      const r = JSON.parse(req) as { name: string }
      if (!r.name.includes('.')) throw new Error('[TOOL_NOT_FOUND] 不存在')
      return JSON.stringify({ callId: 'c', result: { ok: r }, stateHints: [], instanceId: null, overview: null })
    },
    callToolWithProgress: async (req, onProgress) => {
      const r = JSON.parse(req) as { name: string }
      if (!r.name.includes('.')) throw new Error('[TOOL_NOT_FOUND] 不存在')
      onProgress({ progress: 1, total: 2, message: '一半' })
      onProgress({ progress: 2, total: null, message: null })
      return JSON.stringify({ callId: 'p', result: { ok: r }, stateHints: [], instanceId: null, overview: null })
    },
    cancelCall: () => {},
    readResource: async () => '{"uri":"u","mimeType":null,"text":"1","blob":null}',
    subscribe: (uri) => {
      if (uri === 'bad') throw new Error('[RESOURCE_NOT_FOUND] 无')
    },
    unsubscribe: () => {},
    selectInstance: () => {},
    resetSession: () => {},
    exportTools: (format) => JSON.stringify([{ format }]),
    dispatch: async (format, call, session) => JSON.stringify({ format, call: JSON.parse(call), session }),
    serveHttp: async (addr) => addr,
    onEvent: (l) => {
      state.listener = l
    },
    setApprovalHandler: (h) => {
      state.approval = h
    },
    setPairingHandler: (h) => {
      state.pairing = h
    },
    setWaker: (h) => {
      state.waker = h
    },
    setEventHandler: (h) => {
      state.eventHandler = h
    },
  }
  const binding: HubBinding = {
    Hub: {
      async start(json) {
        state.config = JSON.parse(json ?? 'null')
        return native
      },
    },
  }
  return { binding, state }
}

describe('Hub 封装', () => {
  it('配置去掉封装层选项后传给原生层；JSON 往返', async () => {
    const { binding, state } = fakeBinding()
    const hub = await Hub.start({ binding, keepAlive: false, listen: null, approval: { requireAtOrAbove: 'payment' } })
    expect(state.config).toEqual({ listen: null, approval: { requireAtOrAbove: 'payment' } })
    expect(hub.wsUrl).toBe('ws://127.0.0.1:1234/app')
    expect(hub.listenAddr).toBe('127.0.0.1:1234')
    expect(hub.ipcEndpoint).toBe('unix:/run/x/hub.sock')
    expect(hub.tools({ maxRisk: 'read' })).toEqual([{ name: 'a.b', filter: { maxRisk: 'read' } }])
    expect(hub.overview('a')).toEqual({ appId: 'a' })
    expect(hub.overview('b')).toBeNull()
    const status = hub.status()
    expect(status.service).toBe('app-mcp')
    expect(status.apps[0]).toMatchObject({ appId: 'a', state: 'disconnected', lastError: { message: 'x' } })
    expect((await hub.callTool({ name: 'a.b', arguments: { x: 1 } })).result.ok).toEqual({
      name: 'a.b',
      arguments: { x: 1 },
    })
    expect(await hub.dispatch('anthropic', { type: 'tool_use', id: 'i', name: 'n', input: {} }, 's')).toEqual({
      format: 'anthropic',
      call: { type: 'tool_use', id: 'i', name: 'n', input: {} },
      session: 's',
    })
    expect(hub.exportTools('gemini')).toEqual([{ format: 'gemini' }])
    expect((await hub.readResource('u')).text).toBe('1')
  })

  it('策略：配置透传、setPolicy 序列化、policy() 解析、不合法规则转为 HubError(INVALID_INPUT)', async () => {
    const { binding, state } = fakeBinding()
    const policy = { rules: [{ id: 'no-pay', action: 'deny' as const, app: 'shop', tool: 'order.pay', hooks: ['call' as const] }] }
    const hub = await Hub.start({ binding, keepAlive: false, policy })
    expect(state.config).toEqual({ policy })
    hub.setPolicy({ rules: [{ id: 'h', action: 'hide', app: 'shop*' }] })
    expect(state.policy).toEqual({ rules: [{ id: 'h', action: 'hide', app: 'shop*' }] })
    expect(hub.policy()).toEqual({ rules: [], loadedAtMs: 7 })
    const e = (() => {
      try {
        hub.setPolicy({ rules: [{ id: 'bad id', action: 'hide', app: 'x' }] })
      } catch (err) {
        return err
      }
    })()
    expect(e).toBeInstanceOf(HubError)
    expect(e).toMatchObject({ kind: 'INVALID_INPUT' })
  })

  it('标准意图默认表：配置透传、setIntentDefaults 序列化、intents() 解析、不合法转为 HubError(INVALID_INPUT)', async () => {
    const { binding, state } = fakeBinding()
    const intentDefaults = { 'message.send': 'mail.compose.send' }
    const hub = await Hub.start({ binding, keepAlive: false, intentDefaults })
    expect(state.config).toEqual({ intentDefaults })
    hub.setIntentDefaults({ 'message.send@1': 'chat.send' })
    expect(state.intentDefaults).toEqual({ 'message.send@1': 'chat.send' })
    expect(hub.intents()).toEqual({ defaults: { 'message.send@1': 'chat.send' }, lastError: 'x' })
    expect(() => hub.setIntentDefaults({ 'bad verb': 'x.y' })).toThrow(expect.objectContaining({ kind: 'INVALID_INPUT' }))
    expect(state.intentDefaults).toEqual({ 'message.send@1': 'chat.send' })
  })

  it('Agent 登记：配置透传、setAgents 序列化为数组', async () => {
    const { binding, state } = fakeBinding()
    const agents = [{ name: 'claude', token: 'claude-0123456789abcdef0123456789abcdef' }]
    await Hub.start({ binding, keepAlive: false, agents }).then((hub) => hub.setAgents([]))
    expect(state.config).toEqual({ agents })
    expect(state.agents).toEqual([])
  })

  it('callTool 的 onProgress 收到进度；回调抛错交给 onListenerError；不传时走普通调用', async () => {
    const { binding } = fakeBinding()
    const errors: unknown[] = []
    const hub = await Hub.start({ binding, keepAlive: false, onListenerError: (e) => errors.push(e) })
    const seen: unknown[] = []
    const out = await hub.callTool({ name: 'a.b' }, { onProgress: (p) => seen.push(p) })
    expect(out.callId).toBe('p')
    expect(seen).toEqual([
      { progress: 1, total: 2, message: '一半' },
      { progress: 2, total: null, message: null },
    ])
    const bad = await hub.callTool(
      { name: 'a.b' },
      {
        onProgress: () => {
          throw new Error('回调炸了')
        },
      },
    )
    expect(bad.callId).toBe('p')
    expect(errors).toHaveLength(2)
    expect((await hub.callTool({ name: 'a.b' })).callId).toBe('c')
    await expect(hub.callTool({ name: 'x' }, { onProgress: () => {} })).rejects.toMatchObject({ kind: 'TOOL_NOT_FOUND' })
  })

  it('原生错误转为 HubError', async () => {
    const { binding } = fakeBinding()
    const hub = await Hub.start({ binding, keepAlive: false })
    await expect(hub.callTool({ name: 'x' })).rejects.toMatchObject({ kind: 'TOOL_NOT_FOUND', message: '不存在' })
    expect(() => hub.subscribe('bad')).toThrow(HubError)
    const plain = new Error('普通错误')
    expect(fromNativeError(plain)).toBe(plain)
  })

  it('缺少 cargo feature → UNSUPPORTED（与 START_FAILED 区分）', () => {
    const e = fromNativeError(new Error('[UNSUPPORTED] Hub 启动失败：本构建未包含上游聚合（… `upstream` 未开启）'))
    expect(e).toBeInstanceOf(HubError)
    expect(e).toMatchObject({ kind: 'UNSUPPORTED', code: 'UNSUPPORTED' })
    expect((e as HubError).message).toContain('`upstream`')
  })

  it('事件多播、监听器异常隔离、全部取消后注销原生回调', async () => {
    const { binding, state } = fakeBinding()
    const onListenerError = vi.fn()
    const hub = await Hub.start({ binding, keepAlive: false, onListenerError })
    const a: string[] = []
    const offA = hub.onEvent((e) => a.push(e.type))
    const offB = hub.onEvent(() => {
      throw new Error('坏监听器')
    })
    state.listener?.('{"type":"toolsChanged"}')
    expect(a).toEqual(['toolsChanged'])
    expect(onListenerError).toHaveBeenCalledTimes(1)
    const waiting = hub.waitForEvent((e) => e.type === 'resourceUpdated')
    state.listener?.('{"type":"resourceUpdated","uri":"x"}')
    expect(await waiting).toEqual({ type: 'resourceUpdated', uri: 'x' })
    offA()
    offB()
    expect(state.listener).toBeNull()
  })

  it('setEventHandler：解析 AppEvent JSON 后回调；回调异常交给 onListenerError；null 清除；旧原生模块抛 UNSUPPORTED_PROTOCOL', async () => {
    const { binding, state } = fakeBinding()
    const onListenerError = vi.fn()
    const hub = await Hub.start({ binding, keepAlive: false, onListenerError })
    const seen: unknown[] = []
    hub.setEventHandler((e) => seen.push(e))
    const event = { id: 'ev-1', appId: 'shop', instanceId: 'i1', name: 'order.shipped', payload: { orderId: 'o1' }, at: 1 }
    state.eventHandler?.(JSON.stringify(event))
    expect(seen).toEqual([event])
    hub.setEventHandler(() => {
      throw new Error('坏回调')
    })
    state.eventHandler?.(JSON.stringify(event))
    expect(onListenerError).toHaveBeenCalledTimes(1)
    hub.setEventHandler(null)
    expect(state.eventHandler).toBeNull()

    const old = fakeBinding()
    delete (await old.binding.Hub.start(null) as Partial<NativeHub>).setEventHandler
    const oldHub = await Hub.start({ binding: old.binding, keepAlive: false })
    expect(() => oldHub.setEventHandler(() => {})).toThrow(expect.objectContaining({ kind: 'UNSUPPORTED_PROTOCOL' }))
  })

  it('审批 / 配对回调规整为 Promise<boolean>：同步值、Promise、抛错、reject、非布尔', async () => {
    const { binding, state } = fakeBinding()
    const hub = await Hub.start({ binding, keepAlive: false })
    const req = JSON.stringify({ callId: 'c', risk: 'payment' })
    hub.setApprovalHandler(() => true)
    expect(await state.approval!(req)).toBe(true)
    hub.setApprovalHandler(async (r) => r.risk !== 'payment')
    expect(await state.approval!(req)).toBe(false)
    hub.setApprovalHandler(() => {
      throw new Error('x')
    })
    expect(await state.approval!(req)).toBe(false)
    hub.setApprovalHandler(() => Promise.reject(new Error('x')))
    expect(await state.approval!(req)).toBe(false)
    hub.setApprovalHandler(() => 'yes' as unknown as boolean)
    expect(await state.approval!(req)).toBe(false)
    hub.setPairingHandler(async (p) => p.appId === 'ok')
    expect(await state.pairing!('{"appId":"ok"}')).toBe(true)
    expect(await state.pairing!('not json')).toBe(false)
  })

  it('生命周期配置原样传给原生层', async () => {
    const { binding, state } = fakeBinding()
    await Hub.start({
      binding,
      keepAlive: false,
      leaseTtlMs: 0,
      wakeTimeoutMs: 2000,
      wakeTokenTtlMs: 3000,
      dormantTtlMs: 4000,
      dormantReplacedByNewInstance: false,
      wakeFromLaunch: true,
      wakeRateLimit: 2,
      legacyHeartbeat: true,
      lease: { adaptive: false, window: 4, idleRevokeMs: 0 },
    })
    expect(state.config).toEqual({
      leaseTtlMs: 0,
      wakeTimeoutMs: 2000,
      wakeTokenTtlMs: 3000,
      dormantTtlMs: 4000,
      dormantReplacedByNewInstance: false,
      wakeFromLaunch: true,
      wakeRateLimit: 2,
      legacyHeartbeat: true,
      lease: { adaptive: false, window: 4, idleRevokeMs: 0 },
    })
  })

  it('Waker 规整：resolve → null；抛错 → LAUNCH_FAILED；HubError 的协议类别透传；null 恢复默认', async () => {
    const { binding, state } = fakeBinding()
    const hub = await Hub.start({ binding, keepAlive: false })
    const req = JSON.stringify({
      appId: 'a',
      instanceId: 'i',
      descriptor: { kind: 'android-intent', target: 'p/.R', background: true },
      token: 't',
      activationArg: 'app-mcp-wake:t',
    })
    const seen: string[] = []
    hub.setWaker(async (r) => {
      seen.push(`${r.appId}/${r.instanceId}/${r.descriptor.kind}/${r.activationArg}`)
    })
    expect(await state.waker!(req)).toBeNull()
    expect(seen).toEqual(['a/i/android-intent/app-mcp-wake:t'])
    hub.setWaker(() => {
      throw new Error('坏了')
    })
    expect(JSON.parse((await state.waker!(req))!)).toEqual({ kind: 'LAUNCH_FAILED', message: '坏了' })
    hub.setWaker(() => Promise.reject(new HubError('APP_NOT_INSTALLED', '没装')))
    expect(JSON.parse((await state.waker!(req))!)).toEqual({ kind: 'APP_NOT_INSTALLED', message: '没装' })
    hub.setWaker(() => Promise.reject(new HubError('SHUTDOWN', 'x')))
    expect(JSON.parse((await state.waker!(req))!).kind).toBe('LAUNCH_FAILED')
    expect(JSON.parse((await state.waker!('not json'))!).kind).toBe('LAUNCH_FAILED')
    hub.setWaker(null)
    expect(state.waker).toBeNull()
  })

  it('keepAlive 定时器在 shutdown 时释放', async () => {
    const { binding, state } = fakeBinding()
    const set = vi.spyOn(globalThis, 'setInterval')
    const clear = vi.spyOn(globalThis, 'clearInterval')
    const hub = await Hub.start({ binding })
    expect(set).toHaveBeenCalledTimes(1)
    await hub.shutdown()
    expect(clear).toHaveBeenCalledTimes(1)
    expect(state.shutdown).toBe(true)
    set.mockRestore()
    clear.mockRestore()
  })
})
