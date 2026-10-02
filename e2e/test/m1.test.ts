/**
 * M1 验收（app-mcp-plan.md 第 15 节）：MCP 客户端（Streamable HTTP）→ app-mcp-host serve → shop Demo（vite dev，persistent 模式）。
 *
 * 各用例按顺序共用同一个 Host / 页面状态。关键用例另由一个 MCP 2026-07-28（modern，无会话 + subscriptions/listen）客户端
 * 并行验证（docs/plans/12-mcp-stateless.md S7）；legacy 客户端的断言不变。
 */
import { afterAll, beforeAll, describe, expect, inject, it } from 'vitest'
import { Browser, type Page } from '../src/browser'
import { type HostHandle, type ShopServer, startHost, startShop } from '../src/env'
import { McpClient, data, ok, texts } from '../src/mcp-client'
import { establishedTo, waitFor } from '../src/util'

let host: HostHandle
let shop: ShopServer
let browser: Browser
let tabA: Page
let tabB: Page | undefined
let modern: McpClient

const STATIC_TOOLS = ['shop.catalog.search', 'shop.info', 'shop.deliveryEstimate']
/** 只对无会话（modern）请求列出的内置工具：任务句柄（docs/plans/12-mcp-stateless.md S8，spec/hub-api.md 3.6）。 */
const MODERN_ONLY_TOOLS = ['apps.task.begin', 'apps.task.end']

/** modern 的工具列表 = legacy 的列表 + 任务句柄工具。 */
async function expectModernToolsMatch(legacyNames: string[]): Promise<void> {
  const names = await modern.toolNames()
  for (const n of MODERN_ONLY_TOOLS) expect(names).toContain(n)
  expect(names.filter((n) => !MODERN_ONLY_TOOLS.includes(n))).toEqual(legacyNames)
  for (const n of MODERN_ONLY_TOOLS) expect(legacyNames).not.toContain(n)
}

beforeAll(async () => {
  // M1 行为：Host 不唤醒（--waker none），App 未打开时调用返回 APP_DISCONNECTED + 启动地址（自动唤醒见 lifecycle.test.ts）
  host = await startHost(inject('hostBin'), [], { waker: 'none' })
  browser = await Browser.launch(inject('chromeBin'))
  modern = new McpClient({ url: host.mcpUrl, era: 'modern', diagnostics: () => host.log(200) })
  await modern.discover()
})

afterAll(async () => {
  await modern?.close()
  await browser?.close()
  await shop?.stop()
  await host?.stop()
})

const mcp = () => host.mcp

async function appsList(): Promise<any> {
  return data(await mcp().callTool('apps.list'))
}

async function shopApp(): Promise<any> {
  return (await appsList()).apps.find((a: { appId: string }) => a.appId === 'shop')
}

async function waitTools(pred: (names: string[]) => boolean, what: string): Promise<string[]> {
  return waitFor(async () => {
    const names = await mcp().toolNames()
    return pred(names) ? names : undefined
  }, what)
}

async function todoTitles(page: Page): Promise<string[]> {
  return page.eval(`[...document.querySelectorAll('.list li')].map((li) => li.textContent.trim())`)
}

describe('M1 验收', () => {
  it('App 未打开：静态工具可列出；调用返回 APP_DISCONNECTED 与启动地址；首次接触附带总览', async () => {
    const init = mcp().initializeResult!
    expect(init.instructions).toContain('shop（示例商城）')
    expect(init.capabilities).toHaveProperty('tools')

    const names = await mcp().toolNames()
    for (const n of ['apps.list', 'apps.select', 'apps.overview', ...STATIC_TOOLS]) expect(names).toContain(n)
    expect(names).not.toContain('shop.todos.add')

    const r = await mcp().callTool('shop.catalog.search', { keyword: '耳机' })
    expect(r.isError).toBe(true)
    const t = texts(r)
    expect(t[0]).toMatch(/^\[app-mcp\] 以下是 App「示例商城」\(shop\) 的总览/)
    expect(t[0]).toContain('<app-overview app="shop"')
    expect(t.at(-1)).toMatch(/^APP_DISCONNECTED/)
    expect(t.at(-1)).toContain('http://localhost:5173/')

    // 第二次不再附带总览
    const again = texts(await mcp().callTool('shop.info'))
    expect(again).toHaveLength(1)
    expect(again[0]).toMatch(/^APP_DISCONNECTED/)

    const app = await shopApp()
    expect(app).toMatchObject({ connected: false, launchUrl: 'http://localhost:5173/', staticToolCount: 3 })

    // modern：协商 2026-07-28，总览经 discover 的 instructions 给出、不在调用结果中附带；无 Mcp-Session-Id
    expect(modern.discoverResult!.supportedVersions).toContain('2026-07-28')
    expect(modern.discoverResult!.instructions).toContain('shop（示例商城）')
    const modernNames = await modern.toolNames()
    for (const n of ['apps.list', 'apps.select', 'apps.overview', ...STATIC_TOOLS]) expect(modernNames).toContain(n)
    const m = texts(await modern.callTool('shop.catalog.search', { keyword: '耳机' }))
    expect(m).toHaveLength(1)
    expect(m[0]).toMatch(/^APP_DISCONNECTED/)
    expect(modern.sessionIdsSeen).toEqual([])
  })

  it('打开页面：连接 Host，工具出现（tools/list_changed），调用成功', async () => {
    shop = await startShop({ VITE_APP_MCP_HOST_URL: host.wsUrl })
    const since = mcp().notifications.length
    const modernSince = modern.notifications.length
    tabA = await browser.newPage(`${shop.url}?tab=a`)
    await tabA.waitForBadge('已连接', 60_000)
    const names = await waitTools((n) => n.includes('shop.todos.add'), '待办工具出现')
    for (const n of ['shop.todos.add', 'shop.todos.toggle', 'shop.todos.remove', ...STATIC_TOOLS]) {
      expect(names).toContain(n)
    }
    expect(names).not.toContain('shop.cart.add')
    await waitFor(() => mcp().countSince('notifications/tools/list_changed', since) > 0, '收到 tools/list_changed')

    const tools = await mcp().listTools()
    const add = tools.find((t) => t.name === 'shop.todos.add')!
    expect(add.inputSchema).toMatchObject({ type: 'object', properties: { title: { type: 'string' } } })

    const app = await shopApp()
    expect(app.connected).toBe(true)
    expect(app.instances).toHaveLength(1)
    expect(app.instances[0].url).toContain('tab=a')

    // 惰性加载的静态工具（catalog.search）
    const found = ok(await mcp().callTool('shop.catalog.search', { keyword: '耳机' }))
    expect(JSON.stringify(found)).toContain('耳机')

    // modern：list_changed 经 subscriptions/listen 流送达（带 subscriptionId），列表与调用与 legacy 一致
    await waitFor(() => modern.countSince('notifications/tools/list_changed', modernSince) > 0, 'modern 收到 tools/list_changed')
    const note = modern.notifications.slice(modernSince).find((n) => n.method === 'notifications/tools/list_changed')!
    expect(note.params?._meta?.['io.modelcontextprotocol/subscriptionId']).toBe(modern.subscriptionId)
    await expectModernToolsMatch(await mcp().toolNames())
    expect(JSON.stringify(ok(await modern.callTool('shop.catalog.search', { keyword: '耳机' })))).toContain('耳机')
  })

  it('添加 3 个待办并勾选第 2 个（resources/read 与页面渲染一致）', async () => {
    const added: string[] = []
    for (const title of ['e2e 待办一', 'e2e 待办二', 'e2e 待办三']) {
      const d = ok(await mcp().callTool('shop.todos.add', { title }))
      expect(d.todo).toMatchObject({ title, done: false })
      added.push(d.todo.id)
    }
    const toggled = ok(await mcp().callTool('shop.todos.toggle', { id: added[1] }))
    expect(toggled.todo).toMatchObject({ id: added[1], done: true })

    const resources = await mcp().listResources()
    const uri = resources.find((r) => r.uri.endsWith('todos.list'))?.uri
    expect(uri).toBeDefined()
    const list = await mcp().readResource(uri!)
    expect(await modern.readResource(uri!)).toEqual(list)
    const mine = list.todos.filter((t: { id: string }) => added.includes(t.id))
    expect(mine.map((t: { title: string; done: boolean }) => [t.title, t.done])).toEqual([
      ['e2e 待办一', false],
      ['e2e 待办二', true],
      ['e2e 待办三', false],
    ])

    await waitFor(async () => (await todoTitles(tabA)).some((t) => t.includes('e2e 待办三')), '页面渲染新待办')
    const checked = await tabA.eval<boolean[]>(
      `[...document.querySelectorAll('.list li')].filter((li) => li.textContent.includes('e2e 待办')).map((li) => li.querySelector('input[type=checkbox]').checked)`,
    )
    expect(checked).toEqual([false, true, false])

    // 参数校验在 Host 侧完成
    const bad = await mcp().callTool('shop.todos.add', {})
    expect(bad.isError).toBe(true)
    expect(texts(bad).join('\n')).toMatch(/INVALID_INPUT/)
  })

  it('切换页签：工具随之变化（list_changed）', async () => {
    const since = mcp().notifications.length
    await tabA.clickTab('购物车')
    const names = await waitTools((n) => n.includes('shop.cart.add') && !n.includes('shop.todos.add'), '购物车工具替换待办工具')
    expect(names).toContain('shop.cart.removeItem')
    // 购物车为空时 checkout 被禁用，不出现在列表中
    expect(names).not.toContain('shop.cart.checkout')
    for (const n of STATIC_TOOLS) expect(names).toContain(n)
    await waitFor(() => mcp().countSince('notifications/tools/list_changed', since) > 0, '收到 tools/list_changed')

    // 已注销的工具：TOOL_NOT_FOUND 类错误
    const r = await mcp().callTool('shop.todos.add', { title: 'x' })
    expect(r.isError).toBe(true)
  })

  it('把购物车里最贵的商品删掉', async () => {
    const products = ok(await mcp().callTool('shop.catalog.search', {}))
    const list: Array<{ id: string; name: string; price: number }> = Array.isArray(products)
      ? products
      : (products.products ?? products.items ?? products.results)
    expect(list.length).toBeGreaterThanOrEqual(3)
    const picked = [...list].sort((a, b) => a.price - b.price).slice(0, 3)
    for (const p of picked) ok(await mcp().callTool('shop.cart.add', { productId: p.id, qty: 1 }))
    await waitTools((n) => n.includes('shop.cart.checkout'), '购物车非空后 checkout 可用')

    const uri = (await mcp().listResources()).find((r) => r.uri.endsWith('cart.state'))!.uri
    const cart = await mcp().readResource(uri)
    expect(cart.items).toHaveLength(3)
    const priceOf = (item: any): number => item.unitPrice ?? item.price ?? item.product?.price
    const mostExpensive = [...cart.items].sort((a: any, b: any) => priceOf(b) - priceOf(a))[0]
    const removed = ok(await mcp().callTool('shop.cart.removeItem', { itemId: mostExpensive.itemId ?? mostExpensive.id }))
    expect(removed).toBeDefined()

    const after = await mcp().readResource(uri)
    expect(after.items).toHaveLength(2)
    const maxLeft = Math.max(...after.items.map(priceOf))
    expect(maxLeft).toBeLessThanOrEqual(priceOf(mostExpensive))
    expect(after.items.some((i: any) => (i.itemId ?? i.id) === (mostExpensive.itemId ?? mostExpensive.id))).toBe(false)
    const expensiveName = picked.find((p) => p.price === priceOf(mostExpensive))!.name
    await waitFor(async () => !(await tabA.eval<string>(`document.querySelector('.grid .card:last-child')?.innerText ?? ''`)).includes(expensiveName), '页面购物车移除该商品')
  })

  it('两个标签页：调用发往聚焦的标签页；apps.select 切换目标', async () => {
    await tabA.clickTab('待办')
    tabB = await browser.newPage(`${shop.url}?tab=b`)
    await tabB.waitForBadge('已连接', 60_000)
    const app = await waitFor(async () => {
      const a = await shopApp()
      return a.instances.length === 2 ? a : undefined
    }, '两个实例')
    const idOf = (tab: string) => app.instances.find((i: { url: string }) => i.url.includes(`tab=${tab}`)).instanceId as string
    const idA = idOf('a')
    const idB = idOf('b')
    expect(idA).not.toBe(idB)

    // 两个实例共用一条 WebSocket（SharedWorker 持有，spec/protocol.md 第 9 节）
    const targets = await browser.send<{ targetInfos: { type: string }[] }>('Target.getTargets')
    expect(targets.targetInfos.filter((t) => t.type === 'shared_worker')).toHaveLength(1)
    const port = Number(new URL(host.wsUrl).port)
    const sockets = establishedTo(port)
    if (sockets !== undefined) expect(sockets).toBe(1)
    await waitTools((n) => n.includes('shop.todos.add'), '待办工具')

    const addAndLocate = async (title: string): Promise<'a' | 'b'> => {
      ok(await mcp().callTool('shop.todos.add', { title }))
      return waitFor(async () => {
        if ((await todoTitles(tabA)).some((t) => t.includes(title))) return 'a' as const
        if ((await todoTitles(tabB!)).some((t) => t.includes(title))) return 'b' as const
        return undefined
      }, `待办「${title}」出现在某个标签页`)
    }

    // B 获得焦点
    await tabA.setFocused(false)
    await tabB.setFocused(true)
    await waitFor(async () => (await shopApp()).instances.find((i: any) => i.instanceId === idB)?.focused, 'Host 看到 B 聚焦')
    expect(await addAndLocate('发往 B')).toBe('b')

    // A 获得焦点
    await tabB.setFocused(false)
    await tabA.setFocused(true)
    await waitFor(async () => (await shopApp()).instances.find((i: any) => i.instanceId === idA)?.focused, 'Host 看到 A 聚焦')
    expect(await addAndLocate('发往 A')).toBe('a')

    // apps.select 优先于焦点
    const sel = await mcp().callTool('apps.select', { appId: 'shop', instanceId: idB })
    expect(sel.isError).toBeFalsy()
    expect((await shopApp()).selectedInstanceId).toBe(idB)
    expect(await addAndLocate('选定 B')).toBe('b')

    // modern 任务句柄（S8）：同一主体的两个任务各自选择不同实例，互不影响，也不影响不带 taskId 的调用；结束后的句柄得到可恢复错误
    const begin = async () => ok(await modern.callTool('apps.task.begin')).taskId as string
    const t1 = await begin()
    const t2 = await begin()
    expect(t1).toMatch(/^task-[0-9a-f]{32}$/)
    expect(t2).not.toBe(t1)
    ok(await modern.callTool('apps.select', { appId: 'shop', instanceId: idA, taskId: t1 }))
    ok(await modern.callTool('apps.select', { appId: 'shop', instanceId: idB, taskId: t2 }))
    const selectedIn = async (taskId?: string) =>
      ok(await modern.callTool('apps.list', taskId ? { taskId } : {})).apps.find((a: any) => a.appId === 'shop').selectedInstanceId
    expect(await selectedIn(t1)).toBe(idA)
    expect(await selectedIn(t2)).toBe(idB)
    expect(await selectedIn()).toBeFalsy()
    expect(ok(await modern.callTool('apps.task.end', { taskId: t2 })).ended).toBe(true)
    const gone = await modern.callTool('apps.list', { taskId: t2 })
    expect(gone.isError).toBe(true)
    expect(gone.structuredContent.error).toMatchObject({ kind: 'INVALID_INPUT', details: { reason: 'task-expired', taskId: t2 } })
    expect(texts(gone).join('\n')).toContain('apps.task.begin')
    expect(await selectedIn(t1)).toBe(idA)
    ok(await modern.callTool('apps.task.end', { taskId: t1 }))
    // legacy 会话出示句柄被显式拒绝，其选择不变
    const legacyWithTask = await mcp().callTool('apps.list', { taskId: t1 })
    expect(legacyWithTask.structuredContent.error).toMatchObject({ kind: 'INVALID_INPUT', details: { reason: 'task-handle-unsupported' } })
    expect((await shopApp()).selectedInstanceId).toBe(idB)

    // 选定的标签页关闭后回到默认规则（A）
    await tabB.close()
    tabB = undefined
    await waitFor(async () => (await shopApp()).instances.length === 1, 'B 断开')
    expect(await addAndLocate('B 关闭后')).toBe('a')
  })

  it('关闭全部页面：运行时工具移除，静态工具仍在，调用返回 APP_DISCONNECTED', async () => {
    const modernSince = modern.notifications.length
    await tabA.close()
    const names = await waitTools((n) => !n.includes('shop.todos.add'), '运行时工具移除')
    await waitFor(() => modern.countSince('notifications/tools/list_changed', modernSince) > 0, 'modern 收到 tools/list_changed')
    await expectModernToolsMatch(names)
    expect(modern.sessionIdsSeen).toEqual([])
    for (const n of STATIC_TOOLS) expect(names).toContain(n)
    const r = await mcp().callTool('shop.catalog.search', { keyword: '' })
    expect(r.isError).toBe(true)
    expect(texts(r).at(-1)).toMatch(/^APP_DISCONNECTED/)
    expect(texts(r).join('\n')).toContain('http://localhost:5173/')
    expect((await shopApp()).connected).toBe(false)
    // 清单声明了 wake，但 waker 为 none：不尝试唤醒
    expect(host.log(500)).not.toMatch(/唤醒 App/)
  })
})
