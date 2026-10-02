/**
 * 显式页面定义（@app-mcp/build 的 `pages` 选项）：路由扫描无法静态确定的页面在这里声明，同名页面以此为准
 * （路由 `/cart` 取自 src/routes.tsx 的扫描结果）。
 */
import { definePages } from '@app-mcp/build/define'
import { cartAdd, cartCheckout, cartRemoveItem } from './cart-tools'

export default definePages([
  {
    name: 'cart',
    title: '购物车',
    description: '查看购物车、移除条目、结算',
    tools: [cartAdd, cartRemoveItem, cartCheckout],
  },
])
