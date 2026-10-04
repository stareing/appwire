/**
 * 标准意图声明 `implements`（spec/intents.md 第 1、5 节）：驱动层透传到核心、Electron 桥接透传到主进程。
 * 格式校验在核心（真实 WASM 见 wasm-smoke.test.ts）。
 */
import { describe, expect, it, vi } from 'vitest'
import type { CoreToolDef } from '../src/core'
import { type AppMcpBridge, BRIDGE_VERSION, createBridgeAppMcp, type OpReply, type RendererOp } from '../src/electron-bridge'
import { connected, silentLogger } from './fakes'

const handler = (): null => null

function registered(h: Awaited<ReturnType<typeof connected>>, name: string): CoreToolDef | undefined {
  return h.core.callsOf('registerTool').map((c) => c[0] as CoreToolDef).find((d) => d.name === name)
}

describe('驱动层（WASM 核心）', () => {
  it('注册时带 implements；缺省或空数组不带', async () => {
    const h = await connected()
    h.app.tool('mail.send', { description: '发信', implements: ['message.send@1'], handler })
    h.app.tool('plain', { description: '普通', handler })
    h.app.tool('empty', { description: '空', implements: [], handler })
    h.app.scope('s').tool('scoped', { description: 'S', implements: ['link.open@1'], handler })
    expect(registered(h, 'mail.send')).toMatchObject({ implements: ['message.send@1'] })
    expect(registered(h, 'plain')).not.toHaveProperty('implements')
    expect(registered(h, 'empty')).not.toHaveProperty('implements')
    expect(registered(h, 'scoped')).toMatchObject({ implements: ['link.open@1'] })
    h.app.dispose()
  })

  it('update 整体替换；显式 undefined 清除（发空数组）；未出现时不变', async () => {
    const h = await connected()
    const t = h.app.tool('mail.send', { description: '发信', implements: ['message.send@1'], handler })
    t.update({ implements: ['message.send@1', 'file.share@1'] })
    t.update({ implements: undefined })
    t.update({ description: '发信 2' })
    const updates = h.core.callsOf('updateTool').map((c) => c[1])
    expect(updates).toEqual([
      { implements: ['message.send@1', 'file.share@1'] },
      { implements: [] },
      { description: '发信 2' },
    ])
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
    const app = createBridgeAppMcp({ appId: 'mail', appName: 'Mail', logger: silentLogger() }, bridge)
    return { app, ops }
  }
  const settle = async (): Promise<void> => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
  }

  it('implements 随定义发送；update 缺省即清除', async () => {
    const { app, ops } = page()
    const t = app.tool('mail.send', { description: '发信', implements: ['message.send@1'], handler })
    app.tool('plain', { description: '普通', implements: [], handler })
    await settle()
    const regs = ops.filter((o) => o.op === 'tool.register') as Extract<RendererOp, { op: 'tool.register' }>[]
    expect(regs.find((o) => o.name === 'mail.send')?.spec).toMatchObject({ implements: ['message.send@1'] })
    expect(regs.find((o) => o.name === 'plain')?.spec).not.toHaveProperty('implements')
    t.update({ implements: undefined })
    await settle()
    expect(ops.find((o) => o.op === 'tool.update')).toEqual({ op: 'tool.update', id: 1, spec: { description: '发信' } })
  })
})
