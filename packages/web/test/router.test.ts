/** 路由适配共用部分：页面地址生成、路由表 → 页面，以及 Vue Router 适配（结构化假路由）。 */
import { describe, expect, it, vi } from 'vitest'
import { pagePath, routePages } from '../src/router'
import { bindVueRouter, type VueRouteLocationLike, type VueRouterLike } from '../src/router/vue-router'
import { ToolCallError, type AppMcp, type NavigationHandler } from '../src/types'

describe('pagePath', () => {
  it.each([
    ['/cart', {}, '/cart'],
    ['/orders/:id', { id: 'A 1' }, '/orders/A%201'],
    ['/orders/:id?', {}, '/orders'],
    ['/files/*', { '*': 'a/b c' }, '/files/a/b%20c'],
    ['/tags/:tags+', { tags: ['x', 'y'] }, '/tags/x/y'],
    ['/u/:id(\\d+)', { id: 7 }, '/u/7'],
    ['/search', { q: '耳机', page: 2 }, '/search?q=%E8%80%B3%E6%9C%BA&page=2'],
    ['/', {}, '/'],
  ])('%s + %j → %s', (pattern, params, path) => {
    expect(pagePath(pattern, params)).toBe(path)
  })

  it('缺少必填参数 → NAVIGATION_FAILED', () => {
    try {
      pagePath('/orders/:id', {}, 'orders')
      expect.unreachable()
    } catch (e) {
      expect(e).toBeInstanceOf(ToolCallError)
      expect((e as ToolCallError).kind).toBe('NAVIGATION_FAILED')
      expect((e as ToolCallError).message).toContain('id')
    }
  })
})

describe('routePages', () => {
  it('带 id 的路由是页面，子路由拼接父路径，index 取父路径', () => {
    const pages = routePages([
      { path: '/', children: [{ index: true, id: 'home' }, { path: 'cart', id: 'cart' }] },
      { path: '/orders', id: 'orders', children: [{ path: ':id', id: 'orders.detail' }, { path: '/abs', id: 'abs' }] },
      { path: '/about' },
    ])
    expect(pages).toEqual({ home: '/', cart: '/cart', orders: '/orders', 'orders.detail': '/orders/:id', abs: '/abs' })
  })

  it('重复 id 抛错', () => {
    expect(() => routePages([{ path: '/a', id: 'x' }, { path: '/b', id: 'x' }])).toThrow(/重复/)
  })
})

describe('bindVueRouter', () => {
  function fakeApp(): { app: AppMcp; handler: () => NavigationHandler | null } {
    let current: NavigationHandler | null = null
    const app = {
      options: { appId: 'a', appName: 'A' },
      setNavigationHandler: (h: NavigationHandler | null) => {
        current = h
      },
    } as unknown as AppMcp
    return { app, handler: () => current }
  }

  function fakeRouter(result: unknown = undefined): VueRouterLike & { pushed: VueRouteLocationLike[] } {
    const pushed: VueRouteLocationLike[] = []
    return {
      pushed,
      push: async (to) => {
        pushed.push(to)
        return result
      },
      hasRoute: (name) => ['cart', 'order'].includes(name),
      getRoutes: () => [
        { name: 'cart', path: '/cart' },
        { name: 'order', path: '/orders/:id' },
      ],
    }
  }

  it('页面名 = 路由 name；路径参数进 params，其余进 query；解除后清除回调', async () => {
    const { app, handler } = fakeApp()
    const router = fakeRouter()
    const unbind = bindVueRouter(app, router)
    await handler()!({ page: 'order', params: { id: 42, tab: 'items' } })
    expect(router.pushed).toEqual([{ name: 'order', params: { id: '42' }, query: { tab: 'items' } }])
    unbind()
    expect(handler()).toBeNull()
  })

  it('pages 表映射页面名；不在表中的页面失败', async () => {
    const { app, handler } = fakeApp()
    const router = fakeRouter()
    bindVueRouter(app, router, { pages: { checkout: 'cart' } })
    await handler()!({ page: 'checkout', params: undefined })
    expect(router.pushed).toEqual([{ name: 'cart' }])
    await expect(handler()!({ page: 'cart', params: undefined })).rejects.toMatchObject({ kind: 'NAVIGATION_FAILED' })
  })

  it.each([
    [{ type: 4 }, 'NAVIGATION_DENIED'],
    [{ type: 8 }, 'NAVIGATION_FAILED'],
  ])('push 返回失败 %j → %s；重复导航（16）视为成功', async (failure, kind) => {
    const { app, handler } = fakeApp()
    bindVueRouter(app, fakeRouter(failure))
    await expect(handler()!({ page: 'cart', params: undefined })).rejects.toMatchObject({ kind })
    const dup = fakeApp()
    bindVueRouter(dup.app, fakeRouter({ type: 16 }))
    await expect(dup.handler()!({ page: 'cart', params: undefined })).resolves.toBeUndefined()
  })

  it('guard 拒绝 → NAVIGATION_DENIED，不跳转', async () => {
    const { app, handler } = fakeApp()
    const router = fakeRouter()
    bindVueRouter(app, router, { guard: ({ page }) => (page === 'cart' ? '结算中，不能离开' : true) })
    await expect(handler()!({ page: 'cart', params: undefined })).rejects.toMatchObject({
      kind: 'NAVIGATION_DENIED',
      message: '结算中，不能离开',
    })
    expect(router.pushed).toEqual([])
  })

  it('不支持导航的实现：警告并返回空操作', () => {
    const warn = vi.fn()
    const app = { options: { appId: 'a', appName: 'A', logger: { warn, debug: vi.fn(), error: vi.fn() } } } as unknown as AppMcp
    const unbind = bindVueRouter(app, fakeRouter())
    expect(warn).toHaveBeenCalled()
    unbind()
  })
})
