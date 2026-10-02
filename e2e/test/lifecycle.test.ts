/**
 * 生命周期（spec/lifecycle.md）：shop 以 `VITE_APP_MCP_LIFECYCLE=idle`（短空闲时间）启动 →
 * 空闲休眠 → Host 显示 dormant、工具仍列出 → 调用触发唤醒 → 快速恢复后结果正确 → 再次休眠；
 * 调用其他页面的工具时唤醒后先导航（第 4c 项，spec/hub-api.md 3.14）。
 *
 * 唤醒方式：Host 配置 `--waker {"exec": [node, src/record-wake.mjs, 日志]}`，唤醒程序只记录 WakeRequest；
 * 测试读到请求后按 web-url 唤醒的约定把 `#app-mcp-wake=<令牌>` 交给页面（新开，或设置到已打开页面的
 * hash 上，相当于浏览器把该地址交给现有标签页），页面经 hashchange 带令牌回连。
 */
import { afterAll, beforeAll, describe, expect, inject, it } from 'vitest'
import { Browser, type Page } from '../src/browser'
import { type HostHandle, type ShopServer, startHost, startShop } from '../src/env'
import { data, ok } from '../src/mcp-client'
import { sleep, waitFor } from '../src/util'

const IDLE_MS = 1500

let host: HostHandle
let shop: ShopServer
let browser: Browser
let page: Page

beforeAll(async () => {
  // 租约缩短到 500 ms：调用完成后很快允许再次休眠
  host = await startHost(inject('hostBin'), ['--lease-ms', '500', '--wake-timeout-ms', '20000'])
  shop = await startShop({
    VITE_APP_MCP_HOST_URL: host.wsUrl,
    VITE_APP_MCP_LIFECYCLE: 'idle',
    VITE_APP_MCP_IDLE_MS: String(IDLE_MS),
    VITE_APP_MCP_HIDDEN_IDLE_MS: String(IDLE_MS),
  })
  browser = await Browser.launch(inject('chromeBin'))
})

afterAll(async () => {
  await browser?.close()
  await shop?.stop()
  await host?.stop()
})

const mcp = () => host.mcp

async function shopApp(): Promise<any> {
  return data(await mcp().callTool('apps.list')).apps.find((a: { appId: string }) => a.appId === 'shop')
}

async function waitDormant(): Promise<any> {
  const app = await waitFor(async () => {
    const a = await shopApp()
    return a.dormant && a.dormantInstances.length === 1 ? a : undefined
  }, 'Host 显示休眠', 20_000, 200)
  await page.waitForBadge('休眠中')
  return app
}

describe('生命周期：idle 休眠与调用唤醒', () => {
  it('App 未打开：按清单 wake（web-url）冷启动，页面带令牌打开后完成调用', async () => {
    expect((await shopApp()).connected).toBe(false)
    const call = mcp().callTool('shop.catalog.search', { keyword: '耳机' }, 60_000)
    const req = await waitFor(() => host.wakeRequests()[0], 'Host 按清单执行 web-url 唤醒', 20_000, 50)
    // 冷启动：没有休眠实例，唤醒描述来自清单（地址是 http://localhost:5173/；测试服务器端口不同，带令牌打开测试地址）
    expect(req).toMatchObject({ appId: 'shop', instanceId: null, descriptor: { kind: 'web-url', target: 'http://localhost:5173/' } })
    expect(req.token).toMatch(/^[\w-]+$/)
    expect(req.activationArg).toBe(`app-mcp-wake:${req.token}`)
    page = await browser.newPage(`${shop.url}#app-mcp-wake=${req.token}`)
    const r = await call
    const found = ok(r)
    expect(JSON.stringify(found)).toContain('耳机')
    await page.waitForBadge('已连接')
  })

  it('空闲后休眠：Host 显示 dormant，工具仍列出且不发 list_changed', async () => {
    await waitFor(async () => (await mcp().toolNames()).includes('shop.todos.add'), '待办工具出现')
    await mcp().quiet()
    const since = mcp().notifications.length

    const app = await waitDormant()
    expect(app.connected).toBe(false)
    expect(app.instances).toHaveLength(0)
    expect(app.dormantInstances[0]).toMatchObject({ wake: 'web-url' })
    expect(app.dormantInstances[0].tools).toContain('todos.add')

    const names = await mcp().toolNames()
    expect(names).toContain('shop.todos.add')
    expect(mcp().countSince('notifications/tools/list_changed', since)).toBe(0)
  })

  it('调用休眠实例的工具：Host 唤醒（web-url）→ 页面带令牌回连 → 结果正确 → 再次休眠', async () => {
    const before = host.wakeRequests().length
    const call = mcp().callTool('shop.todos.add', { title: '唤醒后添加' }, 40_000)

    // Host 调用唤醒程序（记录 WakeRequest）→ 令牌交给已打开的页面
    const req = await waitFor(() => host.wakeRequests()[before], 'Host 执行 web-url 唤醒', 20_000, 50)
    // 休眠实例：唤醒描述来自页面休眠时上报的 wake（当前页面地址）
    expect(req.instanceId).toBeTruthy()
    expect(req.descriptor).toMatchObject({ kind: 'web-url', target: shop.url })
    await page.eval(`location.hash = ${JSON.stringify(`#app-mcp-wake=${req.token}`)}`)

    const result = ok(await call)
    expect(result.todo).toMatchObject({ title: '唤醒后添加', done: false })
    await waitFor(async () => (await page.text()).includes('唤醒后添加'), '页面渲染新待办')
    // 令牌读取后从地址栏移除
    await waitFor(async () => !(await page.eval<string>('location.href')).includes('app-mcp-wake'), '地址栏移除唤醒令牌')

    const app = await shopApp()
    expect(app.instances).toHaveLength(1)
    expect(app.dormantInstances).toHaveLength(0)

    // 租约到期、空闲后再次休眠
    await waitDormant()
  })

  it('休眠中调用静态工具（惰性 handler）同样唤醒', async () => {
    const before = host.wakeRequests().length
    const call = mcp().callTool('shop.catalog.search', { keyword: '耳机' }, 40_000)
    const req = await waitFor(() => host.wakeRequests()[before], 'Host 执行 web-url 唤醒', 20_000, 50)
    await page.eval(`location.hash = ${JSON.stringify(`#app-mcp-wake=${req.token}`)}`)
    const result = ok(await call)
    expect(JSON.stringify(result)).toContain('耳机')
    await waitDormant()
  })

  it('休眠中调用其他页面的工具（页面目录）：唤醒 → 导航到该页面 → 等工具注册后派发', async () => {
    expect(await page.eval<string>('location.pathname')).toBe('/')
    const before = host.wakeRequests().length
    const call = mcp().callTool('shop.orders.list', {}, 40_000)
    const req = await waitFor(() => host.wakeRequests()[before], 'Host 执行 web-url 唤醒', 20_000, 50)
    await page.eval(`location.hash = ${JSON.stringify(`#app-mcp-wake=${req.token}`)}`)
    expect(ok(await call)).toEqual({ orders: [] })
    expect(await page.eval<string>('location.pathname')).toBe('/orders')
    // 休眠快照中的 view 工具不在 tools/list（spec/hub-api.md 3.14 L1），页面目录仍可经 apps.page 查到
    await waitDormant()
    expect(await mcp().toolNames()).not.toContain('shop.orders.list')
    const orders = data(await mcp().callTool('apps.page', { appId: 'shop', page: 'orders' }))
    expect(orders.tools.map((t: { name: string }) => t.name)).toEqual(['shop.orders.list'])
  })

  it('页面重新可见时自行回连（不经 Host 唤醒）', async () => {
    const wakes = host.wakeRequests().length
    await page.setVisible(false)
    await sleep(100)
    await page.setVisible(true)
    await waitFor(async () => (await shopApp()).instances.length === 1, 'Host 看到回连')
    expect(host.wakeRequests()).toHaveLength(wakes)
    const r = ok(await mcp().callTool('shop.info'))
    expect(r).toBeDefined()
    await waitDormant()
  })
})
