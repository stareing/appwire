/**
 * 网页唤醒交接（spec/lifecycle.md 第 5 节「Web 唤醒交接」，TASKS 4f G9）：
 * 原标签页隐藏后休眠 → 调用触发 web-url 唤醒 → 浏览器以唤醒地址新开标签页 →
 * 新标签页把令牌交给原标签页 → 原标签页回连并执行调用 → 新标签页自行关闭（关不掉时显示提示）。
 *
 * 唤醒方式同 lifecycle.test.ts：Host 只记录 WakeRequest，测试以唤醒地址直接新建标签页（等同操作系统打开 URL）。
 */
import { afterAll, beforeAll, describe, expect, inject, it } from 'vitest'
import { Browser, type Page } from '../src/browser'
import { type HostHandle, type ShopServer, startHost, startShop } from '../src/env'
import { data, ok } from '../src/mcp-client'
import { waitFor } from '../src/util'

const IDLE_MS = 1500

let host: HostHandle
let shop: ShopServer
let browser: Browser
let page: Page

beforeAll(async () => {
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

async function shopApp(): Promise<any> {
  return data(await host.mcp.callTool('apps.list')).apps.find((a: { appId: string }) => a.appId === 'shop')
}

describe('网页唤醒交接', () => {
  it('隐藏的休眠标签页：唤醒地址新开的标签页把令牌交给它，原标签页回连执行调用，新标签页关闭或提示', async () => {
    page = await browser.newPage(shop.url)
    await page.waitForBadge('已连接')
    await page.setVisible(false)
    await waitFor(async () => {
      const a = await shopApp()
      return a?.dormant && a.dormantInstances.length === 1 ? a : undefined
    }, 'Host 显示休眠', 20_000, 200)

    const call = host.mcp.callTool('shop.todos.add', { title: '交接唤醒' }, 40_000)
    const req = await waitFor(() => host.wakeRequests()[0], 'Host 执行 web-url 唤醒', 20_000, 50)
    expect(req.descriptor).toMatchObject({ kind: 'web-url', target: shop.url })
    const opened = await browser.openTarget(`${shop.url}#app-mcp-wake=${req.token}`)

    const result = ok(await call)
    expect(result.todo).toMatchObject({ title: '交接唤醒' })
    // 调用由原（隐藏）标签页执行
    await waitFor(async () => (await page.text()).includes('交接唤醒'), '原标签页渲染新待办')

    // 新标签页：能关则已关闭；关不掉时显示提示且地址栏已移除令牌
    let tab: Page | undefined
    const outcome = await waitFor(async () => {
      if (!(await browser.targetExists(opened))) return 'closed'
      tab ??= await browser.attach(opened)
      const notice = await tab.eval<string | null>(`document.querySelector('[data-app-mcp-handoff]')?.textContent ?? null`)
      if (notice === null) return undefined
      expect(await tab.eval<string>('location.href')).not.toContain('app-mcp-wake')
      return 'notice'
    }, '新标签页关闭或显示提示', 10_000, 100)
    expect(['closed', 'notice']).toContain(outcome)

    // 只有原标签页这一个实例（新标签页没有带令牌另起实例）
    const app = await shopApp()
    expect(app.instances).toHaveLength(1)
    expect(app.instances[0].instanceId).toBe(req.instanceId)
  })
})
