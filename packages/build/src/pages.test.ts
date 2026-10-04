/** 页面目录（第 4c 项 C）：清单 pages 的生成与校验、路由扫描（React Router / Vue Router）、插件合并显式页面。 */
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { build } from 'vite'
import { afterAll, describe, expect, it } from 'vitest'
import { z } from 'zod'
import { appMcp, definePages, generateManifest, ManifestError, mergePages, validateManifest, type AppMcpManifest } from './index'
import { scanRoutes } from './routes'
import { createTempProjects } from './testing/temp-projects'

const projects = createTempProjects()
afterAll(() => projects.cleanup())

const INFO = { appId: 'shop', name: '示例商城' }

describe('清单 pages', () => {
  it('转换 params 与页面工具（zod / JSON Schema），只写出声明的字段', () => {
    const manifest = generateManifest(INFO, [], {}, definePages([
      {
        name: 'orders.detail',
        title: '订单详情',
        route: '/orders/:id',
        params: z.object({ id: z.string() }),
        navigable: false,
        tools: [{ name: 'orders.refund', description: '退款', surface: 'view', input: z.object({ reason: z.string() }) }],
      },
      { name: 'cart', description: '购物车' },
    ]))
    expect(manifest.pages).toEqual([
      {
        name: 'orders.detail',
        title: '订单详情',
        route: '/orders/:id',
        params: expect.objectContaining({ type: 'object', required: ['id'] }),
        tools: [
          {
            name: 'orders.refund',
            description: '退款',
            surface: 'view',
            inputSchema: expect.objectContaining({ type: 'object', required: ['reason'] }),
          },
        ],
        navigable: false,
      },
      { name: 'cart', description: '购物车' },
    ])
  })

  it('surface: app 不写出（缺省）；顶层工具可带 page', () => {
    const manifest = generateManifest(INFO, [{ name: 'a', description: 'A', surface: 'app', page: 'cart' }], {}, [
      { name: 'cart' },
    ])
    expect(manifest.tools).toEqual([{ name: 'a', description: 'A', inputSchema: { type: 'object', properties: {} }, page: 'cart' }])
  })

  it.each([
    [{ pages: [{ name: 'bad name' }] }, /页面名不合法/],
    [{ pages: [{ name: 'a' }, { name: 'a' }] }, /页面名重复/],
    [{ pages: [{ name: 'a', description: ' ' }] }, /description 不能为空字符串/],
    [{ pages: [{ name: 'a', params: { type: 'string' } }] }, /params 必须是 type 为 "object"/],
    [
      { pages: [{ name: 'a', tools: [{ name: 't', description: 'T', inputSchema: { type: 'object' }, page: 'b' }] }] },
      /必须等于所在页面名/,
    ],
    [
      {
        tools: [{ name: 't', description: 'T', inputSchema: { type: 'object' } }],
        pages: [{ name: 'a', tools: [{ name: 't', description: 'T2', inputSchema: { type: 'object' } }] }],
      },
      /名称重复/,
    ],
    [{ tools: [{ name: 't', description: 'T', inputSchema: { type: 'object' }, surface: 'page' }] }, /surface "page" 不合法/],
  ])('校验错误 %#', (extra, message) => {
    const result = validateManifest({ manifestVersion: 1, ...INFO, ...extra } as AppMcpManifest)
    expect(result.errors.join('\n')).toMatch(message)
  })

  it('顶层工具的 page 指向未声明的页面只警告', () => {
    const result = validateManifest({
      manifestVersion: 1,
      ...INFO,
      tools: [{ name: 't', description: 'T', inputSchema: { type: 'object' }, page: 'nowhere' }],
    })
    expect(result.errors).toEqual([])
    expect(result.warnings.join('\n')).toContain('nowhere')
  })

  it('页面工具与顶层工具同名时 generateManifest 抛出 ManifestError', () => {
    expect(() =>
      generateManifest(INFO, [{ name: 't', description: 'T' }], {}, [{ name: 'p', tools: [{ name: 't', description: 'T' }] }]),
    ).toThrow(ManifestError)
  })

  it('mergePages：显式定义优先，缺 route 时补扫描到的路由', () => {
    expect(
      mergePages(
        [
          { name: 'a', route: '/a', tools: [{ name: 'x', description: 'X' }] },
          { name: 'b', route: '/b' },
        ],
        [{ name: 'a', description: '显式' }, { name: 'c' }],
      ),
    ).toEqual([{ name: 'a', description: '显式', route: '/a' }, { name: 'b', route: '/b' }, { name: 'c' }])
  })
})

const REACT_ROUTES = `
import { createBrowserRouter } from 'react-router'
import { Layout } from './Layout'
import { OrdersPage } from './pages/OrdersPage'
import CartPage from './pages/CartPage'
import { Home } from './pages/Home'

export const router = createBrowserRouter([
  {
    path: '/',
    element: <Layout />,
    children: [
      { index: true, id: 'home', element: <Home /> , handle: { mcp: { title: '首页' } } },
      { path: 'orders/:id?', id: 'orders', element: <OrdersPage />, handle: { mcp: { title: '订单', description: '历史订单', params: { type: 'object', properties: { id: { type: 'string' } } } } } },
      { path: 'cart', id: 'cart', Component: CartPage },
      { path: 'lazy', id: 'lazy', lazy: () => import('./pages/LazyPage') },
      { path: 'about' },
    ],
  },
])
`

const ORDERS_PAGE = `
import { useTool } from '@app-mcp/react'
export function OrdersPage() {
  useTool('orders.list', {
    title: '订单列表',
    description: '列出订单',
    input: { type: 'object', properties: { status: { type: 'string', enum: ['paid', 'shipped'] } } } as const,
    annotations: { readOnlyHint: true },
    surface: 'view',
    implements: ['link.open@1'],
    cache: { ttlMs: 5000, scope: 'shared' },
    deprecated: { message: '改用 orders.search', replacement: 'orders.search' },
    handler: () => [],
    enabled: true,
  })
  return null
}
`

describe('scanRoutes（React Router）', () => {
  it('带 id 的路由是页面：路由拼接、handle.mcp 说明、组件模块中的 useTool 为页面工具', async () => {
    const root = await projects.make({
      'src/routes.tsx': REACT_ROUTES,
      'src/Layout.tsx': `import { useTool } from '@app-mcp/react'\nexport function Layout() { useTool('layout.x', { description: 'x', handler() {} }); return null }\n`,
      'src/pages/OrdersPage.tsx': ORDERS_PAGE,
      'src/pages/CartPage.tsx': `export default function CartPage() { return null }\n`,
      'src/pages/Home.tsx': `export function Home() { return null }\n`,
      'src/pages/LazyPage.tsx': `import { useTool } from '@app-mcp/react'\nexport function Component() { useTool('lazy.t', { description: '惰性', surface: 'view', handler: () => 1 }); return null }\n`,
    })
    const result = scanRoutes({ root, routes: [{ file: 'src/routes.tsx', router: 'react-router' }] })
    expect(result.errors).toEqual([])
    expect(result.pages.map((p) => p.page)).toEqual([
      { name: 'home', route: '/', title: '首页' },
      expect.objectContaining({ name: 'orders', route: '/orders/:id?', title: '订单', description: '历史订单' }),
      { name: 'cart', route: '/cart' },
      { name: 'lazy', route: '/lazy', tools: [{ name: 'lazy.t', description: '惰性', surface: 'view' }] },
    ])
    const orders = result.pages[1]!.page
    expect(orders.params).toEqual({ type: 'object', properties: { id: { type: 'string' } } })
    expect(orders.tools).toEqual([
      {
        name: 'orders.list',
        title: '订单列表',
        description: '列出订单',
        input: { type: 'object', properties: { status: { type: 'string', enum: ['paid', 'shipped'] } } },
        annotations: { readOnlyHint: true },
        surface: 'view',
        implements: ['link.open@1'],
        cache: { ttlMs: 5000, scope: 'shared' },
        deprecated: { message: '改用 orders.search', replacement: 'orders.search' },
      },
    ])
    expect(result.dependencies.some((d) => d.endsWith('OrdersPage.tsx'))).toBe(true)
  })

  it('无法静态确定时报错（带位置），要求显式声明；显式声明的页面不扫描组件', async () => {
    const root = await projects.make({
      'src/routes.tsx': `
import { CartPage } from './CartPage'
const pageId = 'dyn'
export const routes = [
  { path: '/cart', id: 'cart', element: <CartPage /> },
  { path: '/dyn', id: pageId, element: <CartPage /> },
  { path: '/local', id: 'local', element: <Local /> },
]
function Local() { return null }
`,
      'src/CartPage.tsx': `
import { useTool } from '@app-mcp/react'
import { z } from 'zod'
const name = 'cart.x'
export function CartPage() {
  useTool('cart.add', { description: '加入', input: z.object({ id: z.string() }), handler: () => {} })
  useTool(name, { description: 'x', handler: () => {} })
  return null
}
`,
    })
    const result = scanRoutes({ root, routes: [{ file: 'src/routes.tsx', router: 'react-router' }] })
    const text = result.errors.join('\n')
    expect(text).toMatch(/src\/CartPage\.tsx:6 工具 cart\.add 的 input 不是 JSON Schema 字面量/)
    expect(text).toMatch(/src\/CartPage\.tsx:7 工具名不是字符串字面量/)
    expect(text).toMatch(/src\/routes\.tsx:6 路由的 id 不是字符串字面量/)
    expect(text).toMatch(/组件 Local 不是从其他模块导入的/)
    expect(text).toContain('definePage()')

    const skipped = scanRoutes({
      root,
      routes: [{ file: 'src/routes.tsx', router: 'react-router' }],
      skip: new Set(['cart', 'local']),
    })
    expect(skipped.errors.join('\n')).not.toContain('CartPage.tsx')
    expect(skipped.pages.map((p) => p.page)).toContainEqual({ name: 'cart', route: '/cart' })
  })

  it('工具声明的 page 与路由页面不一致时报错', async () => {
    const root = await projects.make({
      'src/routes.tsx': `import { P } from './P'\nexport default [{ path: '/p', id: 'p', element: <P /> }]\n`,
      'src/P.tsx': `export function P() { useTool('t', { description: 'T', page: 'other', handler() {} }) }\n`,
    })
    const result = scanRoutes({ root, routes: [{ file: 'src/routes.tsx', router: 'react-router' }] })
    expect(result.errors.join('\n')).toMatch(/page "other" 与所在路由页面 "p" 不一致/)
  })
})

describe('scanRoutes（Vue Router）', () => {
  it('带 name 的路由是页面；meta.mcp 说明；.vue 组件中 <x>.tool(...) 为页面工具', async () => {
    const root = await projects.make({
      'src/router.ts': `
import { createRouter, createWebHistory } from 'vue-router'
import Home from './views/Home.vue'
export default createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'home', component: Home, meta: { mcp: { title: '首页' } } },
    { path: '/orders', name: 'orders', component: () => import('./views/Orders.vue'),
      children: [{ path: ':id', name: 'orders.detail', component: () => import('./views/OrderDetail.vue'), meta: { mcp: { navigable: false } } }] },
  ],
})
`,
      'src/views/Home.vue': `<template><div /></template>\n`,
      'src/views/Orders.vue': `<template>\n  <ul />\n</template>\n<script setup lang="ts">\nimport { appMcp } from '../mcp'\nappMcp.tool('orders.list', { description: '订单列表', surface: 'view', handler: () => [] })\n</script>\n`,
      'src/views/OrderDetail.vue': `<script setup>\nscope.tool('orders.refund', { description: '退款', input: refundSchema, handler() {} })\n</script>\n`,
    })
    const result = scanRoutes({ root, routes: [{ file: 'src/router.ts', router: 'vue-router' }] })
    expect(result.errors).toEqual([expect.stringMatching(/src\/views\/OrderDetail\.vue:2 工具 orders\.refund 的 input/)])
    expect(result.pages.map((p) => p.page)).toEqual([
      { name: 'home', route: '/', title: '首页' },
      { name: 'orders', route: '/orders', tools: [{ name: 'orders.list', description: '订单列表', surface: 'view' }] },
    ])
  })
})

describe('插件 pages / routes', () => {
  it('扫描路由 + 显式页面模块合并后写入清单', async () => {
    const dir = await projects.make({
      'index.html': '<!doctype html><html><body></body></html>',
      'src/routes.tsx': `import { OrdersPage } from './OrdersPage'\nimport { CartPage } from './CartPage'\nexport const routes = [\n  { path: '/orders', id: 'orders', element: <OrdersPage /> },\n  { path: '/cart', id: 'cart', element: <CartPage /> },\n]\n`,
      'src/OrdersPage.tsx': ORDERS_PAGE,
      'src/CartPage.tsx': `import { z } from 'zod'\nexport function CartPage() { useTool('cart.add', { description: '加入', input: z.object({}), handler() {} }) }\n`,
      'src/pages.ts': `import { z } from 'zod'\nexport default [{ name: 'cart', title: '购物车', tools: [{ name: 'cart.add', description: '加入', surface: 'view', input: z.object({ productId: z.string() }) }] }]\n`,
    })
    await build({
      root: dir,
      configFile: false,
      logLevel: 'silent',
      plugins: [
        appMcp({
          ...INFO,
          routes: { file: 'src/routes.tsx', router: 'react-router' },
          pages: 'src/pages.ts',
          writeTo: 'app-mcp.json',
        }),
      ],
    })
    const manifest = JSON.parse(await readFile(join(dir, 'app-mcp.json'), 'utf8')) as AppMcpManifest
    expect(manifest.pages?.map((p) => [p.name, p.route, p.tools?.map((t) => t.name)])).toEqual([
      ['orders', '/orders', ['orders.list']],
      ['cart', '/cart', ['cart.add']],
    ])
    expect(manifest.pages?.[1]?.tools?.[0]?.inputSchema).toMatchObject({ required: ['productId'] })
    expect(manifest.tools).toBeUndefined()
  })

  it('扫描错误使构建失败', async () => {
    const dir = await projects.make({
      'index.html': '<!doctype html><html><body></body></html>',
      'src/routes.tsx': `import { P } from './P'\nexport const routes = [{ path: '/p', id: 'p', element: <P /> }]\n`,
      'src/P.tsx': `export function P() { useTool('p.x', { ...base, handler() {} }) }\n`,
    })
    await expect(
      build({
        root: dir,
        configFile: false,
        logLevel: 'silent',
        plugins: [appMcp({ ...INFO, routes: { file: 'src/routes.tsx', router: 'react-router' } })],
      }),
    ).rejects.toThrow(/展开或计算属性名/)
  })
})
