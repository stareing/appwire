/**
 * 生命周期：`data-mcp-*` 声明的工具在休眠 / 唤醒之间保持有效。
 *
 * 使用 @app-mcp/web 的真实驱动层 + 真实 WASM 核心 + 模拟 Host（假 WebSocket），
 * 需要先 `pnpm --filter @app-mcp/web build:wasm`，否则跳过。
 */
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { CoreClient, CoreConfig, CoreFactory } from '../../web/src/core'
import { AppMcpDriver } from '../../web/src/driver'
import { FakeSocket, silentLogger } from '../../web/test/fakes'
import { attachDom } from '../src'

// vitest 以包目录为 cwd（happy-dom 环境下 import.meta.url 不是 file: URL）
const glue = resolve(process.cwd(), '../web/src/wasm/app_mcp_wasm.js')
const wasm = resolve(process.cwd(), '../web/src/wasm/app_mcp_wasm_bg.wasm')
const available = existsSync(glue) && existsSync(wasm)

let factoryPromise: Promise<CoreFactory> | undefined
function loadRealCore(): Promise<CoreFactory> {
  factoryPromise ??= (async () => {
    const mod = (await import(/* @vite-ignore */ pathToFileURL(glue).href)) as {
      default: (init: { module_or_path: BufferSource }) => Promise<unknown>
      WasmClient: new (c: CoreConfig) => CoreClient
      parseWakeToken: (args: string) => string | undefined
    }
    await mod.default({ module_or_path: readFileSync(wasm) })
    const factory: CoreFactory = (config) => new mod.WasmClient(config)
    factory.parseWakeToken = mod.parseWakeToken
    return factory
  })()
  return factoryPromise
}

type Json = { jsonrpc?: string; id?: number | string; method?: string; params?: any; result?: any; error?: any }

const tick = async (n = 10) => {
  for (let i = 0; i < n; i++) await new Promise((r) => setTimeout(r, 0))
}

async function waitFor(check: () => boolean, ms = 3000): Promise<void> {
  const deadline = Date.now() + ms
  while (!check()) {
    if (Date.now() > deadline) throw new Error('等待超时')
    await new Promise((r) => setTimeout(r, 5))
  }
}

let app: AppMcpDriver | undefined
let detach: (() => void) | undefined

beforeEach(() => {
  document.body.innerHTML = ''
  sessionStorage.clear()
  localStorage.clear()
})
afterEach(() => {
  detach?.()
  detach = undefined
  app?.dispose()
  app = undefined
})

describe.skipIf(!available)('休眠 / 唤醒（真实 WASM 核心）', () => {
  it('data-mcp-* 工具在休眠期间保留，唤醒后快速恢复并可调用；休眠期间新增的声明在回连时同步', async () => {
    document.body.innerHTML = `
      <button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车">清空</button>
      <form data-mcp-tool="todo.add" data-mcp-desc="添加待办">
        <input name="title" required />
        <button type="submit">添加</button>
      </form>`
    const clicks = vi.fn()
    document.querySelector('button')!.addEventListener('click', clicks)
    const submits: string[] = []
    document.querySelector('form')!.addEventListener('submit', (e) => {
      e.preventDefault()
      submits.push((e.target as HTMLFormElement).querySelector('input')!.value)
    })

    const sockets: FakeSocket[] = []
    app = new AppMcpDriver(
      { appId: 'shop', appName: '示例商城', logger: silentLogger(), lifecycle: { mode: 'idle', idleTimeoutMs: 60_000 } },
      {
        loadCore: loadRealCore,
        createWebSocket: (url) => {
          const s = new FakeSocket(url)
          sockets.push(s)
          return s
        },
        now: () => Date.now(),
      },
    )
    detach = attachDom(app, { settleMs: 0, snapshot: false })
    await waitFor(() => sockets.length === 1)

    const sock = () => sockets.at(-1)!
    const sent = () => sock().sent.map((t) => JSON.parse(t) as Json)
    const last = (method: string) => sent().filter((m) => m.method === method).at(-1)
    const reply = (msg: Json, result: unknown) =>
      sock().receive(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }))
    const paired = { status: 'paired', token: 'tk', protocolVersion: '1', hostVersion: '0.1.0' }
    let nextId = 1
    const invoke = async (name: string, args: unknown) => {
      const id = `h${nextId++}`
      sock().receive(
        JSON.stringify({ jsonrpc: '2.0', id, method: 'tools/invoke', params: { callId: id, name, arguments: args } }),
      )
      await waitFor(() => sent().some((m) => m.id === id))
      return sent().find((m) => m.id === id)
    }

    // 首次连接：完整同步
    sock().open()
    reply(last('app/hello')!, paired)
    const sync1 = last('tools/sync')!
    expect(sync1.params.tools.map((t: { name: string }) => t.name).sort()).toEqual(['cart.clear', 'todo.add'])
    expect(app.state.status).toBe('connected')
    expect(await invoke('cart.clear', {})).toMatchObject({ result: { data: { ok: true } } })
    expect(clicks).toHaveBeenCalledTimes(1)

    // 休眠
    app.sleep()
    const sleep = last('app/sleep')!
    reply(sleep, { accepted: true, resumeToken: 'rt-1' })
    expect(app.state.status).toBe('dormant')
    expect(sock().closed).toBe(true)

    // 休眠期间 DOM 变化不回连（仅更新注册表）
    const extra = document.createElement('button')
    extra.setAttribute('data-mcp-tool', 'cart.checkout')
    extra.setAttribute('data-mcp-desc', '结算')
    extra.textContent = '结算'
    document.body.appendChild(extra)
    await tick()
    await new Promise((r) => setTimeout(r, 150)) // 等 DOM 观察批量刷新（rAF / 定时器兜底）
    expect(sockets).toHaveLength(1)

    // 唤醒：摘要变化 → Host 回 toolsCurrent: false → 完整同步，包含休眠期间新增的工具
    app.wake()
    expect(sockets).toHaveLength(2)
    sock().open()
    const hello2 = last('app/hello')!
    expect(hello2.params).toMatchObject({ resumeToken: 'rt-1', wakeReason: 'app' })
    expect(hello2.params.toolsHash).not.toBe(sleep.params.toolsHash)
    reply(hello2, { ...paired, toolsCurrent: false })
    const sync2 = last('tools/sync')!
    expect(sync2.params.tools.map((t: { name: string }) => t.name).sort()).toEqual([
      'cart.checkout',
      'cart.clear',
      'todo.add',
    ])
    expect(app.state.status).toBe('connected')

    // 回连后 DOM 工具照常可用（handler 仍绑定到原元素）
    expect(await invoke('todo.add', { title: '买牛奶' })).toMatchObject({ result: { data: expect.anything() } })
    expect(submits).toEqual(['买牛奶'])
    expect(await invoke('cart.clear', {})).toMatchObject({ result: { data: { ok: true } } })
    expect(clicks).toHaveBeenCalledTimes(2)

    // 再次休眠 → 摘要未变 → 快速恢复（跳过 tools/sync）
    app.sleep()
    const sleep2 = last('app/sleep')!
    reply(sleep2, { accepted: true, resumeToken: 'rt-2' })
    expect(app.state.status).toBe('dormant')
    app.wake()
    sock().open()
    const hello3 = last('app/hello')!
    expect(hello3.params.toolsHash).toBe(sleep2.params.toolsHash)
    reply(hello3, { ...paired, toolsCurrent: true })
    expect(sent().map((m) => m.method)).not.toContain('tools/sync')
    expect(app.state.status).toBe('connected')
    expect(await invoke('cart.checkout', {})).toMatchObject({ result: { data: { ok: true } } })
  })
})
