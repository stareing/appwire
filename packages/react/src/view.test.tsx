/** 界面级暴露（spec/protocol.md 3.4）：ToolScope 界面声明、ToolLayer 层栈、useTool 的 surface / page、路由导航适配。 */
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { openViewLayers } from '@app-mcp/web'
import { AppMcpProvider, ToolLayer, ToolScope, useNavigationHandler, useResource, useRouterNavigation, useTool } from './index'
import { FakeAppMcp } from './testing/fake-app-mcp'

afterEach(() => {
  cleanup()
  for (const layer of [...openViewLayers()]) layer.close()
})

function setup(ui: ReactNode) {
  const app = new FakeAppMcp()
  const wrap = (node: ReactNode) => <AppMcpProvider value={app}>{node}</AppMcpProvider>
  const result = render(wrap(ui))
  return { app, ...result, rerender: (node: ReactNode) => result.rerender(wrap(node)) }
}

function Tool({ name, surface }: { name: string; surface?: 'app' | 'view' }) {
  useTool(name, { description: name, ...(surface && { surface }), handler: () => null })
  return null
}

describe('ToolScope 界面声明', () => {
  it('anchor（ref）/ page / surface 作为其下工具的缺省声明', () => {
    function Page() {
      const ref = useRef<HTMLElement>(null)
      return (
        <section ref={ref} id="cart-root">
          <ToolScope name="cart" anchor={ref} page="cart" surface="view">
            <Tool name="cart.add" />
          </ToolScope>
        </section>
      )
    }
    const { app } = setup(<Page />)
    const options = app.getScopeOptions('cart.add')!
    expect(options).toMatchObject({ page: 'cart', surface: 'view' })
    expect((options.anchor as () => Element | null)()).toBe(document.getElementById('cart-root'))
  })

  it('不给界面声明时不传 options（与之前相同）', () => {
    const { app } = setup(
      <ToolScope name="plain">
        <Tool name="x" />
      </ToolScope>,
    )
    expect(app.getScopeOptions('x')).toEqual({})
  })
})

describe('ToolLayer', () => {
  it('挂载期间打开层（早于子组件注册），卸载时关闭；层内工具缺省 surface view', () => {
    const seen: number[] = []
    function Inside() {
      useEffect(() => {
        seen.push(openViewLayers().length)
      }, [])
      return <Tool name="dialog.ok" />
    }
    const { app, rerender } = setup(
      <ToolLayer name="确认">
        <Inside />
      </ToolLayer>,
    )
    expect(seen).toEqual([1])
    expect(openViewLayers().map((l) => l.name)).toEqual(['确认'])
    const options = app.getScopeOptions('dialog.ok')!
    expect(options.surface).toBe('view')
    expect(options.layer?.isTop).toBe(true)
    rerender(<div />)
    expect(openViewLayers()).toEqual([])
    expect(app.toolNames()).toEqual([])
  })

  it('open={false} 时不压栈；嵌套层后打开的在上', () => {
    function Layers({ inner }: { inner: boolean }) {
      return (
        <ToolLayer name="外层">
          <Tool name="outer.t" />
          <ToolLayer name="内层" open={inner}>
            <Tool name="inner.t" />
          </ToolLayer>
        </ToolLayer>
      )
    }
    const { app, rerender } = setup(<Layers inner={false} />)
    expect(openViewLayers().map((l) => l.name)).toEqual(['外层'])
    rerender(<Layers inner />)
    expect(openViewLayers().map((l) => l.name)).toEqual(['外层', '内层'])
    expect(app.getScopeOptions('inner.t')?.layer?.isTop).toBe(true)
    expect(app.getScopeOptions('outer.t')?.layer?.isTop).toBe(false)
  })
})

describe('useTool 的 surface / page', () => {
  it('声明随定义注册，变化时 update', () => {
    function T({ page }: { page: string }) {
      useTool('t', { description: 't', surface: 'view', page, visibility: 'always', handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T page="a" />)
    expect(app.getTool('t')).toMatchObject({ surface: 'view', page: 'a', visibility: 'always' })
    rerender(<T page="b" />)
    expect(app.getTool('t')).toMatchObject({ page: 'b' })
    expect(app.count('tool.update', 't')).toBe(1)
  })
})

describe('useTool 的调用调度声明', () => {
  it('concurrency / exclusive 随定义注册，变化时 update（移除即清除）', () => {
    function T({ exclusive }: { exclusive?: string }) {
      useTool('t', { description: 't', concurrency: 2, ...(exclusive && { exclusive }), handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T exclusive="doc" />)
    expect(app.getTool('t')).toMatchObject({ concurrency: 2, exclusive: 'doc' })
    rerender(<T exclusive="doc" />)
    expect(app.count('tool.update', 't')).toBe(0)
    rerender(<T />)
    expect(app.getTool('t')?.exclusive).toBeUndefined()
    expect(app.getTool('t')).toMatchObject({ concurrency: 2 })
    expect(app.count('tool.update', 't')).toBe(1)
  })
})

describe('useTool 的标准意图声明', () => {
  it('implements 随定义注册；内联数组内容不变不 update，变化 / 移除时 update', () => {
    function T({ verbs }: { verbs?: string[] }) {
      useTool('t', { description: 't', ...(verbs && { implements: [...verbs] }), handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T verbs={['message.send@1']} />)
    expect(app.getTool('t')?.implements).toEqual(['message.send@1'])
    rerender(<T verbs={['message.send@1']} />)
    expect(app.count('tool.update', 't')).toBe(0)
    rerender(<T verbs={['message.send@1', 'link.open@1']} />)
    expect(app.getTool('t')?.implements).toEqual(['message.send@1', 'link.open@1'])
    rerender(<T />)
    expect(app.getTool('t')?.implements).toBeUndefined()
    expect(app.count('tool.update', 't')).toBe(2)
  })
})

describe('结果缓存声明 cache', () => {
  it('useTool：随定义注册；内联对象内容不变不 update，变化 / 移除时 update', () => {
    function T({ ttlMs, scope }: { ttlMs?: number; scope?: 'shared' }) {
      useTool('t', { description: 't', risk: 'read', ...(ttlMs && { cache: { ttlMs, ...(scope && { scope }) } }), handler: () => null })
      return null
    }
    const { app, rerender } = setup(<T ttlMs={5000} />)
    expect(app.getTool('t')?.cache).toEqual({ ttlMs: 5000 })
    rerender(<T ttlMs={5000} />)
    expect(app.count('tool.update', 't')).toBe(0)
    rerender(<T ttlMs={5000} scope="shared" />)
    expect(app.getTool('t')?.cache).toEqual({ ttlMs: 5000, scope: 'shared' })
    rerender(<T />)
    expect(app.getTool('t')?.cache).toBeUndefined()
    expect(app.count('tool.update', 't')).toBe(2)
  })

  it('useResource：随定义注册；值不变不重新注册，变化时重新注册', () => {
    function R({ ttlMs, scope }: { ttlMs?: number; scope?: 'shared' }) {
      useResource('r', { description: 'r', ...(ttlMs && { cache: { ttlMs, ...(scope && { scope }) } }), read: () => 1 })
      return null
    }
    const { app, rerender } = setup(<R ttlMs={30000} />)
    expect(app.getResource('r')?.cache).toEqual({ ttlMs: 30000 })
    rerender(<R ttlMs={30000} />)
    expect(app.count('resource.register', 'r')).toBe(1)
    rerender(<R ttlMs={30000} scope="shared" />)
    expect(app.getResource('r')?.cache).toEqual({ ttlMs: 30000, scope: 'shared' })
    rerender(<R />)
    expect(app.getResource('r')?.cache).toBeUndefined()
    expect(app.count('resource.register', 'r')).toBe(3)
  })
})

describe('路由导航适配', () => {
  it('useRouterNavigation：页面表 → navigate(地址)；参数填入路由，其余进查询串', async () => {
    const navigate = vi.fn()
    function Root() {
      useRouterNavigation({ navigate, pages: { orders: '/orders/:id', cart: '/cart' } })
      return null
    }
    const { app, unmount } = setup(<Root />)
    await app.navigate('orders', { id: 'A1', tab: 'items' })
    expect(navigate).toHaveBeenLastCalledWith('/orders/A1?tab=items')
    await app.navigate('cart')
    expect(navigate).toHaveBeenLastCalledWith('/cart')
    await expect(app.navigate('nowhere')).rejects.toMatchObject({ kind: 'NAVIGATION_FAILED' })
    await expect(app.navigate('orders')).rejects.toMatchObject({ kind: 'NAVIGATION_FAILED' })
    unmount()
    expect(app.navigationHandler).toBeNull()
  })

  it('路由表（带 id 的路由是页面）与 guard 拒绝', async () => {
    const navigate = vi.fn(async () => {})
    const routes = [{ path: '/', children: [{ index: true, id: 'home' }, { path: 'orders', id: 'orders' }] }]
    function Root() {
      useRouterNavigation({ navigate, pages: routes, guard: ({ page }) => page !== 'orders' || '订单页需要登录' })
      return null
    }
    const { app } = setup(<Root />)
    await app.navigate('home')
    expect(navigate).toHaveBeenCalledWith('/')
    await expect(app.navigate('orders')).rejects.toMatchObject({ kind: 'NAVIGATION_DENIED', message: '订单页需要登录' })
  })

  it('useNavigationHandler 始终调用最新闭包，并传递选项', async () => {
    const calls: string[] = []
    function Root() {
      const [label, setLabel] = useState('v1')
      useNavigationHandler(({ page }) => void calls.push(`${label}:${page}`), { whileLayerOpen: 'allow' })
      return <button onClick={() => setLabel('v2')}>next</button>
    }
    const { app, getByText } = setup(<Root />)
    expect(app.navigationOptions).toEqual({ whileLayerOpen: 'allow' })
    await app.navigate('a')
    act(() => getByText('next').click())
    await app.navigate('b')
    expect(calls).toEqual(['v1:a', 'v2:b'])
  })
})
