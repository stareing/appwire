import { describe, expect, it, vi } from 'vitest'
import { ToolCallError, createAppMcp } from '../src/index'

describe('createAppMcp', () => {
  it('enabled: false 返回空操作实现', () => {
    const app = createAppMcp({ appId: 'shop', appName: '示例商城', enabled: false })
    expect(app.state).toEqual({ status: 'disabled' })
    expect(app.options.enabled).toBe(false)
    const listener = vi.fn()
    const off = app.onStateChange(listener)
    const scope = app.scope('cart')
    const t = scope.tool('cart.clear', { description: '', handler: () => {} })
    expect(t.name).toBe('cart.clear')
    t.update({ enabled: false })
    t.setHandler(() => {})
    t.dispose()
    const r = app.resource('cart.state', { description: '', read: () => 1 })
    r.notifyChanged()
    r.dispose()
    scope.scope('inner').dispose()
    scope.dispose()
    off()
    app.dispose()
    expect(listener).not.toHaveBeenCalled()
    expect(sessionStorage.length).toBe(0)
  })

  it('导出 ToolCallError', () => {
    const e = new ToolCallError('USER_REJECTED', 'no')
    expect(e).toBeInstanceOf(Error)
    expect(e.kind).toBe('USER_REJECTED')
    expect(e.name).toBe('ToolCallError')
  })
})
