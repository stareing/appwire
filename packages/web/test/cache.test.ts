/**
 * 结果缓存声明 `cache`（spec/protocol.md 3.6）：驱动层透传到核心、Electron / Tauri 桥接透传到主进程。
 * 范围校验在核心（真实 WASM 见 wasm-smoke.test.ts）。
 */
import { describe, expect, it, vi } from 'vitest'
import type { CoreResourceDef, CoreToolDef } from '../src/core'
import { type AppMcpBridge, BRIDGE_VERSION, createBridgeAppMcp, type OpReply, type RendererOp } from '../src/electron-bridge'
import { connected, silentLogger } from './fakes'

const handler = (): null => null

function registered(h: Awaited<ReturnType<typeof connected>>, name: string): CoreToolDef | undefined {
  return h.core.callsOf('registerTool').map((c) => c[0] as CoreToolDef).find((d) => d.name === name)
}

describe('驱动层（WASM 核心）', () => {
  it('工具与资源注册时带 cache；未声明不带', async () => {
    const h = await connected()
    h.app.tool('feed.list', { description: '列表', risk: 'read', cache: { ttlMs: 5000, scope: 'shared' }, handler })
    h.app.tool('plain', { description: '普通', handler })
    h.app.scope('s').tool('scoped', { description: 'S', risk: 'read', cache: { ttlMs: 10 }, handler })
    h.app.resource('feed', { description: '订阅', cache: { ttlMs: 30000 }, read: () => [] })
    h.app.resource('cart', { description: '购物车', read: () => [] })
    expect(registered(h, 'feed.list')).toMatchObject({ cache: { ttlMs: 5000, scope: 'shared' } })
    expect(registered(h, 'plain')).not.toHaveProperty('cache')
    expect(registered(h, 'scoped')).toMatchObject({ cache: { ttlMs: 10 } })
    const resources = h.core.callsOf('registerResource').map((c) => c[0] as CoreResourceDef)
    expect(resources.find((r) => r.name === 'feed')).toMatchObject({ cache: { ttlMs: 30000 } })
    expect(resources.find((r) => r.name === 'cart')).not.toHaveProperty('cache')
    h.app.dispose()
  })

  it('update 整体替换；显式 undefined 清除（发 null）；未出现时不变', async () => {
    const h = await connected()
    const t = h.app.tool('feed.list', { description: '列表', risk: 'read', cache: { ttlMs: 5000 }, handler })
    t.update({ cache: { ttlMs: 1000, scope: 'shared' } })
    t.update({ cache: undefined })
    t.update({ description: '列表 2' })
    const updates = h.core.callsOf('updateTool').map((c) => c[1])
    expect(updates).toEqual([{ cache: { ttlMs: 1000, scope: 'shared' } }, { cache: null }, { description: '列表 2' }])
    h.app.dispose()
  })
})

describe('Electron 桥接', () => {
  function page() {
    const ops: RendererOp[] = []
    const bridge: AppMcpBridge = {
      version: BRIDGE_VERSION,
      request: vi.fn(async (op: RendererOp): Promise<OpReply> => {
        ops.push(structuredClone(op))
        if (op.op === 'hello') return { ok: true, value: { instanceId: 'main-1', state: { status: 'connected' } } }
        return { ok: true }
      }),
      onMessage: () => () => undefined,
    }
    const app = createBridgeAppMcp({ appId: 'feed', appName: 'Feed', logger: silentLogger() }, bridge)
    return { app, ops }
  }
  const settle = async (): Promise<void> => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
  }

  it('cache 随工具与资源定义发送；update 整体发送、缺省即清除', async () => {
    const { app, ops } = page()
    const t = app.tool('feed.list', { description: '列表', risk: 'read', cache: { ttlMs: 5000, scope: 'shared' }, handler })
    app.tool('plain', { description: '普通', handler })
    app.resource('feed', { description: '订阅', cache: { ttlMs: 30000 }, read: () => [] })
    await settle()
    const regs = ops.filter((o) => o.op === 'tool.register') as Extract<RendererOp, { op: 'tool.register' }>[]
    expect(regs.find((o) => o.name === 'feed.list')?.spec).toMatchObject({ cache: { ttlMs: 5000, scope: 'shared' } })
    expect(regs.find((o) => o.name === 'plain')?.spec).not.toHaveProperty('cache')
    expect(ops.find((o) => o.op === 'resource.register')).toMatchObject({ name: 'feed', cache: { ttlMs: 30000 } })
    t.update({ cache: { ttlMs: 7 } })
    await settle()
    t.update({ cache: undefined })
    await settle()
    const updates = ops.filter((o) => o.op === 'tool.update') as Extract<RendererOp, { op: 'tool.update' }>[]
    expect(updates.map((o) => o.spec)).toEqual([
      { description: '列表', risk: 'read', cache: { ttlMs: 7 } },
      { description: '列表', risk: 'read' },
    ])
  })
})
