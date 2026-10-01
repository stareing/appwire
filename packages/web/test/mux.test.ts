import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { LeaderEndpoint, type LocksLike, portEndpoint } from '../src/mux/endpoints'
import { MuxLink, type OwnerEndpoint } from '../src/mux/link'
import { MuxOwner } from '../src/mux/owner'
import { decodeFrame, encodeMsg, MUX_REQUEST, parseMuxReply } from '../src/mux/protocol'
import type { BroadcastChannelLike } from '../src/instance-guard'
import { FakeSocket } from './fakes'

const URL_A = 'ws://127.0.0.1:7717'

/** 标签页 ↔ 持有方的内存端点（同步投递，消息经结构化克隆）。 */
function memoryEndpoint(owner: MuxOwner): OwnerEndpoint & { detach(): void } {
  const ep: OwnerEndpoint & { detach(): void } = {
    onmessage: null,
    onreset: null,
    post: (m) => handle.receive(structuredClone(m)),
    close: () => {},
    detach: () => handle.detach(),
  }
  const handle = owner.attach((m) => ep.onmessage?.(structuredClone(m)))
  return ep
}

function setupOwner(): { owner: MuxOwner; sockets: FakeSocket[]; csp: Set<string> } {
  const sockets: FakeSocket[] = []
  const csp = new Set<string>()
  const owner = new MuxOwner({
    createWebSocket: (url) => {
      const s = new FakeSocket(url)
      sockets.push(s)
      return s
    },
    cspBlocked: (url) => csp.has(url),
  })
  return { owner, sockets, csp }
}

/** 完成 app/mux 协商。 */
function negotiate(sock: FakeSocket, ok = true): void {
  sock.open()
  expect(sock.sent[0]).toBe(MUX_REQUEST)
  sock.receive(
    ok
      ? JSON.stringify({ jsonrpc: '2.0', id: 'mux', result: { version: 1, maxChannels: 64 } })
      : JSON.stringify({ jsonrpc: '2.0', id: 'mux', error: { code: -32014, message: '尚未完成握手' } }),
  )
}

function frames(sock: FakeSocket): unknown[] {
  return sock.sent.slice(1).map((t) => JSON.parse(t))
}

describe('Host 帧与协商', () => {
  it('编码、解码与协商响应', () => {
    expect(encodeMsg(3, '{"a":1}')).toBe('{"type":"msg","ch":3,"msg":{"a":1}}')
    expect(decodeFrame('{"type":"msg","ch":3,"msg":{"a":1}}')).toEqual({ type: 'msg', ch: 3, text: '{"a":1}' })
    expect(decodeFrame('{"type":"close","ch":2,"reason":"x"}')).toEqual({ type: 'close', ch: 2, reason: 'x' })
    expect(decodeFrame('{"type":"msg","ch":0,"msg":{}}')).toBeUndefined()
    expect(decodeFrame('nope')).toBeUndefined()
    expect(parseMuxReply('{"jsonrpc":"2.0","id":"mux","result":{"version":1}}')).toBe(true)
    expect(parseMuxReply('{"jsonrpc":"2.0","id":"mux","error":{"code":-32014}}')).toBe(false)
    expect(parseMuxReply('{"jsonrpc":"2.0","id":7,"result":{}}')).toBeUndefined()
  })
})

describe('MuxOwner + MuxLink', () => {
  it('多个标签页共用一条连接，按通道路由', () => {
    const { owner, sockets } = setupOwner()
    const tabA = new MuxLink(memoryEndpoint(owner))
    const tabB = new MuxLink(memoryEndpoint(owner))
    const a = tabA.open(URL_A)
    const b = tabB.open(URL_A)
    expect(sockets).toHaveLength(1)
    const sock = sockets[0]!
    const opened = vi.fn()
    a.onopen = opened
    b.onopen = opened
    negotiate(sock)
    expect(opened).toHaveBeenCalledTimes(2)
    expect(a.readyState).toBe(1)
    expect(frames(sock)).toEqual([
      { type: 'open', ch: 1 },
      { type: 'open', ch: 2 },
    ])

    // 标签页 → Host
    a.send('{"jsonrpc":"2.0","id":1,"method":"app/hello","params":{}}')
    b.send('{"jsonrpc":"2.0","method":"app/ready","params":{}}')
    expect(frames(sock).slice(2)).toEqual([
      { type: 'msg', ch: 1, msg: { jsonrpc: '2.0', id: 1, method: 'app/hello', params: {} } },
      { type: 'msg', ch: 2, msg: { jsonrpc: '2.0', method: 'app/ready', params: {} } },
    ])

    // Host → 标签页
    const gotA: unknown[] = []
    const gotB: unknown[] = []
    a.onmessage = (e) => gotA.push(e.data)
    b.onmessage = (e) => gotB.push(e.data)
    sock.receive('{"type":"msg","ch":2,"msg":{"jsonrpc":"2.0","id":5,"method":"ping"}}')
    expect(gotA).toEqual([])
    expect(gotB).toEqual(['{"jsonrpc":"2.0","id":5,"method":"ping"}'])

    // Host 关闭通道 1
    const closedA = vi.fn()
    a.onclose = closedA
    sock.receive('{"type":"close","ch":1}')
    expect(closedA).toHaveBeenCalledOnce()
    expect(a.readyState).toBe(3)

    // 标签页关闭通道 2：最后一个通道关闭后连接随之关闭
    b.close()
    expect(frames(sock).at(-1)).toEqual({ type: 'close', ch: 2 })
    expect(sock.closed).toBe(true)
    expect(owner.connectionCount).toBe(0)

    // 再次打开：新连接
    tabA.open(URL_A)
    expect(sockets).toHaveLength(2)
  })

  it('旧 Host（app/mux 返回错误）：通道以 unsupported 关闭，之后的打开直接拒绝', () => {
    const { owner, sockets } = setupOwner()
    const link = new MuxLink(memoryEndpoint(owner))
    const ch = link.open(URL_A)
    const onclose = vi.fn()
    ch.onclose = onclose
    negotiate(sockets[0]!, false)
    expect(onclose).toHaveBeenCalledOnce()
    expect(ch.failure).toEqual({ unsupported: true })
    expect(sockets[0]!.closed).toBe(true)
    const again = link.open(URL_A)
    expect(sockets).toHaveLength(1)
    expect(again.failure).toEqual({ unsupported: true })
  })

  it('连接断开时所有通道收到断开；打开前失败时报告 CSP 拦截', async () => {
    const { owner, sockets, csp } = setupOwner()
    const link = new MuxLink(memoryEndpoint(owner))
    const a = link.open(URL_A)
    const b = link.open(URL_A)
    negotiate(sockets[0]!)
    const closed = vi.fn()
    a.onclose = closed
    b.onclose = closed
    sockets[0]!.fail()
    expect(closed).toHaveBeenCalledTimes(2)
    expect(a.failure).toEqual({})

    csp.add(URL_A)
    const c = link.open(URL_A)
    sockets[1]!.fail()
    // CSP 违规事件可能晚于失败事件：下一轮任务再判断
    expect(c.readyState).toBe(0)
    await new Promise((r) => setTimeout(r, 0))
    expect(c.failure).toEqual({ csp: true })
  })

  it('bfcache：暂存期间的消息在恢复后投递', () => {
    const { owner, sockets } = setupOwner()
    const link = new MuxLink(memoryEndpoint(owner))
    const a = link.open(URL_A)
    negotiate(sockets[0]!)
    const got: unknown[] = []
    a.onmessage = (e) => got.push(e.data)
    link.park()
    sockets[0]!.receive('{"type":"msg","ch":1,"msg":{"jsonrpc":"2.0","id":1,"result":{"accepted":true}}}')
    expect(got).toEqual([])
    link.unpark()
    expect(got).toEqual(['{"jsonrpc":"2.0","id":1,"result":{"accepted":true}}'])
  })

  it('标签页离开（bye）关闭它的全部通道，不影响其他标签页', () => {
    const { owner, sockets } = setupOwner()
    const tabA = new MuxLink(memoryEndpoint(owner))
    const tabB = new MuxLink(memoryEndpoint(owner))
    tabA.open(URL_A)
    tabA.open(URL_A)
    const b = tabB.open(URL_A)
    negotiate(sockets[0]!)
    tabA.dispose()
    expect(frames(sockets[0]!).slice(3)).toEqual([
      { type: 'close', ch: 1 },
      { type: 'close', ch: 2 },
    ])
    expect(b.readyState).toBe(1)
    expect(sockets[0]!.closed).toBe(false)
  })

  describe('打开超时', () => {
    beforeEach(() => {
      vi.useFakeTimers()
    })
    afterEach(() => {
      vi.useRealTimers()
    })
    it('持有方无应答时按连接失败处理并通知端点', () => {
      const stalled = vi.fn()
      const ep: OwnerEndpoint = { onmessage: null, onreset: null, post: () => {}, close: () => {}, stalled }
      const link = new MuxLink(ep, 1000)
      const ch = link.open(URL_A)
      const onclose = vi.fn()
      ch.onclose = onclose
      vi.advanceTimersByTime(1000)
      expect(onclose).toHaveBeenCalledOnce()
      expect(stalled).toHaveBeenCalledOnce()
    })
  })
})

describe('portEndpoint', () => {
  it('SharedWorker 无法启动时以 unsupported 关闭通道', async () => {
    const posted: unknown[] = []
    const port = { postMessage: (m: unknown) => posted.push(m), onmessage: null, start: vi.fn(), close: vi.fn() }
    const worker = { port, onerror: null as ((ev: unknown) => void) | null }
    const link = new MuxLink(portEndpoint(worker))
    expect(port.start).toHaveBeenCalled()
    const a = link.open(URL_A)
    expect(posted).toEqual([{ t: 'open', ch: 1, url: URL_A }])
    worker.onerror?.({})
    expect(a.failure).toEqual({ unsupported: true, reason: 'SharedWorker 无法启动' })
    const b = link.open(URL_A)
    await Promise.resolve()
    expect(b.failure?.unsupported).toBe(true)
  })
})

// ---------------------------------------------------------------------------
// 选主
// ---------------------------------------------------------------------------

/** 内存 BroadcastChannel：同名通道之间异步（微任务）投递，不投给自己。 */
class Bus {
  private readonly members = new Set<BroadcastChannelLike & { name: string }>()
  create = (name: string): BroadcastChannelLike => {
    const ch = {
      name,
      onmessage: null as ((ev: { data: unknown }) => void) | null,
      postMessage: (m: unknown) => {
        for (const other of this.members) {
          if (other !== ch && other.name === name) {
            const data = structuredClone(m)
            queueMicrotask(() => other.onmessage?.({ data }))
          }
        }
      },
      close: () => {
        this.members.delete(ch)
      },
    }
    this.members.add(ch)
    return ch
  }
}

/** 内存 Web Locks：ifAvailable 语义。 */
class Locks implements LocksLike {
  held = new Set<string>()
  async request(name: string, _o: { ifAvailable: boolean }, cb: (lock: unknown) => unknown): Promise<unknown> {
    if (this.held.has(name)) return cb(null)
    this.held.add(name)
    try {
      return await cb({ name })
    } finally {
      this.held.delete(name)
    }
  }
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

describe('LeaderEndpoint（无 SharedWorker 时选主）', () => {
  it('第一个标签页成为主标签页，其他标签页经它共用连接；主标签页让位后重新选主', async () => {
    const bus = new Bus()
    const locks = new Locks()
    const sockets: FakeSocket[] = []
    const createOwner = () =>
      new MuxOwner({
        createWebSocket: (url) => {
          const s = new FakeSocket(url)
          sockets.push(s)
          return s
        },
      })
    const opts = { name: 'app-mcp:mux', createChannel: bus.create, locks, createOwner, retryMs: 5 }
    const epA = new LeaderEndpoint(opts)
    const epB = new LeaderEndpoint(opts)
    const tabA = new MuxLink(epA)
    const tabB = new MuxLink(epB)

    const a = tabA.open(URL_A)
    await flush()
    expect(epA.isLeader).toBe(true)
    const b = tabB.open(URL_A)
    await flush()
    expect(epB.isLeader).toBe(false)
    expect(sockets).toHaveLength(1)
    negotiate(sockets[0]!)
    await flush()
    expect(a.readyState).toBe(1)
    expect(b.readyState).toBe(1)

    // 跟随者的消息经主标签页发出
    b.send('{"jsonrpc":"2.0","method":"app/ready","params":{}}')
    await flush()
    expect(frames(sockets[0]!).at(-1)).toEqual({ type: 'msg', ch: 2, msg: { jsonrpc: '2.0', method: 'app/ready', params: {} } })
    const got: unknown[] = []
    b.onmessage = (e) => got.push(e.data)
    sockets[0]!.receive('{"type":"msg","ch":2,"msg":{"jsonrpc":"2.0","id":9,"method":"ping"}}')
    await flush()
    expect(got).toEqual(['{"jsonrpc":"2.0","id":9,"method":"ping"}'])

    // 主标签页让位（pagehide / freeze）：所有通道断开，锁释放
    const closedB = vi.fn()
    b.onclose = closedB
    epA.abdicate()
    await flush()
    expect(closedB).toHaveBeenCalled()
    expect(sockets[0]!.closed).toBe(true)
    expect(locks.held.size).toBe(0)

    // 跟随者重新打开：自己成为主标签页
    tabB.open(URL_A)
    await flush()
    expect(epB.isLeader).toBe(true)
    expect(sockets).toHaveLength(2)
    epB.close()
    epA.close()
  })

  it('pagehide 时主标签页让位', async () => {
    const bus = new Bus()
    const locks = new Locks()
    const win = new EventTarget()
    const ep = new LeaderEndpoint({
      name: 'n',
      createChannel: bus.create,
      locks,
      createOwner: () => new MuxOwner({ createWebSocket: (url) => new FakeSocket(url) }),
      win,
    })
    new MuxLink(ep).open(URL_A)
    await flush()
    expect(ep.isLeader).toBe(true)
    win.dispatchEvent(new Event('pagehide'))
    await flush()
    expect(ep.isLeader).toBe(false)
    expect(locks.held.size).toBe(0)
  })
})
