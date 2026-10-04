/**
 * 工具弃用声明 `deprecated`（spec/protocol.md 3.7）：驱动层透传到核心、Electron / Tauri 桥接透传到主进程。
 * 格式校验在核心（真实 WASM 见 wasm-smoke.test.ts）。
 */
import { describe, expect, it, vi } from 'vitest'
import type { CoreToolDef } from '../src/core'
import { type AppMcpBridge, BRIDGE_VERSION, createBridgeAppMcp, type OpReply, type RendererOp } from '../src/electron-bridge'
import { connected, silentLogger } from './fakes'

const handler = (): null => null
const deprecated = { message: '改用 orders.search：支持分页', replacement: 'orders.search', until: '2027-06-30' }

function registered(h: Awaited<ReturnType<typeof connected>>, name: string): CoreToolDef | undefined {
  return h.core.callsOf('registerTool').map((c) => c[0] as CoreToolDef).find((d) => d.name === name)
}

describe('驱动层（WASM 核心）', () => {
  it('注册时带 deprecated（复制，不与调用方共享对象）；未声明不带；scope 内同样透传', async () => {
    const h = await connected()
    const dep = { ...deprecated }
    h.app.tool('orders.list', { description: '列表', deprecated: dep, handler })
    h.app.tool('plain', { description: '普通', handler })
    h.app.scope('s').tool('scoped', { description: 'S', deprecated: { message: '即将移除' }, handler })
    expect(registered(h, 'orders.list')?.deprecated).toEqual(deprecated)
    expect(registered(h, 'orders.list')?.deprecated).not.toBe(dep)
    expect(registered(h, 'plain')).not.toHaveProperty('deprecated')
    expect(registered(h, 'scoped')?.deprecated).toEqual({ message: '即将移除' })
    h.app.dispose()
  })

  it('update 整体替换；显式 undefined 清除（发 null）；未出现时不变', async () => {
    const h = await connected()
    const t = h.app.tool('orders.list', { description: '列表', deprecated, handler })
    t.update({ deprecated: { message: '改用 orders.v2' } })
    t.update({ deprecated: undefined })
    t.update({ description: '列表 2' })
    const updates = h.core.callsOf('updateTool').map((c) => c[1])
    expect(updates).toEqual([{ deprecated: { message: '改用 orders.v2' } }, { deprecated: null }, { description: '列表 2' }])
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
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop', logger: silentLogger() }, bridge)
    return { app, ops }
  }
  const settle = async (): Promise<void> => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
  }

  it('deprecated 随工具定义发送；update 整体发送、缺省即清除', async () => {
    const { app, ops } = page()
    const t = app.tool('orders.list', { description: '列表', deprecated, handler })
    app.tool('plain', { description: '普通', handler })
    await settle()
    const regs = ops.filter((o) => o.op === 'tool.register') as Extract<RendererOp, { op: 'tool.register' }>[]
    expect(regs.find((o) => o.name === 'orders.list')?.spec).toMatchObject({ deprecated })
    expect(regs.find((o) => o.name === 'plain')?.spec).not.toHaveProperty('deprecated')
    t.update({ deprecated: { message: 'm' } })
    await settle()
    t.update({ deprecated: undefined })
    await settle()
    const updates = ops.filter((o) => o.op === 'tool.update') as Extract<RendererOp, { op: 'tool.update' }>[]
    expect(updates.map((o) => o.spec)).toEqual([
      { description: '列表', deprecated: { message: 'm' } },
      { description: '列表' },
    ])
  })
})
