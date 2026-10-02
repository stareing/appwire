import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { BroadcastChannelFactory } from '../src/instance-guard'
import { WakeHandoff, wakeChannelName } from '../src/wake-handoff'
import { channelHub, type Harness, settle, setup } from './fakes'

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
  document.body.innerHTML = ''
})

afterEach(() => {
  vi.useRealTimers()
})

function responder(factory: BroadcastChannelFactory, appId = 'shop', canClaim = true, accept = true) {
  const acceptToken = vi.fn((_token: string) => accept)
  const handoff = new WakeHandoff({ appId, createChannel: factory, canClaim: () => canClaim, acceptToken })
  return { handoff, acceptToken }
}

function offerer(factory: BroadcastChannelFactory, timeoutMs = 50): WakeHandoff {
  return new WakeHandoff({ appId: 'shop', createChannel: factory, canClaim: () => false, acceptToken: () => false, timeoutMs })
}

const grants = (posted: unknown[]) => posted.filter((m) => (m as { t?: string }).t === 'grant')

describe('WakeHandoff', () => {
  it('休眠中的标签页认领：令牌只在 grant 中出现一次，offer 返回 true', async () => {
    const { factory, posted } = channelHub()
    const old = responder(factory)
    const fresh = offerer(factory)
    expect(await fresh.offer('tok-1')).toBe(true)
    expect(old.acceptToken).toHaveBeenCalledExactlyOnceWith('tok-1')
    expect(posted.map((m) => (m as { t: string }).t)).toEqual(['offer', 'claim', 'grant', 'taken'])
    expect(posted.filter((m) => JSON.stringify(m).includes('tok-1'))).toHaveLength(1)
    old.handoff.dispose()
    fresh.dispose()
  })

  it('多个标签页认领：只交给第一个', async () => {
    const { factory, posted } = channelHub()
    const a = responder(factory)
    const b = responder(factory)
    const fresh = offerer(factory)
    expect(await fresh.offer('tok-2')).toBe(true)
    expect(a.acceptToken.mock.calls.length + b.acceptToken.mock.calls.length).toBe(1)
    expect(grants(posted)).toHaveLength(1)
    for (const h of [a.handoff, b.handoff, fresh]) h.dispose()
  })

  it('没有可认领的标签页（不在休眠 / 不同 appId）：超时返回 false，令牌不外发', async () => {
    const { factory, posted } = channelHub()
    const busy = responder(factory, 'shop', false)
    const other = responder(factory, 'other')
    const fresh = offerer(factory, 20)
    expect(await fresh.offer('tok-3')).toBe(false)
    expect(grants(posted)).toHaveLength(0)
    expect(busy.acceptToken).not.toHaveBeenCalled()
    expect(other.acceptToken).not.toHaveBeenCalled()
    for (const h of [busy.handoff, other.handoff, fresh]) h.dispose()
  })

  it('认领方的核心不接受令牌：不回 taken，超时返回 false（新标签页自己回连）', async () => {
    const { factory } = channelHub()
    const old = responder(factory, 'shop', true, false)
    const fresh = offerer(factory, 20)
    expect(await fresh.offer('tok-4')).toBe(false)
    expect(old.acceptToken).toHaveBeenCalledOnce()
    old.handoff.dispose()
    fresh.dispose()
  })

  it('不同 appId 的消息与通道互不相干；通道不可用时立即返回 false；dispose 结束进行中的 offer', async () => {
    expect(wakeChannelName('shop')).toBe('app-mcp:shop:wake')
    const broken = new WakeHandoff({
      appId: 'shop',
      createChannel: () => {
        throw new Error('no channel')
      },
      canClaim: () => true,
      acceptToken: () => true,
    })
    expect(await broken.offer('tok')).toBe(false)
    const { factory } = channelHub()
    const fresh = offerer(factory, 60_000)
    const pending = fresh.offer('tok-5')
    fresh.dispose()
    expect(await pending).toBe(false)
  })
})

// ---------------------------------------------------------------------------
// 驱动层：原标签页（休眠）+ 带令牌打开的新标签页
// ---------------------------------------------------------------------------

class FakeTabWindow extends EventTarget {
  closed = false
  closeCalls = 0
  location: { href: string }
  history = {
    state: null as unknown,
    replaceState: (_state: unknown, _title: string, url: string) => {
      this.location.href = url
    },
  }
  constructor(
    href: string,
    private readonly closable: boolean,
  ) {
    super()
    this.location = { href }
  }
  close(): void {
    this.closeCalls++
    if (this.closable) this.closed = true
  }
}

const WAKE_URL = 'https://shop.example/app#app-mcp-wake=tok-W'

async function dormantTab(factory: BroadcastChannelFactory): Promise<Harness> {
  const h = setup({ lifecycle: { mode: 'idle' } }, false, { createBroadcastChannel: factory, instanceProbeMs: 1 })
  await vi.waitFor(() => expect(h.core.methods()).toContain('start'))
  h.app.sleep()
  expect(h.app.state.status).toBe('dormant')
  return h
}

function wakeTab(factory: BroadcastChannelFactory, closable: boolean, wakeHandoffMs = 1000) {
  const win = new FakeTabWindow(WAKE_URL, closable)
  const h = setup({ lifecycle: { mode: 'idle' } }, false, {
    createBroadcastChannel: factory,
    instanceProbeMs: 1,
    wakeHandoffMs,
    window: win as unknown as Window,
  })
  return { h, win }
}

const notice = () => document.querySelector('[data-app-mcp-handoff]')

describe('驱动层唤醒交接', () => {
  it('已有休眠标签页：原标签页带令牌回连，新标签页移除令牌并关闭（不创建核心）', async () => {
    const { factory } = channelHub()
    const old = await dormantTab(factory)
    const { h, win } = wakeTab(factory, true)
    await vi.waitFor(() => expect(win.closeCalls).toBe(1))
    await settle()
    expect(old.core.callsOf('handleWake')).toEqual([['#app-mcp-wake=tok-W', expect.any(Number)]])
    expect(win.location.href).toBe('https://shop.example/app')
    expect(h.core.config).toBeUndefined()
    expect(h.core.methods()).not.toContain('start')
    expect(notice()).toBeNull()
    old.app.dispose()
    h.app.dispose()
  })

  it('新标签页关不掉：显示中文提示，按普通标签页启动且不再使用令牌', async () => {
    const { factory } = channelHub()
    const old = await dormantTab(factory)
    const { h, win } = wakeTab(factory, false)
    await vi.waitFor(() => expect(h.core.methods()).toContain('start'))
    expect(win.closeCalls).toBe(1)
    expect(notice()?.textContent).toContain('原来的标签页')
    expect(old.core.callsOf('handleWake')).toHaveLength(1)
    expect(h.core.callsOf('handleWake')).toEqual([])
    expect(win.location.href).toBe('https://shop.example/app')
    old.app.dispose()
    h.app.dispose()
  })

  it('没有休眠标签页（原标签页已连接）：超时后新标签页自己用令牌回连', async () => {
    const { factory } = channelHub()
    const old = setup({ lifecycle: { mode: 'idle' } }, false, { createBroadcastChannel: factory, instanceProbeMs: 1 })
    await vi.waitFor(() => expect(old.core.methods()).toContain('start'))
    old.socket().open()
    expect(old.app.state.status).toBe('connected')
    const { h, win } = wakeTab(factory, true, 20)
    await vi.waitFor(() => expect(h.core.methods()).toContain('start'))
    expect(h.core.callsOf('handleWake')).toEqual([[WAKE_URL, expect.any(Number)]])
    expect(old.core.callsOf('handleWake')).toEqual([])
    expect(win.closeCalls).toBe(0)
    old.app.dispose()
    h.app.dispose()
  })
})
