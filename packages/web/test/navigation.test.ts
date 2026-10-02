/**
 * 导航回调（spec/protocol.md 3.4）：能力声明、`navigate` 事件 → 回调 → `completeNavigate`，以及拒绝 / 失败的映射。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ToolCallError } from '../src/types'
import { createViewLayer, openViewLayers } from '../src/view'
import { connected, setup, settle } from './fakes'

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  for (const layer of [...openViewLayers()]) layer.close()
  vi.restoreAllMocks()
})

/** 等到核心收到 completeNavigate。 */
async function completion(h: Awaited<ReturnType<typeof connected>>): Promise<unknown> {
  for (let i = 0; i < 200; i++) {
    const done = h.core.callsOf('completeNavigate')
    if (done.length > 0) return done[0]
    await new Promise((r) => setTimeout(r, 5))
  }
  throw new Error('没有 completeNavigate')
}

describe('能力声明', () => {
  it('加载前设置回调：核心在启动前打开导航能力', async () => {
    const h = setup({}, true)
    h.app.setNavigationHandler(() => {})
    await h.load()
    const methods = h.core.methods()
    expect(methods.indexOf('setNavigation')).toBeGreaterThanOrEqual(0)
    expect(methods.indexOf('setNavigation')).toBeLessThan(methods.indexOf('start'))
    expect(h.core.callsOf('setNavigation')).toEqual([[true]])
    h.app.dispose()
  })

  it('不设置回调时不声明；清除回调时关闭', async () => {
    const h = await connected()
    expect(h.core.callsOf('setNavigation')).toEqual([])
    h.app.setNavigationHandler(() => {})
    h.app.setNavigationHandler(null)
    expect(h.core.callsOf('setNavigation')).toEqual([[true], [false]])
    h.app.dispose()
  })

  it('连接建立后才设置：警告下次连接生效', async () => {
    const h = await connected()
    h.app.setNavigationHandler(() => {})
    expect(h.logger.warn).toHaveBeenCalledWith(expect.stringContaining('下次连接时生效'))
    h.app.dispose()
  })
})

describe('navigate 事件', () => {
  it('交给回调（页面与参数），完成后回复 {}', async () => {
    const h = setup({}, true)
    const handler = vi.fn(async () => {})
    h.app.setNavigationHandler(handler, { settleMs: 10 })
    await h.load()
    h.socket().open()
    h.socket().script({ type: 'navigate', navigate: 7, page: 'orders', params: { id: 'A1' } })
    expect(await completion(h)).toEqual([7, {}])
    expect(handler).toHaveBeenCalledWith({ page: 'orders', params: { id: 'A1' } })
    h.app.dispose()
  })

  it('参数缺省（null）时为 undefined', async () => {
    const h = await connected()
    const handler = vi.fn()
    h.app.setNavigationHandler(handler, { settleMs: 10 })
    h.socket().script({ type: 'navigate', navigate: 1, page: 'cart', params: null })
    await completion(h)
    expect(handler).toHaveBeenCalledWith({ page: 'cart', params: undefined })
    h.app.dispose()
  })

  it('没有回调：NAVIGATION_FAILED（unsupported）', async () => {
    const h = await connected()
    h.socket().script({ type: 'navigate', navigate: 2, page: 'cart', params: null })
    expect(await completion(h)).toEqual([
      2,
      { error: { kind: 'NAVIGATION_FAILED', message: expect.stringContaining('cart'), details: { reason: 'unsupported' } } },
    ])
    h.app.dispose()
  })

  it.each([
    ['navigationDenied', ToolCallError.navigationDenied('需要登录'), 'NAVIGATION_DENIED', 'app', '需要登录'],
    ['navigationFailed', ToolCallError.navigationFailed('页面不存在'), 'NAVIGATION_FAILED', 'error', '页面不存在'],
    ['无详情的 DENIED', new ToolCallError('NAVIGATION_DENIED', '不行'), 'NAVIGATION_DENIED', 'app', '不行'],
    ['普通异常', new Error('boom'), 'NAVIGATION_FAILED', 'error', 'boom'],
    ['其他类别', new ToolCallError('HANDLER_ERROR', 'x'), 'NAVIGATION_FAILED', 'error', 'x'],
  ])('回调抛出 %s', async (_label, error, kind, reason, text) => {
    const h = await connected()
    h.app.setNavigationHandler(() => {
      throw error
    })
    h.socket().script({ type: 'navigate', navigate: 3, page: 'p', params: null })
    const [, outcome] = (await completion(h)) as [number, { error: { kind: string; message: string; details: { reason: string } } }]
    expect(outcome.error.kind).toBe(kind)
    expect(outcome.error.details.reason).toBe(reason)
    expect(outcome.error.message).toContain(text)
    h.app.dispose()
  })

  it('有打开的层时拒绝且不调用回调；whileLayerOpen: allow 时照常导航', async () => {
    const h = await connected()
    const handler = vi.fn()
    h.app.setNavigationHandler(handler, { settleMs: 10 })
    const layer = createViewLayer('结算确认')
    layer.open()
    h.socket().script({ type: 'navigate', navigate: 4, page: 'orders', params: null })
    const [, outcome] = (await completion(h)) as [number, { error: { kind: string; message: string } }]
    expect(outcome.error).toMatchObject({ kind: 'NAVIGATION_DENIED', details: { reason: 'app' } })
    expect(outcome.error.message).toContain('结算确认')
    expect(handler).not.toHaveBeenCalled()

    h.core.calls = []
    h.app.setNavigationHandler(handler, { whileLayerOpen: 'allow', settleMs: 10 })
    h.socket().script({ type: 'navigate', navigate: 5, page: 'orders', params: null })
    expect(await completion(h)).toEqual([5, {}])
    expect(handler).toHaveBeenCalledTimes(1)
    h.app.dispose()
  })

  it('导航后新页面的 view 工具在回复之前注册', async () => {
    const h = await connected()
    const pageEl = document.body.appendChild(document.createElement('div'))
    pageEl.hidden = true
    h.app.tool('orders.list', { description: '订单', surface: 'view', page: 'orders', anchor: pageEl, handler: () => [] })
    h.app.setNavigationHandler(
      () => {
        pageEl.hidden = false
      },
      { settleMs: 20 },
    )
    h.core.calls = []
    h.socket().script({ type: 'navigate', navigate: 6, page: 'orders', params: null })
    await completion(h)
    await settle()
    const methods = h.core.methods()
    expect(methods.indexOf('updateTool')).toBeGreaterThanOrEqual(0)
    expect(methods.indexOf('updateTool')).toBeLessThan(methods.indexOf('completeNavigate'))
    h.app.dispose()
  })
})
