/**
 * 界面级暴露与页面渐进披露（第 4c 项，spec/protocol.md 3.4、spec/hub-api.md 3.14）：shop 多页面
 * （待办 / 商品（keep-alive）/ 购物车 / 订单）+ 商品详情对话框（ToolLayer）。
 *
 * 各用例按顺序共用同一个 Host / 页面状态。休眠实例被唤醒后导航见 lifecycle.test.ts。
 */
import { afterAll, beforeAll, describe, expect, inject, it } from 'vitest'
import { Browser, type Page } from '../src/browser'
import { type HostHandle, type ShopServer, startHost, startShop } from '../src/env'
import { data, ok, texts } from '../src/mcp-client'
import { waitFor } from '../src/util'

let host: HostHandle
let shop: ShopServer
let browser: Browser
let page: Page

const PRODUCT_TOOLS = ['shop.products.filter', 'shop.products.open']
const DIALOG_TOOLS = ['shop.productDialog.addToCart', 'shop.productDialog.close']

beforeAll(async () => {
  host = await startHost(inject('hostBin'), [], { waker: 'none' })
  shop = await startShop({ VITE_APP_MCP_HOST_URL: host.wsUrl })
  browser = await Browser.launch(inject('chromeBin'))
  page = await browser.newPage(shop.url)
  await page.waitForBadge('已连接', 60_000)
})

afterAll(async () => {
  await browser?.close()
  await shop?.stop()
  await host?.stop()
})

const mcp = () => host.mcp

async function waitTools(pred: (names: string[]) => boolean, what: string): Promise<string[]> {
  return waitFor(async () => {
    const names = await mcp().toolNames()
    return pred(names) ? names : undefined
  }, what)
}

const has = (names: string[], list: string[]) => list.every((n) => names.includes(n))
const none = (names: string[], list: string[]) => list.every((n) => !names.includes(n))
const pathname = () => page.eval<string>('location.pathname')

async function shopApp(): Promise<any> {
  return data(await mcp().callTool('apps.list')).apps.find((a: { appId: string }) => a.appId === 'shop')
}

describe('页面目录与渐进披露', () => {
  it('清单页面目录：apps.list 带页面数与导航能力，apps.page 返回页面工具；未打开页面的工具不在 tools/list', async () => {
    const app = await waitFor(async () => {
      const a = await shopApp()
      return a?.instances?.length === 1 ? a : undefined
    }, '实例连接')
    expect(app.pageCount).toBe(3)
    expect(app.instances[0].navigation).toBe(true)

    const names = await mcp().toolNames()
    expect(names).toContain('apps.page')
    expect(names).toContain('shop.todos.add')
    expect(none(names, ['shop.orders.list', 'shop.cart.add', ...PRODUCT_TOOLS])).toBe(true)

    const orders = data(await mcp().callTool('apps.page', { appId: 'shop', page: 'orders' }))
    expect(orders.page).toMatchObject({ name: 'orders', route: '/orders', navigable: true, current: false })
    expect(orders.tools.map((t: { name: string }) => t.name)).toEqual(['shop.orders.list'])
  })

  it('切换页面：工具列表随之变化；keep-alive 页面隐藏时其 view 工具暂停', async () => {
    await page.clickTab('商品')
    let names = await waitTools((n) => has(n, PRODUCT_TOOLS) && !n.includes('shop.todos.add'), '商品页工具出现、待办工具移除')
    expect(names).not.toContain('shop.orders.list')

    const filtered = ok(await mcp().callTool('shop.products.filter', { keyword: '耳机' }))
    expect(filtered.products.map((p: { name: string }) => p.name)).toEqual(['降噪耳机'])

    await page.clickTab('订单')
    names = await waitTools((n) => n.includes('shop.orders.list') && none(n, PRODUCT_TOOLS), '订单页工具出现、商品页工具暂停')
    // keep-alive：商品页仍挂载（隐藏），筛选条件保留
    expect(await page.eval<boolean>(`document.querySelector('[data-route="/products"]')?.hidden === true`)).toBe(true)

    await page.clickTab('商品')
    await waitTools((n) => has(n, PRODUCT_TOOLS) && !n.includes('shop.orders.list'), '回到商品页，工具恢复')
    expect(await page.eval<string>(`document.querySelector('[aria-label="筛选商品"]').value`)).toBe('耳机')
  })

  it('打开对话框（ToolLayer）：下层 view 工具暂停，只有对话框工具；对话框打开时导航被拒绝', async () => {
    ok(await mcp().callTool('shop.products.open', { productId: 'p3' }))
    await waitTools((n) => has(n, DIALOG_TOOLS) && none(n, PRODUCT_TOOLS), '对话框工具出现、商品页工具暂停')
    // app 工具不受层级影响
    expect(await mcp().toolNames()).toContain('shop.catalog.search')

    const added = ok(await mcp().callTool('shop.productDialog.addToCart', { qty: 2 }))
    expect(added.cart.items).toEqual([expect.objectContaining({ productId: 'p3', qty: 2 })])

    // 调用其他页面的工具：App 以 NAVIGATION_DENIED 拒绝（用户正在与弹层交互），不切换页面
    const denied = await mcp().callTool('shop.orders.list')
    expect(denied.isError).toBe(true)
    expect(texts(denied).join('\n')).toMatch(/^NAVIGATION_DENIED/m)
    expect(texts(denied).join('\n')).toContain('商品详情')
    expect(await pathname()).toBe('/products')

    // 无返回值：Host 输出「已完成」
    expect(texts(await mcp().callTool('shop.productDialog.close'))).toEqual(['已完成'])
    await waitTools((n) => has(n, PRODUCT_TOOLS) && none(n, DIALOG_TOOLS), '对话框关闭，商品页工具恢复')
  })

  it('调用不在当前页面的工具：Host 请求导航，等目标工具注册后派发', async () => {
    await page.clickTab('待办')
    await waitTools((n) => n.includes('shop.todos.add') && none(n, PRODUCT_TOOLS), '回到待办页')

    const orders = ok(await mcp().callTool('shop.orders.list'))
    expect(orders).toEqual({ orders: [] })
    expect(await pathname()).toBe('/orders')

    // zod 输入的页面工具（清单中显式声明）：从订单页导航到购物车
    const cart = ok(await mcp().callTool('shop.cart.add', { productId: 'p1', qty: 1 }))
    expect(cart.cart.items.map((i: { productId: string }) => i.productId)).toEqual(['p3', 'p1'])
    expect(await pathname()).toBe('/cart')

    // keep-alive 页面：导航后恢复可见，暂停的工具重新启用
    const filtered = ok(await mcp().callTool('shop.products.filter', { keyword: '' }))
    expect(filtered.products).toHaveLength(6)
    expect(await pathname()).toBe('/products')

    // 页面工具结算后订单页可见新订单
    ok(await mcp().callTool('shop.cart.checkout', { addressId: 'home' }))
    expect(await pathname()).toBe('/cart')
    const after = ok(await mcp().callTool('shop.orders.list'))
    expect(after.orders).toHaveLength(1)
    expect(await pathname()).toBe('/orders')
  })

  it('页面隐藏（切到后台标签页）时 view 工具暂停，app 工具保留', async () => {
    await page.setVisible(false)
    await waitTools((n) => !n.includes('shop.orders.list') && n.includes('shop.catalog.search'), '页面隐藏，view 工具暂停')
    await page.setVisible(true)
    await waitTools((n) => n.includes('shop.orders.list'), '页面可见，view 工具恢复')
  })
})
