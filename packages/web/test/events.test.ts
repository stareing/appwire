/**
 * 事件（spec/protocol.md 3.5）：驱动层的声明 / 撤销 / 发出与核心的同步、发出前校验，以及桥接页面的 `event.*` op。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BRIDGE_VERSION, createBridgeAppMcp, type AppMcpBridge, type OpReply, type RendererOp } from '../src/electron-bridge'
import { MAX_EVENT_PAYLOAD_BYTES } from '../src/events'
import { createAppMcp } from '../src/index'
import { connected, setup, silentLogger } from './fakes'

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  vi.restoreAllMocks()
})

const shipped = { name: 'order.shipped', description: '订单已发货', payloadSchema: { type: 'object' } }
const code = (c: string) => expect.objectContaining({ code: c })

describe('驱动层（WASM 核心）', () => {
  it('核心加载前：声明记录在 JS 侧、发出丢弃返回 false；加载时在启动前把声明同步给核心', async () => {
    const h = setup({}, true)
    h.app.declareEvent(shipped)
    h.app.declareEvent({ name: 'download.done', description: '下载完成' })
    expect(h.app.emitEvent('order.shipped', { orderId: 'o1' })).toBe(false)
    await h.load()
    const methods = h.core.methods()
    expect(h.core.callsOf('declareEvent')).toEqual([[shipped], [{ name: 'download.done', description: '下载完成' }]])
    expect(methods.lastIndexOf('declareEvent')).toBeLessThan(methods.indexOf('start'))
    expect(h.core.callsOf('emitEvent')).toEqual([])
    h.app.dispose()
  })

  it('加载前撤销的声明不同步；同名替换保持一份', async () => {
    const h = setup({}, true)
    h.app.declareEvent(shipped)
    h.app.declareEvent({ ...shipped, description: '新的说明' })
    h.app.declareEvent({ name: 'gone', description: 'x' })
    expect(h.app.removeEvent('gone')).toBe(true)
    expect(h.app.removeEvent('gone')).toBe(false)
    await h.load()
    expect(h.core.callsOf('declareEvent')).toEqual([[{ ...shipped, description: '新的说明' }]])
    h.app.dispose()
  })

  it('已连接：emitEvent 以 JSON 文本交给核心并返回核心的结果；无载荷时不带', async () => {
    const h = await connected()
    h.app.declareEvent(shipped)
    expect(h.core.callsOf('declareEvent')).toEqual([[shipped]])
    expect(h.app.emitEvent('order.shipped', { orderId: 'o1' })).toBe(true)
    expect(h.app.emitEvent('order.shipped')).toBe(true)
    expect(h.core.callsOf('emitEvent')).toEqual([
      ['order.shipped', '{"orderId":"o1"}'],
      ['order.shipped', undefined],
    ])
    h.core.setState({ status: 'dormant' })
    expect(h.app.emitEvent('order.shipped', {})).toBe(false)
    expect(h.app.removeEvent('order.shipped')).toBe(true)
    expect(h.app.removeEvent('order.shipped')).toBe(false)
    expect(h.core.callsOf('removeEvent')).toEqual([['order.shipped']])
    h.app.dispose()
  })

  it('本地错误（不交给核心）：未声明 / 名称不合法 INVALID_NAME，载荷不是对象 / 超限 INVALID_JSON，schema 不是对象 INVALID_SCHEMA', async () => {
    const h = await connected()
    h.app.declareEvent(shipped)
    expect(() => h.app.emitEvent('nope')).toThrow(code('INVALID_NAME'))
    expect(() => h.app.emitEvent('bad name')).toThrow(code('INVALID_NAME'))
    expect(() => h.app.emitEvent('order.shipped', 1 as never)).toThrow(code('INVALID_JSON'))
    expect(() => h.app.emitEvent('order.shipped', [1] as never)).toThrow(code('INVALID_JSON'))
    expect(() => h.app.emitEvent('order.shipped', null as never)).toThrow(code('INVALID_JSON'))
    const big = { s: 'x'.repeat(MAX_EVENT_PAYLOAD_BYTES) }
    expect(() => h.app.emitEvent('order.shipped', big)).toThrow(code('INVALID_JSON'))
    // 恰好在上限内（`{"s":"…"}` 共 8 字节外壳）：照常发出
    expect(h.app.emitEvent('order.shipped', { s: 'x'.repeat(MAX_EVENT_PAYLOAD_BYTES - 8) })).toBe(true)
    expect(() => h.app.declareEvent({ name: 'a b', description: 'x' })).toThrow(code('INVALID_NAME'))
    expect(() => h.app.declareEvent({ name: 'ok', description: 'x', payloadSchema: [] as never })).toThrow(code('INVALID_SCHEMA'))
    expect(h.core.callsOf('emitEvent')).toHaveLength(1)
    h.app.dispose()
  })

  it('dispose 后发出返回 false（仍做本地校验）', async () => {
    const h = await connected()
    h.app.declareEvent(shipped)
    h.app.dispose()
    expect(h.app.emitEvent('order.shipped')).toBe(false)
    expect(h.core.callsOf('emitEvent')).toEqual([])
  })

  it('enabled: false：空操作', () => {
    const app = createAppMcp({ appId: 'shop', appName: 'Shop', enabled: false })
    app.declareEvent(shipped)
    expect(app.emitEvent('order.shipped')).toBe(false)
    expect(app.removeEvent('order.shipped')).toBe(false)
  })
})

describe('桥接页面（Electron / Tauri）', () => {
  function page(reply: (op: RendererOp) => OpReply = () => ({ ok: true })) {
    const ops: RendererOp[] = []
    const bridge: AppMcpBridge = {
      version: BRIDGE_VERSION,
      request: async (op) => {
        ops.push(structuredClone(op))
        if (op.op === 'hello') return { ok: true, value: { instanceId: 'main-1', state: { status: 'connected' } } }
        return reply(op)
      },
      onMessage: () => () => {},
    }
    const logger = silentLogger()
    const app = createBridgeAppMcp({ appId: 'shop', appName: 'Shop', logger }, bridge)
    return { app, logger, eventOps: () => ops.filter((o) => o.op.startsWith('event.')) }
  }

  const settle = async () => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
  }

  it('声明 / 撤销 / 发出发送 event.* op；镜像状态不是 connected 时发出丢弃', async () => {
    const { app, eventOps } = page()
    app.declareEvent(shipped)
    expect(app.emitEvent('order.shipped', { orderId: 'o0' })).toBe(false)
    await settle()
    expect(app.state.status).toBe('connected')
    expect(app.emitEvent('order.shipped', { orderId: 'o1' })).toBe(true)
    expect(app.emitEvent('order.shipped')).toBe(true)
    expect(app.removeEvent('order.shipped')).toBe(true)
    expect(app.removeEvent('order.shipped')).toBe(false)
    await settle()
    expect(eventOps()).toEqual([
      { op: 'event.declare', event: shipped },
      { op: 'event.emit', name: 'order.shipped', payload: { orderId: 'o1' } },
      { op: 'event.emit', name: 'order.shipped' },
      { op: 'event.remove', name: 'order.shipped' },
    ])
    app.dispose()
  })

  it('本地校验与驱动层相同：未声明 INVALID_NAME、载荷不是对象 INVALID_JSON，不发送', async () => {
    const { app, eventOps } = page()
    app.declareEvent(shipped)
    await settle()
    expect(() => app.emitEvent('nope')).toThrow(code('INVALID_NAME'))
    expect(() => app.emitEvent('order.shipped', [1] as never)).toThrow(code('INVALID_JSON'))
    await settle()
    expect(eventOps().map((o) => o.op)).toEqual(['event.declare'])
    app.dispose()
  })

  it('主进程不支持（旧版回复错误）：记警告；dispose 后不再发送', async () => {
    const { app, logger, eventOps } = page((op) =>
      op.op.startsWith('event.') ? { ok: false, message: `未知的操作 "${op.op}"` } : { ok: true },
    )
    app.declareEvent(shipped)
    await settle()
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('event.declare'))
    app.dispose()
    expect(app.emitEvent('order.shipped')).toBe(false)
    app.declareEvent({ name: 'late', description: 'x' })
    await settle()
    expect(eventOps().map((o) => o.op)).toEqual(['event.declare'])
  })
})
