/**
 * 用户正在操作（spec/protocol.md 5.3）：`busyPolicy` 配置、`setBusy` / `isBusy` / `setBusyPolicy` 与核心的同步，
 * 以及桥接页面的 `busy.set`。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BRIDGE_VERSION, createBridgeAppMcp, type AppMcpBridge, type OpReply, type RendererOp } from '../src/electron-bridge'
import { createAppMcp } from '../src/index'
import type { BusyPolicy } from '../src/types'
import { connected, setup, silentLogger } from './fakes'

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('驱动层（WASM 核心）', () => {
  it('busyPolicy 选项传给核心配置；缺省不带（核心缺省 reject）', async () => {
    const plain = await connected()
    expect(plain.core.config).not.toHaveProperty('busyPolicy')
    plain.app.dispose()
    const h = await connected({ busyPolicy: 'queue' })
    expect(h.core.config).toMatchObject({ busyPolicy: 'queue' })
    h.app.dispose()
  })

  it('核心加载前 setBusy / setBusyPolicy：记录状态，加载时在启动前同步给核心', async () => {
    const h = setup({}, true)
    h.app.setBusy(true)
    h.app.setBusyPolicy('queue')
    expect(h.app.isBusy()).toBe(true)
    expect(h.core.calls).toEqual([])
    await h.load()
    expect(h.core.config).toMatchObject({ busyPolicy: 'queue' })
    const methods = h.core.methods()
    expect(methods.indexOf('setBusy')).toBeGreaterThanOrEqual(0)
    expect(methods.indexOf('setBusy')).toBeLessThan(methods.indexOf('start'))
    expect(h.core.callsOf('setBusy')).toEqual([[true]])
    h.app.dispose()
  })

  it('加载前声明又撤销：不调用核心', async () => {
    const h = setup({}, true)
    h.app.setBusy(true)
    h.app.setBusy(false)
    await h.load()
    expect(h.core.callsOf('setBusy')).toEqual([])
    h.app.dispose()
  })

  it('加载后 setBusy 转给核心，值不变时不重复调用；isBusy 反映当前声明', async () => {
    const h = await connected()
    expect(h.app.isBusy()).toBe(false)
    h.app.setBusy(true)
    h.app.setBusy(true)
    expect(h.app.isBusy()).toBe(true)
    h.app.setBusy(false)
    expect(h.app.isBusy()).toBe(false)
    expect(h.core.callsOf('setBusy')).toEqual([[true], [false]])
    h.app.dispose()
  })

  it('作用域（beginBusy）可嵌套、按引用计数；只在有效值变化时调用核心', async () => {
    const h = await connected()
    const outer = h.app.beginBusy()
    const inner = h.app.beginBusy()
    expect(h.app.isBusy()).toBe(true)
    outer.release()
    outer.release()
    expect(h.app.isBusy()).toBe(true)
    inner.release()
    expect(h.app.isBusy()).toBe(false)
    expect(h.core.callsOf('setBusy')).toEqual([[true], [false]])
    h.app.dispose()
  })

  it('显式开关与作用域互不清除：setBusy(false) 不结束作用域，作用域结束不清除显式开关', async () => {
    const h = await connected()
    const scope = h.app.beginBusy()
    h.app.setBusy(true)
    h.app.setBusy(false)
    expect(h.app.isBusy()).toBe(true)
    h.app.setBusy(true)
    scope.release()
    expect(h.app.isBusy()).toBe(true)
    h.app.setBusy(false)
    expect(h.app.isBusy()).toBe(false)
    expect(h.core.callsOf('setBusy')).toEqual([[true], [false]])
    h.app.dispose()
  })

  it('核心加载前开始的作用域：加载时同步', async () => {
    const h = setup({}, true)
    const scope = h.app.beginBusy()
    await h.load()
    expect(h.core.callsOf('setBusy')).toEqual([[true]])
    scope.release()
    expect(h.core.callsOf('setBusy')).toEqual([[true], [false]])
    h.app.dispose()
  })

  it('setBusy 之后立即取出核心产生的消息并发出（排队调用被拒绝 / 开始执行的回复不等下一次输入）', async () => {
    const h = await connected()
    const original = h.core.setBusy.bind(h.core)
    h.core.setBusy = (busy: boolean) => {
      original(busy)
      h.core.emit({ type: 'send', text: `busy=${busy}` })
    }
    h.app.setBusy(true)
    expect(h.socket().sent).toContain('busy=true')
    h.app.setBusy(false)
    expect(h.socket().sent).toContain('busy=false')
    h.app.dispose()
  })

  it('setBusyPolicy 转给核心；非法值抛错且不转发', async () => {
    const h = await connected()
    h.app.setBusyPolicy('queue')
    expect(() => h.app.setBusyPolicy('drop' as BusyPolicy)).toThrow(/busyPolicy/)
    h.app.setBusyPolicy('reject')
    expect(h.core.callsOf('setBusyPolicy')).toEqual([['queue'], ['reject']])
    h.app.dispose()
  })

  it('dispose 后为空操作', async () => {
    const h = await connected()
    h.app.dispose()
    h.app.setBusy(true)
    h.app.setBusyPolicy('queue')
    expect(h.core.callsOf('setBusy')).toEqual([])
    expect(h.core.callsOf('setBusyPolicy')).toEqual([])
  })

  it('enabled: false：空操作，isBusy 恒为 false', () => {
    const app = createAppMcp({ appId: 'shop', appName: '示例商城', enabled: false })
    app.beginBusy().release()
    app.beginBusy()
    app.setBusy(true)
    app.setBusyPolicy?.('queue')
    expect(app.isBusy()).toBe(false)
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
    return { app, ops, logger, busyOps: () => ops.filter((o) => o.op === 'busy.set') }
  }

  const settle = async () => {
    for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
  }

  it('setBusy 发送 busy.set（只在本页声明变化时）；isBusy 为本页声明；不提供 setBusyPolicy', async () => {
    const { app, busyOps } = page()
    expect(app.isBusy()).toBe(false)
    app.setBusy(true)
    app.setBusy(true)
    expect(app.isBusy()).toBe(true)
    app.setBusy(false)
    await settle()
    expect(busyOps()).toEqual([
      { op: 'busy.set', busy: true },
      { op: 'busy.set', busy: false },
    ])
    expect(app.setBusyPolicy).toBeUndefined()
    app.dispose()
  })

  it('本页有效值 = 显式开关 OR 作用域：只在有效值变化时发送 busy.set', async () => {
    const { app, busyOps } = page()
    const scope = app.beginBusy()
    app.setBusy(true)
    app.setBusy(false)
    expect(app.isBusy()).toBe(true)
    const nested = app.beginBusy()
    scope.release()
    nested.release()
    expect(app.isBusy()).toBe(false)
    await settle()
    expect(busyOps()).toEqual([
      { op: 'busy.set', busy: true },
      { op: 'busy.set', busy: false },
    ])
    app.dispose()
  })

  it('主进程不支持（旧版回复错误）：记警告', async () => {
    const { app, logger } = page((op) => (op.op === 'busy.set' ? { ok: false, message: '未知的操作 "busy.set"' } : { ok: true }))
    app.setBusy(true)
    await settle()
    expect(logger.warn).toHaveBeenCalledWith(expect.stringContaining('busy.set'))
    app.dispose()
  })

  it('dispose 后不再发送（主进程随 reset 清除本页声明）', async () => {
    const { app, busyOps } = page()
    app.dispose()
    app.setBusy(true)
    await settle()
    expect(busyOps()).toEqual([])
  })
})
