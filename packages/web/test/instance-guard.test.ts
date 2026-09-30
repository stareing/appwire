import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AppMcpDriver } from '../src/driver'
import {
  type BroadcastChannelFactory,
  type BroadcastChannelLike,
  globalBroadcastChannel,
  InstanceGuard,
  instanceChannelName,
} from '../src/instance-guard'
import { instanceIdKey } from '../src/storage'
import { FakeCore, silentLogger } from './fakes'

/** 内存中的 BroadcastChannel：同名通道之间异步投递（不投递给发送者自己）。 */
function channelHub(): { factory: BroadcastChannelFactory; posted: unknown[] } {
  const channels = new Map<string, Set<BroadcastChannelLike>>()
  const posted: unknown[] = []
  const factory: BroadcastChannelFactory = (name) => {
    const set = channels.get(name) ?? new Set()
    channels.set(name, set)
    const ch: BroadcastChannelLike = {
      onmessage: null,
      postMessage(message) {
        posted.push(message)
        const data = structuredClone(message)
        for (const other of set) {
          if (other !== ch) queueMicrotask(() => other.onmessage?.({ data }))
        }
      },
      close() {
        set.delete(ch)
      },
    }
    set.add(ch)
    return ch
  }
  return { factory, posted }
}

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('InstanceGuard', () => {
  it('没有冲突时保持原 ID', async () => {
    const { factory, posted } = channelHub()
    const guard = new InstanceGuard({ appId: 'shop', instanceId: 'a', createChannel: factory, windowMs: 5 })
    expect(guard.active).toBe(true)
    expect(await guard.probe()).toBe('a')
    expect(posted).toEqual([{ t: 'probe', id: 'a', from: expect.any(String) }])
    guard.dispose()
  })

  it('复制的标签页（后探测的一方）重新生成 ID 并写回 sessionStorage，原标签页不变', async () => {
    const { factory } = channelHub()
    const original = new InstanceGuard({ appId: 'shop', instanceId: 'dup', createChannel: factory, windowMs: 5 })
    await original.probe()
    const onRegenerated = vi.fn()
    const copy = new InstanceGuard({
      appId: 'shop',
      instanceId: 'dup',
      createChannel: factory,
      windowMs: 20,
      onRegenerated,
    })
    const id = await copy.probe()
    expect(id).not.toBe('dup')
    expect(copy.instanceId).toBe(id)
    expect(original.instanceId).toBe('dup')
    expect(sessionStorage.getItem(instanceIdKey('shop'))).toBe(id)
    expect(onRegenerated).toHaveBeenCalledWith('dup', id)
    original.dispose()
    copy.dispose()
  })

  it('不同 appId 的通道互不影响；dispose 后不再回复', async () => {
    const { factory } = channelHub()
    const other = new InstanceGuard({ appId: 'other', instanceId: 'x', createChannel: factory, windowMs: 5 })
    const gone = new InstanceGuard({ appId: 'shop', instanceId: 'x', createChannel: factory, windowMs: 5 })
    gone.dispose()
    const guard = new InstanceGuard({ appId: 'shop', instanceId: 'x', createChannel: factory, windowMs: 10 })
    expect(await guard.probe()).toBe('x')
    for (const g of [other, guard]) g.dispose()
  })

  it('窗口结束后才收到的冲突只回调 onLateConflict；重复 probe 不再广播', async () => {
    const { factory } = channelHub()
    const peer = factory(instanceChannelName('shop'))
    const received: { t: string; from?: string; to?: string }[] = []
    peer.onmessage = (e) => received.push(e.data as never)
    const onLateConflict = vi.fn()
    const late = new InstanceGuard({ appId: 'shop', instanceId: 'dup', createChannel: factory, windowMs: 0, onLateConflict })
    await late.probe()
    const nonce = received[0]?.from
    expect(received).toEqual([{ t: 'probe', id: 'dup', from: nonce }])
    // 锁定后仍回复其他标签页的探测
    peer.postMessage({ t: 'probe', id: 'dup', from: 'peer' })
    await Promise.resolve()
    await Promise.resolve()
    expect(received[1]).toEqual({ t: 'taken', id: 'dup', to: 'peer' })
    // 迟到的 taken
    peer.postMessage({ t: 'taken', id: 'dup', to: nonce })
    await Promise.resolve()
    expect(onLateConflict).toHaveBeenCalledWith('dup')
    expect(late.instanceId).toBe('dup')
    await late.probe()
    expect(received).toHaveLength(2)
    late.dispose()
    peer.close()
  })

  it('没有 BroadcastChannel 或构造失败时退化为不检测', async () => {
    const guard = new InstanceGuard({ appId: 'shop', instanceId: 'a' })
    expect(guard.active).toBe(false)
    expect(await guard.probe()).toBe('a')
    const broken = new InstanceGuard({
      appId: 'shop',
      instanceId: 'b',
      createChannel: () => {
        throw new Error('SecurityError')
      },
    })
    expect(broken.active).toBe(false)
    vi.stubGlobal('BroadcastChannel', undefined)
    expect(globalBroadcastChannel()).toBeUndefined()
    vi.unstubAllGlobals()
  })

  it('使用环境自带的 BroadcastChannel', async () => {
    const factory = globalBroadcastChannel()
    if (!factory) return
    const a = new InstanceGuard({ appId: 'real', instanceId: 'same', createChannel: factory, windowMs: 1 })
    await a.probe()
    const b = new InstanceGuard({ appId: 'real', instanceId: 'same', createChannel: factory, windowMs: 100 })
    expect(await b.probe()).not.toBe('same')
    a.dispose()
    b.dispose()
  })

  it('忽略格式不对的消息', async () => {
    const { factory } = channelHub()
    const guard = new InstanceGuard({ appId: 'shop', instanceId: 'a', createChannel: factory, windowMs: 5 })
    const peer = factory(instanceChannelName('shop'))
    const replies: unknown[] = []
    peer.onmessage = (e) => replies.push(e.data)
    for (const m of [null, 'x', { t: 'probe', id: 'a' }, { t: 'other', id: 'a', from: 'p' }]) peer.postMessage(m)
    await Promise.resolve()
    expect(replies).toEqual([])
    guard.dispose()
    peer.close()
  })
})

describe('驱动层：复制标签页', () => {
  function driver(factory: BroadcastChannelFactory, loadDelay = 0) {
    const core = new FakeCore()
    const app = new AppMcpDriver(
      { appId: 'shop', appName: 'x', logger: silentLogger() },
      {
        loadCore: async () => {
          if (loadDelay) await new Promise((r) => setTimeout(r, loadDelay))
          return (config) => {
            core.config = config
            return core
          }
        },
        createWebSocket: () => ({
          readyState: 0,
          send() {},
          close() {},
          onopen: null,
          onmessage: null,
          onclose: null,
          onerror: null,
        }),
        createBroadcastChannel: factory,
        instanceProbeMs: 20,
      },
    )
    return { app, core }
  }

  it('第二个标签页在核心创建前换成新的 instanceId', async () => {
    const { factory } = channelHub()
    sessionStorage.setItem(instanceIdKey('shop'), 'copied-id')
    const a = driver(factory)
    await vi.waitFor(() => expect(a.core.config).toBeDefined())
    expect(a.core.config?.instanceId).toBe('copied-id')

    const b = driver(factory)
    expect(b.app.instanceId).toBe('copied-id')
    expect(b.core.config).toBeUndefined()
    await vi.waitFor(() => expect(b.core.config).toBeDefined())
    expect(b.core.config?.instanceId).not.toBe('copied-id')
    expect(b.app.instanceId).toBe(b.core.config?.instanceId)
    expect(a.app.instanceId).toBe('copied-id')
    a.app.dispose()
    b.app.dispose()
  })

  it('探测窗口与 WASM 加载并行（加载更慢时不额外等待）', async () => {
    const { factory } = channelHub()
    const start = Date.now()
    const a = driver(factory, 40)
    await vi.waitFor(() => expect(a.core.config).toBeDefined())
    expect(Date.now() - start).toBeLessThan(200)
    a.app.dispose()
  })
})
