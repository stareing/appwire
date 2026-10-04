/**
 * 插件注入脚本（crates/tauri-plugin/js/bridge.js）与 @app-mcp/web 桥接实现的对接：
 * 页面 → Rust 走 `__TAURI_INTERNALS__.invoke('plugin:app-mcp|op', {op})`，Rust → 页面走 eval 调用分发函数。
 */
import { describe, expect, it, vi } from 'vitest'
import { BRIDGE_VERSION, findElectronBridge, type AppMcpBridge, type MainEvent, type RendererOp } from '@app-mcp/web'
import {
  attachTauriNavigation,
  createTauriAppMcp,
  getTauriBridge,
  isTauri,
  TAURI_DISPATCH_FN,
  TAURI_OP_COMMAND,
  ToolCallError,
} from './index'
import { BRIDGE_SCRIPT, createFakeTauri, defaultReply } from './fake-tauri'

const quiet = { debug() {}, warn: vi.fn(), error: vi.fn() }

function bridgeOf(win: Record<string, any>): AppMcpBridge {
  const bridge = getTauriBridge(win)
  if (!bridge) throw new Error('没有桥接')
  return bridge
}

describe('注入脚本', () => {
  it('暴露与 @app-mcp/web 兼容的桥接对象', () => {
    const { window } = createFakeTauri()
    const bridge = findElectronBridge(window)
    expect(bridge).toBeDefined()
    expect(bridge?.version).toBe(BRIDGE_VERSION)
    expect(Object.isFrozen(window.appMcpBridge)).toBe(true)
    expect(typeof window[TAURI_DISPATCH_FN]).toBe('function')
    expect(BRIDGE_SCRIPT).toContain(`'${TAURI_OP_COMMAND}'`)
    // 页面脚本不能替换桥接或分发函数。
    expect(() => {
      'use strict'
      window.appMcpBridge = {}
    }).toThrow()
    expect(isTauri(window)).toBe(true)
    expect(isTauri({})).toBe(false)
  })

  it('不覆盖已有的桥接（如重复注入）', () => {
    const existing = { version: 1, request: async () => ({ ok: true as const }), onMessage: () => () => {} }
    const { window } = createFakeTauri({ window: { appMcpBridge: existing } })
    expect(window.appMcpBridge).toBe(existing)
    expect(window[TAURI_DISPATCH_FN]).toBeUndefined()
  })

  it('请求经 invoke 发出，事件按顺序分发给监听器，可取消订阅', async () => {
    const fake = createFakeTauri()
    const bridge = bridgeOf(fake.window)
    await expect(bridge.request({ op: 'hello' })).resolves.toMatchObject({ ok: true, value: { instanceId: 'inst-1' } })
    expect(fake.ops).toEqual([{ op: 'hello' }])

    const seen: MainEvent[] = []
    const off = bridge.onMessage((e) => seen.push(e))
    const broken = bridge.onMessage(() => {
      throw new Error('监听器异常不影响其他监听器')
    })
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    fake.emit({ type: 'state', state: { status: 'dormant' } })
    fake.emit({ type: 'read', readId: 1, resourceId: 2 })
    off()
    broken()
    fake.emit({ type: 'state', state: { status: 'connected' } })
    expect(seen).toEqual([
      { type: 'state', state: { status: 'dormant' } },
      { type: 'read', readId: 1, resourceId: 2 },
    ])
    expect(error).toHaveBeenCalledTimes(2)
    error.mockRestore()
  })

  it('invoke 失败（如 capability 未授权）转为 OpReply 错误', async () => {
    const fake = createFakeTauri()
    fake.window.__TAURI_INTERNALS__.invoke = async () => {
      throw 'app-mcp.op not allowed. Permissions associated with this command: app-mcp:allow-op'
    }
    const reply = await bridgeOf(fake.window).request({ op: 'hello' })
    expect(reply).toMatchObject({ ok: false, code: 'IPC_ERROR' })
    expect(reply.ok === false && reply.message).toContain('app-mcp:default')
  })

  it('没有 Tauri IPC 时返回 NO_TAURI', async () => {
    const fake = createFakeTauri({ internals: false })
    await expect(bridgeOf(fake.window).request({ op: 'hello' })).resolves.toMatchObject({ ok: false, code: 'NO_TAURI' })
  })
})

describe('createTauriAppMcp', () => {
  it('工具登记、调用、取消、状态与注销', async () => {
    const fake = createFakeTauri()
    const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城', bridge: bridgeOf(fake.window), logger: quiet })
    const states: string[] = []
    appMcp.onStateChange((s) => states.push(s.status))

    let aborted: AbortSignal | undefined
    appMcp.tool<{ a: number }, number>('math.double', {
      description: '加倍',
      risk: 'read',
      input: { type: 'object', properties: { a: { type: 'number' } }, required: ['a'] },
      handler: ({ a }) => a * 2,
    })
    appMcp.tool('slow.op', {
      description: '慢操作',
      handler: (_input, { signal }) =>
        new Promise((_resolve, reject) => {
          aborted = signal
          signal.addEventListener('abort', () => reject(signal.reason))
        }),
    })
    const scope = appMcp.scope('panel')
    scope.tool('panel.close', { description: '关闭面板', handler: () => 'closed' })

    const register = (await fake.waitFor((op) => op.op === 'tool.register' && op.name === 'math.double')) as Extract<
      RendererOp,
      { op: 'tool.register' }
    >
    expect(register.spec).toMatchObject({ description: '加倍', risk: 'read', inputSchema: { required: ['a'] } })
    expect(fake.ops[0]).toEqual({ op: 'hello' })
    const scoped = (await fake.waitFor((op) => op.op === 'tool.register' && op.name === 'panel.close')) as Extract<
      RendererOp,
      { op: 'tool.register' }
    >
    const created = fake.ops.find((op) => op.op === 'scope.create') as Extract<RendererOp, { op: 'scope.create' }>
    expect(scoped.scopeId).toBe(created.id)
    expect(appMcp.instanceId).toBe('inst-1')

    // Rust → 页面：调用。
    fake.emit({ type: 'call', callId: 'c1', toolId: register.id, input: { a: 21 } })
    await expect(fake.waitFor((op) => op.op === 'call.result' && op.callId === 'c1')).resolves.toMatchObject({
      ok: true,
      data: 42,
    })

    // 取消：handler 的 signal 被中止，结果不再回传。
    const slow = (await fake.waitFor((op) => op.op === 'tool.register' && op.name === 'slow.op')) as Extract<
      RendererOp,
      { op: 'tool.register' }
    >
    fake.emit({ type: 'call', callId: 'c2', toolId: slow.id, input: {} })
    await vi.waitFor(() => expect(aborted).toBeDefined())
    fake.emit({ type: 'cancel', callId: 'c2', kind: 'TIMEOUT', message: '调用超时' })
    expect(aborted?.aborted).toBe(true)
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(fake.ops.some((op) => op.op === 'call.result' && op.callId === 'c2')).toBe(false)

    // 状态转发。
    fake.emit({ type: 'state', state: { status: 'dormant' } })
    expect(appMcp.state).toEqual({ status: 'dormant' })

    // 生命周期操作转给 Rust 侧。
    const hold = appMcp.hold()
    appMcp.wake()
    hold.release()
    await fake.waitFor((op) => op.op === 'lifecycle.release')
    expect(fake.ops.map((op) => op.op)).toEqual(expect.arrayContaining(['lifecycle.hold', 'lifecycle.wake']))

    scope.dispose()
    await fake.waitFor((op) => op.op === 'scope.dispose')
    appMcp.dispose()
    await fake.waitFor((op) => op.op === 'reset')
    expect(states).toEqual(['connected', 'dormant', 'stopped'])
  })

  it('用户正在操作：setBusy 经注入脚本发送 busy.set（本页声明变化时）；不提供 setBusyPolicy（Rust 侧配置）', async () => {
    const fake = createFakeTauri()
    const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城', bridge: bridgeOf(fake.window), logger: quiet })
    appMcp.setBusy(true)
    appMcp.setBusy(true)
    expect(appMcp.isBusy()).toBe(true)
    appMcp.setBusy(false)
    await fake.waitFor((op) => op.op === 'busy.set' && !op.busy)
    expect(JSON.parse(JSON.stringify(fake.ops.filter((op) => op.op === 'busy.set')))).toEqual([
      { op: 'busy.set', busy: true },
      { op: 'busy.set', busy: false },
    ])
    expect(appMcp.setBusyPolicy).toBeUndefined()
    appMcp.dispose()
  })

  it('事件：declareEvent / removeEvent / emitEvent 经注入脚本发送 event.* op；连接前发出丢弃，未声明抛 INVALID_NAME', async () => {
    const fake = createFakeTauri()
    const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城', bridge: bridgeOf(fake.window), logger: quiet })
    appMcp.declareEvent({ name: 'order.shipped', description: '订单已发货', payloadSchema: { type: 'object' } })
    expect(appMcp.emitEvent('order.shipped')).toBe(false)
    await vi.waitFor(() => expect(appMcp.state.status).toBe('connected'))
    expect(appMcp.emitEvent('order.shipped', { orderId: 'o1' })).toBe(true)
    expect(() => appMcp.emitEvent('nope')).toThrow(expect.objectContaining({ code: 'INVALID_NAME' }))
    expect(appMcp.removeEvent('order.shipped')).toBe(true)
    await fake.waitFor((op) => op.op === 'event.remove')
    expect(JSON.parse(JSON.stringify(fake.ops.filter((op) => op.op.startsWith('event.'))))).toEqual([
      { op: 'event.declare', event: { name: 'order.shipped', description: '订单已发货', payloadSchema: { type: 'object' } } },
      { op: 'event.emit', name: 'order.shipped', payload: { orderId: 'o1' } },
      { op: 'event.remove', name: 'order.shipped' },
    ])
    appMcp.dispose()
  })

  it('标准意图：工具的 implements 经注入脚本随 tool.register 送到 Rust 侧', async () => {
    const fake = createFakeTauri()
    const appMcp = createTauriAppMcp({ appId: 'web', appName: '浏览器', bridge: bridgeOf(fake.window), logger: quiet })
    appMcp.tool('open', { description: '打开链接', implements: ['link.open@1'], handler: () => null })
    const reg = await fake.waitFor((op) => op.op === 'tool.register')
    expect(reg).toMatchObject({ name: 'open', spec: { implements: ['link.open@1'] } })
    appMcp.dispose()
  })

  it('结果缓存声明：工具与资源的 cache 经注入脚本随 tool.register / resource.register 送到 Rust 侧', async () => {
    const fake = createFakeTauri()
    const appMcp = createTauriAppMcp({ appId: 'feed', appName: '订阅', bridge: bridgeOf(fake.window), logger: quiet })
    appMcp.tool('list', { description: '列表', risk: 'read', cache: { ttlMs: 5000, scope: 'shared' }, handler: () => null })
    appMcp.resource('feed', { description: '订阅', cache: { ttlMs: 30000 }, read: () => [] })
    const reg = await fake.waitFor((op) => op.op === 'tool.register')
    expect(reg).toMatchObject({ name: 'list', spec: { cache: { ttlMs: 5000, scope: 'shared' } } })
    const res = await fake.waitFor((op) => op.op === 'resource.register')
    expect(res).toMatchObject({ name: 'feed', cache: { ttlMs: 30000 } })
    appMcp.dispose()
  })

  it('USER_ACTION_REQUIRED 的类别与 reason / uri 经注入脚本送到 Rust 侧；缺省字段省略', async () => {
    const fake = createFakeTauri()
    const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城', bridge: bridgeOf(fake.window), logger: quiet })
    appMcp.tool('order.pay', {
      description: '付款',
      handler: () => {
        throw ToolCallError.userActionRequired('请先登录', { reason: 'login', uri: 'shop://login' })
      },
    })
    appMcp.tool('camera.scan', {
      description: '扫码',
      handler: () => {
        throw ToolCallError.userActionRequired('请切到前台')
      },
    })
    const toolId = async (name: string) =>
      ((await fake.waitFor((op) => op.op === 'tool.register' && op.name === name)) as Extract<RendererOp, { op: 'tool.register' }>).id
    fake.emit({ type: 'call', callId: 'c1', toolId: await toolId('order.pay'), input: {} })
    fake.emit({ type: 'call', callId: 'c2', toolId: await toolId('camera.scan'), input: {} })
    const full = await fake.waitFor((op) => op.op === 'call.result' && op.callId === 'c1')
    const bare = await fake.waitFor((op) => op.op === 'call.result' && op.callId === 'c2')
    // Tauri IPC 按 JSON 序列化
    expect(JSON.parse(JSON.stringify(full))).toEqual({
      op: 'call.result',
      callId: 'c1',
      ok: false,
      kind: 'USER_ACTION_REQUIRED',
      message: '请先登录',
      details: { reason: 'login', uri: 'shop://login' },
    })
    expect(JSON.parse(JSON.stringify(bare))).toEqual({
      op: 'call.result',
      callId: 'c2',
      ok: false,
      kind: 'USER_ACTION_REQUIRED',
      message: '请切到前台',
    })
    appMcp.dispose()
  })

  it('连接 ID 经注入脚本从 hello 回复与 state 事件到达页面', async () => {
    const fake = createFakeTauri()
    fake.reply = (op) =>
      op.op === 'hello'
        ? { ok: true, value: { instanceId: 'inst-1', state: { status: 'connected' }, connectionId: '3f9a1c-1' } }
        : { ok: true }
    const appMcp = createTauriAppMcp({ appId: 'shop', appName: '示例商城', bridge: bridgeOf(fake.window), logger: quiet })
    await fake.waitFor((op) => op.op === 'hello')
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(appMcp.connectionId).toBe('3f9a1c-1')

    fake.emit({ type: 'state', state: { status: 'backoff', retryAt: 1, code: 'CONNECT_FAILED' } })
    expect(appMcp.connectionId).toBeUndefined()
    fake.emit({ type: 'state', state: { status: 'connected' }, connectionId: '3f9a1c-2' })
    expect(appMcp.connectionId).toBe('3f9a1c-2')
    appMcp.dispose()
  })

  it('找不到桥接时抛出说明性错误；enabled: false 时为空操作', () => {
    expect(() => createTauriAppMcp({ appId: 'shop', appName: '示例商城' })).toThrow(/不在 Tauri WebView 中/)
    const disabled = createTauriAppMcp({ appId: 'shop', appName: '示例商城', enabled: false })
    expect(disabled.state).toEqual({ status: 'disabled' })
    disabled.tool('x.y', { description: 'x', handler: () => 1 }).dispose()
  })
})

describe('导航（spec/protocol.md 3.4）', () => {
  it('开启导航、执行回调并回复结果', async () => {
    const fake = createFakeTauri()
    const seen: unknown[] = []
    const nav = attachTauriNavigation(async (request) => {
      seen.push(request)
      if (request.page === 'login') throw new ToolCallError('NAVIGATION_DENIED', '需要先登录')
      if (request.page === 'broken') throw new Error('页面加载失败')
    }, bridgeOf(fake.window))
    await nav.ready
    expect(fake.ops).toContainEqual({ op: 'navigation.set', enabled: true })

    fake.emit({ type: 'navigate', navId: 1, page: 'cart', params: { sku: 'A-42' } } as never)
    expect(await fake.waitFor((op) => op.op === ('navigate.result' as never))).toEqual({ op: 'navigate.result', navId: 1, ok: true })
    expect(seen).toEqual([{ page: 'cart', params: { sku: 'A-42' } }])
    fake.emit({ type: 'navigate', navId: 2, page: 'login' } as never)
    expect(await fake.waitFor((op) => (op as { navId?: number }).navId === 2)).toEqual({
      op: 'navigate.result', navId: 2, ok: false, kind: 'NAVIGATION_DENIED', message: '需要先登录',
    })
    fake.emit({ type: 'navigate', navId: 3, page: 'broken', params: [1] } as never)
    expect(await fake.waitFor((op) => (op as { navId?: number }).navId === 3)).toEqual({
      op: 'navigate.result', navId: 3, ok: false, kind: 'NAVIGATION_FAILED', message: '页面加载失败',
    })
    expect(seen[2]).toEqual({ page: 'broken', params: undefined })

    nav.dispose()
    nav.dispose()
    await fake.waitFor((op) => op.op === ('navigation.set' as never) && (op as { enabled?: boolean }).enabled === false)
    fake.emit({ type: 'navigate', navId: 4, page: 'cart' } as never)
    await new Promise((r) => setTimeout(r, 20))
    expect(fake.ops.some((op) => (op as { navId?: number }).navId === 4)).toBe(false)
  })

  it('Rust 侧未开启导航时 ready 拒绝；找不到桥接时抛错', async () => {
    const fake = createFakeTauri()
    fake.reply = (op) =>
      op.op === ('navigation.set' as never) ? { ok: false, code: 'NAVIGATION_DISABLED', message: '插件未开启页面导航' } : defaultReply(op)
    await expect(attachTauriNavigation(() => {}, bridgeOf(fake.window)).ready).rejects.toThrow('插件未开启页面导航')
    expect(() => attachTauriNavigation(() => {})).toThrow('未找到 window.appMcpBridge')
  })
})
