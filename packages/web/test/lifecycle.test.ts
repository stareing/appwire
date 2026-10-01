/**
 * 生命周期（spec/lifecycle.md）：休眠 / 唤醒 / 快速恢复 / 持有 / URL 唤醒令牌 / bfcache。
 *
 * - 「真实核心」一组：驱动层 + 真实 WASM 核心 + 模拟 Host，验证协议消息与资源释放（需要先 build:wasm，否则跳过）；
 * - 「驱动接线」一组：假核心，验证浏览器事件到核心方法的映射。
 */
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { CoreFactory, CoreLoader } from '../src/core'
import { AppMcpDriver, parseWakeTokenJs, stripWakeFragment } from '../src/driver'
import { getToolHub } from '../src/tool-hub'
import { wasmCoreFactory, type WasmBindings } from '../src/wasm-loader'
import { installWebMcp, type ModelContext } from '../src/webmcp'
import type { AppMcpOptions, ToolContext } from '../src/types'
import { FakeSocket, settle, setup, silentLogger } from './fakes'
import { coreTool, defineNative, FakeNativeModelContext, hostCall } from './webmcp-fakes'

const glue = resolve(process.cwd(), 'src/wasm/app_mcp_wasm.js')
const wasm = resolve(process.cwd(), 'src/wasm/app_mcp_wasm_bg.wasm')
const available = existsSync(glue) && existsSync(wasm)

let factoryPromise: Promise<CoreFactory> | undefined
function loadRealCore(): Promise<CoreFactory> {
  factoryPromise ??= (async () => {
    const mod = (await import(/* @vite-ignore */ pathToFileURL(glue).href)) as WasmBindings
    await mod.default({ module_or_path: readFileSync(wasm) })
    return wasmCoreFactory(mod)
  })()
  return factoryPromise
}

// ---------------------------------------------------------------------------
// 假页面：可控的 visibilityState、location / history、页面事件
// ---------------------------------------------------------------------------

class FakeDoc extends EventTarget {
  visibilityState: 'visible' | 'hidden' = 'visible'
  hasFocus(): boolean {
    return this.visibilityState === 'visible'
  }
  setVisible(visible: boolean): void {
    this.visibilityState = visible ? 'visible' : 'hidden'
    this.dispatchEvent(new Event('visibilitychange'))
  }
}

class FakeWin extends EventTarget {
  replaced: string[] = []
  location: { href: string }
  history = {
    state: { page: 1 } as unknown,
    replaceState: (state: unknown, _title: string, url: string) => {
      this.history.state = state
      this.location.href = url
      this.replaced.push(url)
    },
  }
  constructor(href = 'https://shop.example/app') {
    super()
    this.location = { href }
  }
  pageTransition(type: 'pagehide' | 'pageshow', persisted: boolean): void {
    this.dispatchEvent(Object.assign(new Event(type), { persisted }))
  }
}

type Json = { jsonrpc?: string; id?: number | string; method?: string; params?: any; result?: any; error?: any }

interface Page {
  app: AppMcpDriver
  sockets: FakeSocket[]
  doc: FakeDoc
  win: FakeWin
  socket(): FakeSocket
  /** 当前（最后一个）连接上发送的消息。 */
  sent(): Json[]
  last(method: string): Json | undefined
  reply(msg: Json, result: unknown): void
  /** 打开当前 socket 并完成握手（`toolsCurrent` 由 Host 决定）。返回 hello 参数。 */
  handshake(hello?: Record<string, unknown>): Json
}

async function page(options: Partial<AppMcpOptions>, win = new FakeWin(), loadCore: CoreLoader = loadRealCore): Promise<Page> {
  const sockets: FakeSocket[] = []
  const doc = new FakeDoc()
  const app = new AppMcpDriver(
    { appId: 'shop', appName: '示例商城', logger: silentLogger(), ...options },
    {
      loadCore,
      createWebSocket: (url) => {
        const s = new FakeSocket(url)
        sockets.push(s)
        return s
      },
      now: () => Date.now(),
      window: win as unknown as Window,
      document: doc as unknown as Document,
    },
  )
  // 等 WASM 加载（真实 I/O，不受假计时器影响）
  const deadline = performance.now() + 10_000
  while ((app as unknown as { core?: unknown }).core === undefined && performance.now() < deadline) {
    await new Promise<void>((r) => setImmediate(r))
  }
  await settle()
  const p: Page = {
    app,
    sockets,
    doc,
    win,
    socket() {
      const s = sockets.at(-1)
      if (!s) throw new Error('没有 WebSocket')
      return s
    },
    sent: () => p.socket().sent.map((t) => JSON.parse(t) as Json),
    last: (method) => p.sent().filter((m) => m.method === method).at(-1),
    reply(msg, result) {
      p.socket().receive(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }))
    },
    handshake(extra = {}) {
      p.socket().open()
      const hello = p.last('app/hello') as Json
      p.reply(hello, { status: 'paired', token: 'tk', protocolVersion: '1', hostVersion: '0.1.0', ...extra })
      return hello
    },
  }
  return p
}

/** 等待进行中的 handler 等微任务。 */
async function flush(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0)
  await settle()
}

beforeEach(() => {
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  vi.useRealTimers()
})

describe.skipIf(!available)('真实核心：休眠与唤醒', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'] })
  })

  it('idle 超时 → app/sleep → 被接受后无 WebSocket、无定时器 → 重新可见时快速恢复', async () => {
    const p = await page({ lifecycle: { mode: 'idle', idleTimeoutMs: 1000 } })
    p.app.tool('cart.add', { description: '加入购物车', handler: () => 'ok' })
    const states: string[] = []
    p.app.onStateChange((s) => states.push(s.status))
    await flush()
    const hello1 = p.handshake()
    expect(hello1.params.resumeToken).toBeUndefined()
    expect(p.sent().map((m) => m.method)).toContain('tools/sync')
    expect(p.app.state.status).toBe('connected')

    await vi.advanceTimersByTimeAsync(999)
    expect(p.last('app/sleep')).toBeUndefined()
    await vi.advanceTimersByTimeAsync(1)
    const sleep = p.last('app/sleep') as Json
    expect(sleep.params).toMatchObject({
      reason: 'idle',
      wake: { kind: 'web-url', target: 'https://shop.example/app', background: false },
    })
    expect(sleep.params.toolsHash).toMatch(/^[0-9a-f]{16}$/)
    expect(p.app.state.status).toBe('connected') // sleeping 对外仍为 connected

    p.reply(sleep, { accepted: true, resumeToken: 'rt-1' })
    expect(p.app.state.status).toBe('dormant')
    expect(p.socket().closed).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
    // 休眠期间时间流逝不会产生连接
    await vi.advanceTimersByTimeAsync(600_000)
    expect(p.sockets).toHaveLength(1)

    // 隐藏 → 可见：回连
    p.doc.setVisible(false)
    expect(p.sockets).toHaveLength(1)
    p.doc.setVisible(true)
    expect(p.sockets).toHaveLength(2)
    expect(states).toContain('waking')
    p.socket().open()
    const hello2 = p.last('app/hello') as Json
    expect(hello2.params).toMatchObject({ resumeToken: 'rt-1', toolsHash: sleep.params.toolsHash, wakeReason: 'visible' })
    p.reply(hello2, { status: 'paired', token: 'tk', protocolVersion: '1', hostVersion: '0.1.0', toolsCurrent: true })
    expect(p.sent().map((m) => m.method)).toEqual(['app/hello', 'app/visibility', 'app/ready'])
    expect(p.app.state.status).toBe('connected')

    // 回连后工具照常可调用
    p.socket().receive(
      JSON.stringify({ jsonrpc: '2.0', id: 'h1', method: 'tools/invoke', params: { callId: 'c1', name: 'cart.add', arguments: {} } }),
    )
    await flush()
    expect(p.sent().find((m) => m.id === 'h1')?.result).toEqual({ data: 'ok' })
    p.app.dispose()
  })

  it('页面隐藏时使用 hiddenIdleTimeoutMs', async () => {
    const p = await page({ lifecycle: { mode: 'idle', idleTimeoutMs: 60_000, hiddenIdleTimeoutMs: 500 } })
    await flush()
    p.handshake()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(p.last('app/sleep')).toBeUndefined()
    p.doc.setVisible(false)
    expect(p.last('app/visibility')?.params).toMatchObject({ visibility: 'hidden' })
    await vi.advanceTimersByTimeAsync(499)
    expect(p.last('app/sleep')).toBeUndefined()
    await vi.advanceTimersByTimeAsync(1)
    // 隐藏只缩短空闲计时；计时到期的休眠原因仍为 idle（background 专指进入后台立即休眠，如 bfcache）
    expect(p.last('app/sleep')?.params).toMatchObject({ reason: 'idle' })
    p.app.dispose()
  })

  it('hold() 阻止休眠，释放后按空闲计时休眠', async () => {
    const p = await page({ lifecycle: { mode: 'idle', idleTimeoutMs: 1000 } })
    await flush()
    p.handshake()
    const held = p.app.hold()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(p.last('app/sleep')).toBeUndefined()
    held.release()
    held.release() // 重复释放无效果
    await vi.advanceTimersByTimeAsync(1000)
    expect(p.last('app/sleep')).toBeDefined()
    p.app.dispose()
  })

  it('调用进行中不休眠；ToolContext.hold 在调用结束后继续阻止休眠', async () => {
    const p = await page({ lifecycle: { mode: 'idle', idleTimeoutMs: 1000 } })
    let finish!: (v: string) => void
    let ctxRelease: (() => void) | undefined
    p.app.tool('long', {
      description: '长任务',
      handler: (_: unknown, ctx: ToolContext) =>
        new Promise<string>((r) => {
          finish = r
          ctxRelease = ctx.hold?.().release
        }),
    })
    await flush()
    p.handshake()
    p.socket().receive(
      JSON.stringify({ jsonrpc: '2.0', id: 'h1', method: 'tools/invoke', params: { callId: 'c1', name: 'long', arguments: {} } }),
    )
    await flush()
    await vi.advanceTimersByTimeAsync(3_000)
    expect(p.last('app/sleep')).toBeUndefined()
    finish('done')
    await flush()
    expect(p.sent().find((m) => m.id === 'h1')?.result).toEqual({ data: 'done' })
    await vi.advanceTimersByTimeAsync(3_000)
    expect(p.last('app/sleep')).toBeUndefined()
    ctxRelease?.()
    await vi.advanceTimersByTimeAsync(1000)
    expect(p.last('app/sleep')).toBeDefined()
    p.app.dispose()
  })

  it('休眠期间注册工具不唤醒；回连时摘要不一致 → 完整同步', async () => {
    const p = await page({ lifecycle: { mode: 'idle', idleTimeoutMs: 1000 } })
    p.app.tool('a', { description: 'A', handler: () => null })
    await flush()
    p.handshake()
    p.app.sleep()
    const sleep = p.last('app/sleep') as Json
    expect(sleep.params.reason).toBe('app')
    p.reply(sleep, { accepted: true, resumeToken: 'rt-2' })
    expect(p.app.state.status).toBe('dormant')

    p.app.tool('b', { description: 'B', handler: () => null })
    await flush()
    expect(p.sockets).toHaveLength(1)
    expect(vi.getTimerCount()).toBe(0)

    p.app.wake()
    expect(p.sockets).toHaveLength(2)
    p.socket().open()
    const hello = p.last('app/hello') as Json
    expect(hello.params).toMatchObject({ resumeToken: 'rt-2', wakeReason: 'app' })
    expect(hello.params.toolsHash).not.toBe(sleep.params.toolsHash)
    p.reply(hello, { status: 'paired', token: 'tk', protocolVersion: '1', hostVersion: '0.1.0', toolsCurrent: false })
    const sync = p.last('tools/sync') as Json
    expect(sync.params.tools.map((t: { name: string }) => t.name).sort()).toEqual(['a', 'b'])
    expect(p.app.state.status).toBe('connected')
    p.app.dispose()
  })

  it('app/sleep 被拒且 retryAfterMs 为 0：不经定时器立即重试', async () => {
    const p = await page({ lifecycle: { mode: 'idle', idleTimeoutMs: 60_000 } })
    await flush()
    p.handshake()
    p.app.sleep()
    const first = p.last('app/sleep') as Json
    p.reply(first, { accepted: false, retryAfterMs: 0 })
    const sleeps = p.sent().filter((m) => m.method === 'app/sleep')
    expect(sleeps).toHaveLength(2)
    p.reply(sleeps[1] as Json, { accepted: true, resumeToken: 'rt' })
    expect(p.app.state.status).toBe('dormant')
    expect(vi.getTimerCount()).toBe(0)
    p.app.dispose()
  })

  it('on-demand：启动不连接，connectNow() 时连接', async () => {
    const p = await page({ lifecycle: { mode: 'on-demand', graceMs: 2000 } })
    p.app.tool('a', { description: 'A', handler: () => null })
    await flush()
    expect(p.sockets).toHaveLength(0)
    expect(p.app.state.status).toBe('dormant')
    expect(vi.getTimerCount()).toBe(0)
    p.app.connectNow()
    expect(p.sockets).toHaveLength(1)
    const hello = p.handshake()
    expect(hello.params.wakeReason).toBe('app')
    expect(p.last('tools/sync')).toBeDefined()
    await vi.advanceTimersByTimeAsync(2000)
    expect(p.last('app/sleep')?.params).toMatchObject({ reason: 'grace' })
    p.app.dispose()
  })

  it('URL 唤醒令牌：读取后从地址栏移除（保留其他 hash），hello 携带 launchToken', async () => {
    const win = new FakeWin('https://shop.example/app?x=1#tab=cart&app-mcp-wake=tok-ABC_123')
    const p = await page({ lifecycle: { mode: 'on-demand' } }, win)
    expect(win.replaced).toEqual(['https://shop.example/app?x=1#tab=cart'])
    expect(win.history.state).toEqual({ page: 1 })
    expect(p.sockets).toHaveLength(1)
    p.socket().open()
    const hello = p.last('app/hello') as Json
    expect(hello.params).toMatchObject({ launchToken: 'tok-ABC_123', wakeReason: 'os-activation' })
    p.reply(hello, { status: 'paired', token: 'tk', protocolVersion: '1', hostVersion: '0.1.0' })
    p.app.sleep()
    // 唤醒描述不含令牌
    expect(p.last('app/sleep')?.params.wake).toEqual({
      kind: 'web-url',
      target: 'https://shop.example/app?x=1#tab=cart',
      background: false,
    })
    p.app.dispose()
  })

  it('页面已打开时 hash 变为唤醒令牌（hashchange）：回连', async () => {
    const p = await page({ lifecycle: { mode: 'on-demand' } })
    expect(p.sockets).toHaveLength(0)
    p.win.location.href = 'https://shop.example/app#app-mcp-wake=tok2'
    p.win.dispatchEvent(new Event('hashchange'))
    expect(p.win.location.href).toBe('https://shop.example/app')
    expect(p.sockets).toHaveLength(1)
    p.socket().open()
    expect(p.last('app/hello')?.params).toMatchObject({ launchToken: 'tok2', wakeReason: 'os-activation' })
    p.app.dispose()
  })

  it('bfcache：pagehide(persisted) 立即 app/sleep，pageshow(persisted) 回连', async () => {
    const p = await page({})
    await flush()
    p.handshake()
    p.win.pageTransition('pagehide', false) // 非 bfcache：不处理
    expect(p.last('app/sleep')).toBeUndefined()
    p.win.pageTransition('pagehide', true)
    const sleep = p.last('app/sleep') as Json
    expect(sleep.params.reason).toBe('background')
    p.reply(sleep, { accepted: true, resumeToken: 'rt-b' })
    expect(p.app.state.status).toBe('dormant')
    expect(vi.getTimerCount()).toBe(0)
    p.win.pageTransition('pageshow', true)
    expect(p.sockets).toHaveLength(2)
    p.socket().open()
    expect(p.last('app/hello')?.params).toMatchObject({ resumeToken: 'rt-b', wakeReason: 'visible' })
    p.app.dispose()
  })

  it('persistent 模式：重新可见不触发唤醒，也不自动休眠', async () => {
    const p = await page({})
    await flush()
    p.handshake()
    p.doc.setVisible(false)
    p.doc.setVisible(true)
    expect(p.sockets).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(120_000)
    const all = p.sockets.flatMap((s) => s.sent.map((t) => (JSON.parse(t) as Json).method))
    expect(all).not.toContain('app/sleep')
    p.app.dispose()
  })
})

// ---------------------------------------------------------------------------
// 驱动接线（假核心）
// ---------------------------------------------------------------------------

describe('驱动接线', () => {
  it('把生命周期配置与唤醒描述交给核心', async () => {
    const h = setup({ lifecycle: { mode: 'idle', idleTimeoutMs: 5, hiddenIdleTimeoutMs: 1, graceMs: 2 } })
    await settle()
    expect(h.core.config?.lifecycle).toEqual({
      mode: 'idle',
      idleTimeoutMs: 5,
      hiddenIdleTimeoutMs: 1,
      graceMs: 2,
      wake: { kind: 'web-url', target: location.href, background: false },
    })
  })

  it('未配置时为 persistent', async () => {
    const h = setup()
    await settle()
    expect(h.core.config?.lifecycle?.mode).toBe('persistent')
  })

  it('没有唤醒令牌时不调用 handleWake、不改地址', async () => {
    const spy = vi.spyOn(history, 'replaceState')
    const h = setup()
    await settle()
    expect(h.core.methods()).not.toContain('handleWake')
    expect(spy).not.toHaveBeenCalled()
    spy.mockRestore()
  })

  it('核心加载前的 wake / sleep / hold 在 start 之后按顺序执行', async () => {
    const h = setup({ lifecycle: { mode: 'idle' } }, true)
    h.app.sleep()
    const held = h.app.hold()
    h.app.wake()
    h.app.connectNow()
    await h.load()
    held.release()
    const m = h.core.methods()
    expect(m.slice(m.indexOf('start'))).toEqual([
      'start',
      'sleepWithReason',
      'hold',
      'wakeWithReason',
      'connectNow',
      'wakeWithReason',
      'releaseHold',
    ])
  })

  it('加载前获取并释放的持有不交给核心', async () => {
    const h = setup({}, true)
    h.app.hold().release()
    await h.load()
    expect(h.core.methods()).not.toContain('hold')
  })

  it('ToolContext.hold 映射为 holdForCall；本地调用退回普通持有', async () => {
    const h = setup()
    await settle()
    h.socket().open()
    const releases: Array<() => void> = []
    h.app.tool('t', {
      description: 't',
      handler: (_: unknown, ctx: ToolContext) => {
        const r = ctx.hold?.()
        if (r) releases.push(() => r.release())
        return 1
      },
    })
    await hostCall(h, 't')
    const [[callId]] = h.core.callsOf('holdForCall') as [[string]]
    expect(callId).toMatch(/^host-/)
    expect(h.core.methods()).not.toContain('hold')
    releases[0]?.()
    releases[0]?.()
    expect(h.core.callsOf('releaseHold')).toHaveLength(1)
    // 本地调用（WebMCP / 测试）：核心不认识该调用，退回普通持有
    const view = getToolHub(h.app)?.list().find((t) => t.name === 't')
    await view?.call({}, new AbortController().signal)
    expect(h.core.methods()).toContain('hold')
  })

  it('idle 模式：重新可见且 dormant 时 wakeWithReason("visible")；persistent 不唤醒', async () => {
    const run = async (mode: 'idle' | 'persistent'): Promise<unknown[][]> => {
      const d = new FakeDoc()
      const h = setupWithDoc(mode, d)
      await settle()
      h.core.setState({ status: 'dormant' })
      h.socket().script() // 任意输入触发 pump，同步状态
      expect(h.app.state.status).toBe('dormant')
      d.setVisible(false)
      d.setVisible(true)
      h.app.dispose()
      return h.core.callsOf('wakeWithReason')
    }
    expect(await run('idle')).toEqual([['visible', 1000]])
    expect(await run('persistent')).toEqual([])
  })

  it('freeze / resume（idle 模式）：冻结时 background 休眠，恢复时回连；persistent 模式冻结不休眠', async () => {
    const d = new FakeDoc()
    const h = setupWithDoc('idle', d)
    await settle()
    h.socket().open()
    d.dispatchEvent(new Event('freeze'))
    expect(h.core.callsOf('sleepWithReason')).toEqual([['background', 1000]])
    expect(h.app.state.status).toBe('dormant')
    d.dispatchEvent(new Event('resume'))
    // 可见性恢复与 resume 各请求一次唤醒，第二次在 waking 状态下被跳过
    expect(h.core.callsOf('wakeWithReason')).toEqual([['visible', 1000]])
    expect(h.app.state.status).toBe('waking')

    const d2 = new FakeDoc()
    const p = setupWithDoc('persistent', d2)
    await settle()
    d2.dispatchEvent(new Event('freeze'))
    expect(p.core.methods()).not.toContain('sleepWithReason')
  })

  it('idleExit 事件被忽略', async () => {
    const h = setup()
    await settle()
    h.socket().open()
    h.socket().script({ type: 'idleExit' })
    expect(h.logger.warn).not.toHaveBeenCalled()
    expect(h.logger.error).not.toHaveBeenCalled()
  })

  it('dispose 后移除页面监听', async () => {
    const win = new FakeWin()
    const d = new FakeDoc()
    const h = setupWithDoc('idle', d, win)
    await settle()
    h.app.dispose()
    h.core.calls = []
    win.pageTransition('pagehide', true)
    d.dispatchEvent(new Event('freeze'))
    expect(h.core.calls).toEqual([])
  })

  it('WebMCP 镜像在休眠 / 唤醒之间保持工具有效', async () => {
    const d = new FakeDoc()
    const h = setupWithDoc('idle', d)
    await settle()
    h.socket().open()
    const native = new FakeNativeModelContext()
    const undo = defineNative(document, native)
    const uninstall = installWebMcp(h.app)
    const mc = (document as unknown as { modelContext: ModelContext }).modelContext
    await mc.registerTool({
      name: 'add_todo',
      description: '添加待办',
      inputSchema: { type: 'object', properties: {} },
      execute: async () => ({ content: [{ type: 'text', text: 'ok' }] }),
    })
    await settle()
    h.app.sleep()
    expect(h.app.state.status).toBe('dormant')
    expect(h.socket().closed).toBe(true)
    expect(native.tools.has('add_todo')).toBe(true)
    expect(coreTool(h, 'add_todo')).toBeDefined()
    // 浏览器内置 AI 在休眠中仍可调用
    expect(await native.invoke('add_todo', {})).toEqual({ content: [{ type: 'text', text: 'ok' }] })
    h.app.wake()
    expect(h.sockets).toHaveLength(2)
    h.socket().open()
    expect(await hostCall(h, 'add_todo')).toEqual({ data: { text: 'ok' } })
    uninstall()
    undo()
    h.app.dispose()
  })
})

describe('唤醒令牌解析（JS）', () => {
  it('parseWakeTokenJs', () => {
    expect(parseWakeTokenJs('https://a/b#app-mcp-wake=abc')).toBe('abc')
    expect(parseWakeTokenJs('https://a/b#x=1&app-mcp-wake=a.b~c-d_e')).toBe('a.b~c-d_e')
    expect(parseWakeTokenJs('https://a/b?app-mcp-wake=abc')).toBeUndefined()
    expect(parseWakeTokenJs('https://a/b#app-mcp-wake=')).toBeUndefined()
    expect(parseWakeTokenJs('https://a/b#app-mcp-wake=a%20b')).toBeUndefined()
  })

  it('stripWakeFragment', () => {
    expect(stripWakeFragment('https://a/b#app-mcp-wake=abc')).toBe('https://a/b')
    expect(stripWakeFragment('https://a/b?q=1#x=1&app-mcp-wake=abc&y=2')).toBe('https://a/b?q=1#x=1&y=2')
    expect(stripWakeFragment('https://a/b#x=1')).toBeUndefined()
    expect(stripWakeFragment('https://a/b')).toBeUndefined()
  })

  it.skipIf(!available)('与 WASM parseWakeToken 在 URL 片段上一致', async () => {
    const factory = await loadRealCore()
    for (const href of ['https://a/b#app-mcp-wake=abc', 'https://a/b#x=1&app-mcp-wake=k-1', 'https://a/b#x=1']) {
      expect(factory.parseWakeToken?.(href) ?? undefined).toBe(parseWakeTokenJs(href))
    }
  })
})

/** 假核心 + 指定 document / window。 */
function setupWithDoc(mode: 'idle' | 'persistent', doc: FakeDoc, win?: FakeWin): ReturnType<typeof setup> {
  const h = setup({ lifecycle: { mode } }, false, {
    document: doc as unknown as Document,
    ...(win && { window: win as unknown as Window }),
  })
  return h
}
