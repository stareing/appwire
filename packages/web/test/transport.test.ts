/** 驱动层的传输选择：共享连接（多路复用）、旧 Host 回到直接连接、浏览器拦截（LNA / CSP）诊断。 */

import { describe, expect, it, vi } from 'vitest'
import { AppMcpDriver, type DriverDeps } from '../src/driver'
import { MuxLink, type OwnerEndpoint } from '../src/mux/link'
import { MuxOwner } from '../src/mux/owner'
import { MUX_REQUEST } from '../src/mux/protocol'
import { addressSpaceOf, NetworkGuard, type PermissionsLike } from '../src/network-guard'
import type { ConnectionState } from '../src/types'
import { FakeCore, FakeSocket, settle, silentLogger } from './fakes'

class FakeDoc extends EventTarget {
  visibilityState: 'visible' | 'hidden' = 'visible'
  hasFocus(): boolean {
    return true
  }
}

class FakeWin extends EventTarget {
  location: { href: string }
  history = { state: null as unknown, replaceState: () => {} }
  constructor(href: string) {
    super()
    this.location = { href }
  }
  pageTransition(type: 'pagehide' | 'pageshow', persisted: boolean): void {
    this.dispatchEvent(Object.assign(new Event(type), { persisted }))
  }
}

class FakePermission extends EventTarget {
  constructor(public state: string) {
    super()
  }
  set(state: string): void {
    this.state = state
    this.dispatchEvent(new Event('change'))
  }
}

/** 只认识给定权限名的 `navigator.permissions`（其他名称抛 TypeError，与浏览器一致）。 */
function permissions(known: Record<string, FakePermission>): PermissionsLike & { asked: string[] } {
  const asked: string[] = []
  return {
    asked,
    async query({ name }) {
      asked.push(name)
      const p = known[name]
      if (!p) throw new TypeError(`unknown permission ${name}`)
      return p
    },
  }
}

interface Tab {
  app: AppMcpDriver
  core: FakeCore
  direct: FakeSocket[]
  states: ConnectionState[]
  win: FakeWin
  doc: FakeDoc
}

async function tab(deps: Partial<DriverDeps> = {}, href = 'http://localhost:5173/'): Promise<Tab> {
  const core = new FakeCore()
  const direct: FakeSocket[] = []
  const win = new FakeWin(href)
  const doc = new FakeDoc()
  const app = new AppMcpDriver(
    { appId: 'shop', appName: '示例商城', logger: silentLogger() },
    {
      loadCore: async () => (config) => {
        core.config = config
        return core
      },
      createWebSocket: (url) => {
        const s = new FakeSocket(url)
        direct.push(s)
        return s
      },
      window: win as unknown as Window,
      document: doc as unknown as Document,
      permissions: permissions({}),
      ...deps,
    },
  )
  const states: ConnectionState[] = []
  app.onStateChange((s) => states.push(s))
  await settle()
  await new Promise((r) => setTimeout(r, 0))
  return { app, core, direct, states, win, doc }
}

/** 一个持有方 + 可以接入多个标签页的共享连接工厂。 */
function sharedOwner(): { owner: MuxOwner; hostSockets: FakeSocket[]; createSharedLink: () => MuxLink; posted: unknown[] } {
  const hostSockets: FakeSocket[] = []
  const posted: unknown[] = []
  const owner = new MuxOwner({
    createWebSocket: (url) => {
      const s = new FakeSocket(url)
      hostSockets.push(s)
      return s
    },
  })
  const createSharedLink = (): MuxLink => {
    const ep: OwnerEndpoint = {
      onmessage: null,
      onreset: null,
      post: (m) => {
        posted.push(m)
        handle.receive(structuredClone(m))
      },
      close: () => {},
    }
    const handle = owner.attach((m) => ep.onmessage?.(structuredClone(m)))
    return new MuxLink(ep)
  }
  return { owner, hostSockets, createSharedLink, posted }
}

function negotiate(sock: FakeSocket, ok = true): void {
  sock.open()
  expect(sock.sent[0]).toBe(MUX_REQUEST)
  sock.receive(
    JSON.stringify(
      ok ? { jsonrpc: '2.0', id: 'mux', result: { version: 1, maxChannels: 64 } } : { jsonrpc: '2.0', id: 'mux', error: { code: -32014, message: 'x' } },
    ),
  )
}

describe('共享连接', () => {
  it('两个标签页（两个 SDK 实例）共用一条 WebSocket，各自是独立通道', async () => {
    const shared = sharedOwner()
    const a = await tab({ createSharedLink: shared.createSharedLink })
    const b = await tab({ createSharedLink: shared.createSharedLink })
    expect(a.direct).toHaveLength(0)
    expect(b.direct).toHaveLength(0)
    expect(shared.hostSockets).toHaveLength(1)
    const sock = shared.hostSockets[0]!
    negotiate(sock)
    expect(a.core.methods()).toContain('handleConnected')
    expect(b.core.methods()).toContain('handleConnected')
    expect(a.app.state.status).toBe('connected')

    // 核心发出的消息装帧到各自的通道
    a.core.emit({ type: 'send', text: '{"jsonrpc":"2.0","method":"app/ready","params":{}}' })
    a.app.wake() // 任意输入触发 pump
    expect(JSON.parse(sock.sent.at(-1)!)).toEqual({ type: 'msg', ch: 1, msg: { jsonrpc: '2.0', method: 'app/ready', params: {} } })

    // Host 发给通道 2 的消息只进 b 的核心
    sock.receive('{"type":"msg","ch":2,"msg":{"jsonrpc":"2.0","id":3,"method":"ping"}}')
    expect(b.core.callsOf('handleMessage')).toEqual([['{"jsonrpc":"2.0","id":3,"method":"ping"}', expect.any(Number)]])
    expect(a.core.callsOf('handleMessage')).toEqual([])

    // Host 关闭通道 1：a 断开（核心按断开处理），b 不受影响
    sock.receive('{"type":"close","ch":1}')
    expect(a.core.methods()).toContain('handleDisconnected')
    expect(b.core.methods()).not.toContain('handleDisconnected')

    // b dispose：关闭通道 2，连接随最后一个通道关闭
    b.app.dispose()
    expect(sock.closed).toBe(true)
  })

  it('旧 Host 不支持多路复用：不通知核心断开，直接改为本页直连', async () => {
    const shared = sharedOwner()
    const a = await tab({ createSharedLink: shared.createSharedLink })
    negotiate(shared.hostSockets[0]!, false)
    expect(a.direct).toHaveLength(1)
    expect(a.core.methods()).not.toContain('handleDisconnected')
    a.direct[0]!.open()
    expect(a.app.state.status).toBe('connected')
    // 之后断线重连也直接连接
    a.direct[0]!.fail()
    a.core.emit({ type: 'connect' })
    a.app.wake()
    expect(a.direct).toHaveLength(2)
    expect(shared.hostSockets).toHaveLength(1)
  })

  it('bfcache：先发出休眠请求，再请持有方暂存；恢复时取回并回连', async () => {
    const shared = sharedOwner()
    const a = await tab({ createSharedLink: shared.createSharedLink })
    negotiate(shared.hostSockets[0]!)
    a.win.pageTransition('pagehide', true)
    const kinds = shared.posted.map((m) => (m as { t: string }).t)
    expect(a.core.callsOf('sleepWithReason')).toEqual([['background', expect.any(Number)]])
    expect(kinds.at(-1)).toBe('park')
    a.win.pageTransition('pageshow', true)
    expect(shared.posted.map((m) => (m as { t: string }).t)).toContain('unpark')
    expect(a.core.callsOf('wakeWithReason')).toEqual([['visible', expect.any(Number)]])
  })

  it('页面卸载（pagehide 非 bfcache）：关闭本页全部通道', async () => {
    const shared = sharedOwner()
    const a = await tab({ createSharedLink: shared.createSharedLink })
    negotiate(shared.hostSockets[0]!)
    a.win.pageTransition('pagehide', false)
    expect(shared.posted.at(-1)).toEqual({ t: 'bye' })
    expect(shared.hostSockets[0]!.closed).toBe(true)
  })
})

describe('浏览器拦截诊断', () => {
  it('地址空间', () => {
    expect(addressSpaceOf('localhost')).toBe('loopback')
    expect(addressSpaceOf('app.localhost')).toBe('loopback')
    expect(addressSpaceOf('127.0.0.1')).toBe('loopback')
    expect(addressSpaceOf('[::1]')).toBe('loopback')
    expect(addressSpaceOf('192.168.1.5')).toBe('local')
    expect(addressSpaceOf('172.20.0.1')).toBe('local')
    expect(addressSpaceOf('10.0.0.1')).toBe('local')
    expect(addressSpaceOf('8.8.8.8')).toBe('public')
    expect(addressSpaceOf('shop.example')).toBe('public')
  })

  it('LNA 只适用于更"公开"的页面连接更"私有"的地址；新权限名不认识时用旧名', async () => {
    const onChange = vi.fn()
    const local = new NetworkGuard('ws://127.0.0.1:7717', { pageUrl: 'http://localhost:5173/', permissions: permissions({}) }, onChange)
    expect(local.lnaApplies).toBe(false)
    const perms = permissions({ 'local-network-access': new FakePermission('granted') })
    const pub = new NetworkGuard('ws://127.0.0.1:7717', { pageUrl: 'https://shop.example/', permissions: perms }, onChange)
    await pub.whenReady()
    expect(pub.lnaApplies).toBe(true)
    expect(perms.asked).toEqual(['loopback-network', 'local-network-access'])
    expect(pub.lnaPermission).toBe('granted')
    const lan = new NetworkGuard('ws://127.0.0.1:7717', { pageUrl: 'http://192.168.1.5/', permissions: permissions({}) }, onChange)
    expect(lan.lnaApplies).toBe(true)
    // 浏览器不支持该权限：不诊断
    await lan.whenReady()
    expect(lan.lnaPermission).toBeUndefined()
    expect(lan.diagnose()).toBeUndefined()
  })

  it('LNA 被拒：进入 blocked、不按退避重试；授权变为允许后自动重连', async () => {
    const perm = new FakePermission('denied')
    const shared = sharedOwner()
    const a = await tab(
      { permissions: permissions({ 'loopback-network': perm }), isSecureContext: true, createSharedLink: shared.createSharedLink },
      'https://shop.example/',
    )
    expect(shared.hostSockets).toHaveLength(1)
    shared.hostSockets[0]!.fail()
    await new Promise((r) => setTimeout(r, 0))
    expect(a.app.state).toMatchObject({ status: 'blocked', cause: 'local-network-access' })
    expect((a.app.state as { message: string }).message).toContain('网站设置')
    expect(a.core.methods()).not.toContain('handleDisconnected')
    expect(a.core.pollTimeout()).toBeUndefined()

    perm.set('granted')
    expect(a.app.state.status).toBe('connecting')
    expect(shared.hostSockets).toHaveLength(2)
    negotiate(shared.hostSockets[1]!)
    expect(a.app.state.status).toBe('connected')
  })

  it('被拦截时低频重新探测，成功即解除', async () => {
    const perm = new FakePermission('denied')
    const a = await tab(
      { permissions: permissions({ 'loopback-network': perm }), isSecureContext: true, blockedRetryMs: 20 },
      'https://shop.example/',
    )
    a.direct[0]!.fail()
    expect(a.app.state.status).toBe('blocked')
    await new Promise((r) => setTimeout(r, 40))
    expect(a.direct).toHaveLength(2)
    expect(a.app.state.status).toBe('blocked') // 探测期间保持 blocked
    a.direct[1]!.open()
    expect(a.app.state.status).toBe('connected')
  })

  it('授权为 prompt：共享通道失败后由页面直接连接（可弹出授权提示）；再失败按普通断开退避', async () => {
    const perm = new FakePermission('prompt')
    const shared = sharedOwner()
    const a = await tab(
      { permissions: permissions({ 'loopback-network': perm }), isSecureContext: true, createSharedLink: shared.createSharedLink },
      'https://shop.example/',
    )
    shared.hostSockets[0]!.fail()
    await new Promise((r) => setTimeout(r, 0))
    expect(a.direct).toHaveLength(1)
    a.direct[0]!.fail()
    expect(a.app.state.status).not.toBe('blocked')
    expect(a.core.methods()).toContain('handleDisconnected')
  })

  it('wake() 在拦截状态下立即重试', async () => {
    const a = await tab(
      { permissions: permissions({ 'loopback-network': new FakePermission('denied') }), isSecureContext: true },
      'https://shop.example/',
    )
    a.direct[0]!.fail()
    expect(a.app.state.status).toBe('blocked')
    a.app.wake()
    expect(a.direct).toHaveLength(2)
    a.direct[1]!.open()
    expect(a.app.state.status).toBe('connected')
  })

  it('非安全上下文：insecure-context', async () => {
    const a = await tab(
      { permissions: permissions({ 'loopback-network': new FakePermission('prompt') }), isSecureContext: false },
      'http://shop.example/',
    )
    a.direct[0]!.fail()
    expect(a.app.state).toMatchObject({ status: 'blocked', cause: 'insecure-context' })
  })

  it('已授权或不适用时，失败按普通断开处理（退避重连）', async () => {
    const a = await tab(
      { permissions: permissions({ 'loopback-network': new FakePermission('granted') }), isSecureContext: true },
      'https://shop.example/',
    )
    a.direct[0]!.fail()
    expect(a.core.methods()).toContain('handleDisconnected')
    const b = await tab()
    b.direct[0]!.fail()
    expect(b.core.methods()).toContain('handleDisconnected')
  })

  it('CSP connect-src 拦截：blocked csp，后续连接请求不再尝试', async () => {
    const a = await tab()
    a.doc.dispatchEvent(
      Object.assign(new Event('securitypolicyviolation'), {
        blockedURI: 'ws://127.0.0.1:7717/',
        effectiveDirective: 'connect-src',
      }),
    )
    a.direct[0]!.fail()
    expect(a.app.state).toMatchObject({ status: 'blocked', cause: 'csp' })
    expect((a.app.state as { message: string }).message).toContain('connect-src')
    // 无关的违规不影响判断；dispose 后状态为 stopped
    a.app.dispose()
    expect(a.app.state.status).toBe('stopped')
  })

  it('SharedWorker 报告的 CSP 拦截', async () => {
    const sockets: FakeSocket[] = []
    const ownerWithSockets = new MuxOwner({
      createWebSocket: (url) => {
        const s = new FakeSocket(url)
        sockets.push(s)
        return s
      },
      cspBlocked: () => true,
    })
    const createSharedLink = (): MuxLink => {
      const ep: OwnerEndpoint = { onmessage: null, onreset: null, post: (m) => handle.receive(m), close: () => {} }
      const handle = ownerWithSockets.attach((m) => ep.onmessage?.(m))
      return new MuxLink(ep)
    }
    const a = await tab({ createSharedLink })
    sockets[0]!.fail()
    await new Promise((r) => setTimeout(r, 0))
    expect(a.app.state).toMatchObject({ status: 'blocked', cause: 'csp' })
    expect((a.app.state as { message: string }).message).toContain('SharedWorker')
  })
})
