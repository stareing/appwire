import { EventEmitter } from 'node:events'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, type AppMcp } from '@app-mcp/node'
import { FakeNativeClient, fakeBinding } from '../../node/src/testing/fake-native.js'
import { attachLifecycle, type BrowserWindowLike, type ElectronAppLike } from './main.js'

const created: AppMcp[] = []
afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

function setup() {
  const appMcp = createAppMcp({ appId: 'demo', appName: 'Demo', binding: fakeBinding, keepAlive: false })
  created.push(appMcp)
  const native = FakeNativeClient.last as FakeNativeClient
  const app = Object.assign(new EventEmitter(), { quit: vi.fn() }) as unknown as ElectronAppLike &
    EventEmitter & { quit: ReturnType<typeof vi.fn> }
  return { appMcp, native, app }
}

class FakeWindow extends EventEmitter implements BrowserWindowLike {
  visible = true
  minimized = false
  focused = true
  isVisible() {
    return this.visible
  }
  isMinimized() {
    return this.minimized
  }
  isFocused() {
    return this.focused
  }
}

describe('attachLifecycle', () => {
  it('启动参数、second-instance 与 open-url 交给 handleWake', () => {
    const { appMcp, native, app } = setup()
    const onWake = vi.fn()
    attachLifecycle({ appMcp, app, argv: ['/app', 'app-mcp-wake:t1'], onWake })
    expect(onWake).toHaveBeenCalledTimes(1)

    app.emit('second-instance', {}, ['/app', '--other'])
    expect(onWake).toHaveBeenCalledTimes(1)
    app.emit('second-instance', {}, ['/app', 'app-mcp-wake:t2'])
    expect(onWake).toHaveBeenCalledTimes(2)

    const event = { preventDefault: vi.fn() }
    app.emit('open-url', event, 'demo://other')
    expect(event.preventDefault).not.toHaveBeenCalled()
    app.emit('open-url', event, 'demo://app-mcp/wake?token=t3')
    expect(event.preventDefault).toHaveBeenCalledTimes(1)
    expect(onWake).toHaveBeenCalledTimes(3)
    expect(native.lifecycleCalls).toContain('handleWake:app-mcp-wake:t2')
    expect(native.lifecycleCalls).toContain('handleWake:demo://app-mcp/wake?token=t3')
  })

  it('idle-exit 时调用 app.quit()；quitOnIdleExit: false 时不调用；解除后不再响应', () => {
    const { appMcp, native, app } = setup()
    const detach = attachLifecycle({ appMcp, app, argv: [] })
    native.emit({ type: 'idle-exit' })
    expect(app.quit).toHaveBeenCalledTimes(1)
    detach()
    detach()
    native.emit({ type: 'idle-exit' })
    app.emit('second-instance', {}, ['app-mcp-wake:x'])
    expect(app.quit).toHaveBeenCalledTimes(1)
    expect(native.lifecycleCalls).toEqual([])
    expect(app.listenerCount('second-instance')).toBe(0)

    const other = setup()
    attachLifecycle({ appMcp: other.appMcp, app: other.app, argv: [], quitOnIdleExit: false })
    other.native.emit({ type: 'idle-exit' })
    expect(other.app.quit).not.toHaveBeenCalled()
  })

  it('按窗口状态上报可见性', () => {
    const { appMcp, native, app } = setup()
    const win = new FakeWindow()
    const detach = attachLifecycle({ appMcp, app, argv: [], window: win })
    expect(native.visibility).toEqual(['visible', true])
    win.focused = false
    win.emit('blur')
    expect(native.visibility).toEqual(['visible', false])
    win.minimized = true
    win.emit('minimize')
    expect(native.visibility).toEqual(['hidden', false])
    win.emit('closed')
    expect(native.visibility).toEqual(['hidden', false])
    detach()
    expect(win.listenerCount('show')).toBe(0)
    expect(win.listenerCount('closed')).toBe(0)
  })
})
