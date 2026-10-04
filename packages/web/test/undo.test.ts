/**
 * 撤销（spec/protocol.md 3.8）：工具声明 `undoable` 与 handler 结果的 `undo` 经驱动层透传到核心、经 Electron / Tauri 桥接透传到主进程。
 * `undo` 的校验在核心（真实 WASM 见 wasm-smoke.test.ts 与一致性用例 result-undo）。
 */
import { describe, expect, it, vi } from 'vitest'
import type { CoreToolDef } from '../src/core'
import { type AppMcpBridge, BRIDGE_VERSION, createBridgeAppMcp, type OpReply, type RendererOp } from '../src/electron-bridge'
import { connected, settle, silentLogger } from './fakes'

const handler = (): null => null
const undo = { tool: 'todo.remove', arguments: { id: 3 }, label: '删除刚添加的待办' }

function registered(h: Awaited<ReturnType<typeof connected>>, name: string): CoreToolDef | undefined {
  return h.core.callsOf('registerTool').map((c) => c[0] as CoreToolDef).find((d) => d.name === name)
}

describe('驱动层（WASM 核心）', () => {
  it('注册时带 undoable；未声明不带；scope 内同样透传', async () => {
    const h = await connected()
    h.app.tool('todo.add', { description: '添加', undoable: true, handler })
    h.app.tool('plain', { description: '普通', handler })
    h.app.scope('s').tool('scoped', { description: 'S', undoable: false, handler })
    expect(registered(h, 'todo.add')?.undoable).toBe(true)
    expect(registered(h, 'plain')).not.toHaveProperty('undoable')
    expect(registered(h, 'scoped')?.undoable).toBe(false)
    h.app.dispose()
  })

  it('update：给值替换；显式 undefined 取消（发 false）；未出现时不变', async () => {
    const h = await connected()
    const t = h.app.tool('todo.add', { description: '添加', handler })
    t.update({ undoable: true })
    t.update({ undoable: undefined })
    t.update({ description: '添加 2' })
    const updates = h.core.callsOf('updateTool').map((c) => c[1])
    expect(updates).toEqual([{ undoable: true }, { undoable: false }, { description: '添加 2' }])
    h.app.dispose()
  })

  it('handler 结果的 undo 原样交给核心（不合法的也不在 JS 层拦截）；只有 data 的结果不带', async () => {
    const h = await connected()
    const results: unknown[] = [
      { data: { id: 3 }, summary: '已添加', undo },
      { data: 1, status: 'partial', undo: { tool: 'bad name', arguments: [1] } },
      { data: 2 },
    ]
    h.app.tool('t', { description: '', handler: () => results.shift() })
    const toolId = h.core.callsOf('registerTool').length
    for (const callId of ['c1', 'c2', 'c3']) h.socket().script({ type: 'invokeTool', callId, tool: toolId, name: 't', arguments: {} })
    await settle()
    expect(h.core.callsOf('completeCall').map((c) => c[1])).toEqual([
      { data: { id: 3 }, summary: '已添加', undo },
      { data: 1, status: 'partial', undo: { tool: 'bad name', arguments: [1] } },
      { data: 2 },
    ])
    h.app.dispose()
  })
})

describe('Electron 桥接', () => {
  function page() {
    const ops: RendererOp[] = []
    let deliver: ((message: unknown) => void) | undefined
    const bridge: AppMcpBridge = {
      version: BRIDGE_VERSION,
      request: vi.fn(async (op: RendererOp): Promise<OpReply> => {
        ops.push(structuredClone(op))
        if (op.op === 'hello') return { ok: true, value: { instanceId: 'main-1', state: { status: 'connected' } } }
        return { ok: true }
      }),
      onMessage: (listener) => {
        deliver = listener as (message: unknown) => void
        return () => undefined
      },
    }
    const app = createBridgeAppMcp({ appId: 'todo', appName: 'Todo', logger: silentLogger() }, bridge)
    return { app, ops, emit: (message: unknown) => deliver?.(message) }
  }
  const flush = async (): Promise<void> => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
  }

  it('undoable 随工具定义发送；update 整体发送、缺省即取消；结果的 undo 随 call.result 送出', async () => {
    const { app, ops, emit } = page()
    const t = app.tool('todo.add', { description: '添加', undoable: true, handler: () => ({ data: { id: 3 }, undo }) })
    app.tool('plain', { description: '普通', handler })
    await flush()
    const regs = ops.filter((o) => o.op === 'tool.register') as Extract<RendererOp, { op: 'tool.register' }>[]
    const add = regs.find((o) => o.name === 'todo.add')
    expect(add?.spec).toMatchObject({ undoable: true })
    expect(regs.find((o) => o.name === 'plain')?.spec).not.toHaveProperty('undoable')

    emit({ type: 'call', callId: 'c1', toolId: add?.id, input: {} })
    await flush()
    expect(ops.find((o) => o.op === 'call.result')).toEqual({ op: 'call.result', callId: 'c1', ok: true, data: { id: 3 }, undo })

    t.update({ undoable: undefined })
    await flush()
    const updates = ops.filter((o) => o.op === 'tool.update') as Extract<RendererOp, { op: 'tool.update' }>[]
    expect(updates.map((o) => o.spec)).toEqual([{ description: '添加' }])
  })
})
