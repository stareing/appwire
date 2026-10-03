/** 事件（spec/protocol.md 3.5）：declareEvent / removeEvent / emitEvent 与原生客户端（假原生模块）的对接。 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, type AppMcp } from './index.js'
import { FakeNativeClient, fakeBinding } from './testing/fake-native.js'

const created: AppMcp[] = []

function setup() {
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const app = createAppMcp({ appId: 'demo', appName: 'Demo', binding: fakeBinding, logger, keepAlive: false })
  created.push(app)
  const native = FakeNativeClient.last as FakeNativeClient
  const connect = () => native.emit({ type: 'state', state: { status: 'connected' } })
  return { app, native, logger, connect }
}

afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

const code = (c: string) => expect.objectContaining({ code: c })

describe('事件（spec/protocol.md 3.5）', () => {
  it('declareEvent 把 payloadSchema 序列化后交给原生；同名替换', () => {
    const { app, native } = setup()
    app.declareEvent({ name: 'order.shipped', description: '订单已发货', payloadSchema: { type: 'object' } })
    app.declareEvent({ name: 'download.done', description: '下载完成' })
    expect(native.events.get('order.shipped')).toEqual({
      name: 'order.shipped',
      description: '订单已发货',
      payloadSchemaJson: '{"type":"object"}',
    })
    expect(native.events.get('download.done')).toEqual({ name: 'download.done', description: '下载完成' })
    app.declareEvent({ name: 'download.done', description: '新的说明' })
    expect(native.events.get('download.done')?.description).toBe('新的说明')
  })

  it('emitEvent：未连接返回 false；已连接发送载荷并返回 true；无载荷时不带', () => {
    const { app, native, connect } = setup()
    app.declareEvent({ name: 'order.shipped', description: '订单已发货' })
    expect(app.emitEvent('order.shipped', { orderId: 'o0' })).toBe(false)
    connect()
    expect(app.emitEvent('order.shipped', { orderId: 'o1' })).toBe(true)
    expect(app.emitEvent('order.shipped')).toBe(true)
    expect(native.emittedEvents).toEqual([
      { name: 'order.shipped', payload: { orderId: 'o1' } },
      { name: 'order.shipped', payload: undefined },
    ])
  })

  it('错误：未声明 INVALID_NAME；载荷不是对象 / 无法序列化 INVALID_JSON；都不发送', () => {
    const { app, native, connect } = setup()
    connect()
    app.declareEvent({ name: 'order.shipped', description: '订单已发货' })
    expect(() => app.emitEvent('nope')).toThrow(code('INVALID_NAME'))
    expect(() => app.emitEvent('order.shipped', [1] as never)).toThrow(code('INVALID_JSON'))
    expect(() => app.emitEvent('order.shipped', { n: 1n } as never)).toThrow(code('INVALID_JSON'))
    expect(() => app.emitEvent('order.shipped', (() => 1) as never)).toThrow(code('INVALID_JSON'))
    expect(() => app.declareEvent({ name: 'x', description: 'x', payloadSchema: { n: 1n } as never })).toThrow(code('INVALID_JSON'))
    expect(() => app.declareEvent({ name: 'bad name', description: 'x' })).toThrow(code('INVALID_NAME'))
    expect(native.emittedEvents).toEqual([])
  })

  it('removeEvent 转给原生；撤销后发出为未声明', () => {
    const { app, connect } = setup()
    connect()
    app.declareEvent({ name: 'order.shipped', description: '订单已发货' })
    expect(app.removeEvent('order.shipped')).toBe(true)
    expect(app.removeEvent('order.shipped')).toBe(false)
    expect(() => app.emitEvent('order.shipped')).toThrow(code('INVALID_NAME'))
  })

  it('已注销：声明为空操作、发出返回 false', () => {
    const { app, native } = setup()
    app.declareEvent({ name: 'order.shipped', description: '订单已发货' })
    app.dispose()
    app.declareEvent({ name: 'late', description: 'x' })
    expect(native.events.has('late')).toBe(false)
    expect(app.emitEvent('order.shipped')).toBe(false)
    expect(app.removeEvent('order.shipped')).toBe(false)
  })

  it('旧版原生模块（没有事件方法）：记一条警告、无效果', () => {
    const { app, native, logger } = setup()
    for (const key of ['declareEvent', 'removeEvent', 'emitEvent'] as const) {
      ;(native as unknown as Record<string, unknown>)[key] = undefined
    }
    app.declareEvent({ name: 'order.shipped', description: '订单已发货' })
    expect(app.emitEvent('order.shipped')).toBe(false)
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('不支持事件'))
  })
})
