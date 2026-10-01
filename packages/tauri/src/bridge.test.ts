/**
 * 插件注入脚本（crates/tauri-plugin/js/bridge.js）与 @app-mcp/web 桥接实现的对接：
 * 页面 → Rust 走 `__TAURI_INTERNALS__.invoke('plugin:app-mcp|op', {op})`，Rust → 页面走 eval 调用分发函数。
 */
import { describe, expect, it, vi } from 'vitest'
import { BRIDGE_VERSION, findElectronBridge, type AppMcpBridge, type MainEvent, type RendererOp } from '@app-mcp/web'
import { createTauriAppMcp, getTauriBridge, isTauri, TAURI_DISPATCH_FN, TAURI_OP_COMMAND } from './index'
import { BRIDGE_SCRIPT, createFakeTauri } from './fake-tauri'

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
