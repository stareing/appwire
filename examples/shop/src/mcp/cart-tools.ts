/**
 * 购物车页工具的定义（不含 handler）：构建期由 src/mcp/pages.ts 写入清单的页面目录，运行时 CartPage 用同一份定义注册
 * （输入是 zod schema，路由扫描无法静态确定，所以显式声明）。只能导入 zod 与无副作用的模块（构建时在 Node 中执行）。
 */
import { defineStaticTool } from '@app-mcp/build/define'
import { z } from 'zod'
import { ADDRESSES } from '../store/addresses'

export const cartAdd = defineStaticTool({
  name: 'cart.add',
  title: '加入购物车',
  description: '把商品加入购物车（同一商品合并数量），返回更新后的购物车摘要',
  input: z.object({
    productId: z.string().describe('商品 ID，来自 catalog.search'),
    qty: z.number().int().min(1).max(99).default(1).describe('数量'),
  }),
  risk: 'write',
  surface: 'view',
})

export const cartRemoveItem = defineStaticTool({
  name: 'cart.removeItem',
  title: '移除购物车条目',
  description: '从购物车移除一个条目，返回更新后的购物车摘要',
  input: z.object({ itemId: z.string().describe('条目 ID，来自 cart.state') }),
  risk: 'write',
  surface: 'view',
})

export const cartCheckout = defineStaticTool({
  name: 'cart.checkout',
  title: '结算',
  description: '结算当前购物车并下单（支付操作），返回订单号、金额与收货地址',
  input: z.object({
    addressId: z.enum(ADDRESSES.map((a) => a.id) as [string, ...string[]]).describe('收货地址 ID，来自 cart.state'),
  }),
  risk: 'payment',
  activation: 'foreground',
  surface: 'view',
})
