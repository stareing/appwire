/**
 * 生命周期（spec/lifecycle.md）：shop 以 `VITE_APP_MCP_LIFECYCLE=idle`（短空闲时间）启动 →
 * 空闲休眠 → Host 显示 dormant、工具仍列出 → 调用触发唤醒 → 快速恢复后结果正确 → 再次休眠。
 *
 * 唤醒方式：Host 的默认 SystemWaker 对 web-url 执行 `xdg-open <页面地址>#app-mcp-wake=<令牌>`。
 * 测试给 Host 的 PATH 前置一个替身 `xdg-open`，只记录 URL；测试读到 URL 后把其中的 hash 设置到
 * 已打开的页面上（相当于浏览器把该地址交给现有标签页），页面经 hashchange 带令牌回连。
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
    const wakeUrl = await waitFor(() => host.wakeUrls()[0], 'Host 按清单执行 web-url 唤醒', 20_000, 50)
    const url = new URL(wakeUrl)
    // 清单中的地址是 http://localhost:5173/；测试服务器端口不同，保留令牌打开测试地址
    expect(`${url.origin}${url.pathname}`).toBe('http://localhost:5173/')
    expect(url.hash).toMatch(/^#app-mcp-wake=[\w-]+$/)
    page = await browser.newPage(`${shop.url}${url.hash}`)
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
    const before = host.wakeUrls().length
    const call = mcp().callTool('shop.todos.add', { title: '唤醒后添加' }, 40_000)

    // Host 执行唤醒命令（替身 xdg-open 记录 URL）→ 交给已打开的页面
    const wakeUrl = await waitFor(() => host.wakeUrls()[before], 'Host 执行 web-url 唤醒', 20_000, 50)
    const url = new URL(wakeUrl)
    expect(`${url.origin}${url.pathname}`).toBe(shop.url)
    expect(url.hash).toMatch(/^#app-mcp-wake=/)
    await page.eval(`location.hash = ${JSON.stringify(url.hash)}`)

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
    const before = host.wakeUrls().length
    const call = mcp().callTool('shop.catalog.search', { keyword: '耳机' }, 40_000)
    const wakeUrl = await waitFor(() => host.wakeUrls()[before], 'Host 执行 web-url 唤醒', 20_000, 50)
    await page.eval(`location.hash = ${JSON.stringify(new URL(wakeUrl).hash)}`)
    const result = ok(await call)
    expect(JSON.stringify(result)).toContain('耳机')
    await waitDormant()
  })

  it('页面重新可见时自行回连（不经 Host 唤醒）', async () => {
    const wakes = host.wakeUrls().length
    await page.setVisible(false)
    await sleep(100)
    await page.setVisible(true)
    await waitFor(async () => (await shopApp()).instances.length === 1, 'Host 看到回连')
    expect(host.wakeUrls()).toHaveLength(wakes)
    const r = ok(await mcp().callTool('shop.shop.info'))
    expect(r).toBeDefined()
    await waitDormant()
  })
})
