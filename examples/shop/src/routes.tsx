/**
 * 路由表：带 `id` 的路由是页面（页面目录的键），`handle.mcp` 是页面说明。@app-mcp/build 扫描本文件（vite.config.ts 的 `routes`）
 * 生成清单 `pages`：商品、订单页的工具从页面组件模块中的 `useTool` 字面量读出；购物车页的工具输入是 zod schema，无法静态
 * 确定，改由 src/mcp/pages.ts 显式声明（definePage）。待办页没有 id：其工具只在打开时存在，不进入页面目录。
 */
import { CartPage } from './pages/CartPage'
import { OrdersPage } from './pages/OrdersPage'
import { ProductsPage } from './pages/ProductsPage'
import { TodoPage } from './pages/TodoPage'
import type { RouteObject } from './router'

export const routes: RouteObject[] = [
  { path: '/', element: <TodoPage /> },
  {
    path: '/products',
    id: 'products',
    element: <ProductsPage />,
    handle: { keepAlive: true, mcp: { title: '商品列表', description: '浏览与筛选商品，打开商品详情后可从详情加入购物车' } },
  },
  {
    path: '/cart',
    id: 'cart',
    element: <CartPage />,
    handle: { mcp: { title: '购物车', description: '查看购物车、移除条目、结算' } },
  },
  {
    path: '/orders',
    id: 'orders',
    element: <OrdersPage />,
    handle: { mcp: { title: '订单', description: '本次会话中已下的订单' } },
  },
]
