import { useHold, useResource, useTool } from '@app-mcp/react'
import { ToolCallError } from '@app-mcp/web'
import { useState } from 'react'
import { z } from 'zod'
import {
  ADDRESSES,
  addToCart,
  cartStore,
  cartSummary,
  checkout,
  lastOrderStore,
  removeFromCart,
} from '../store/cart'
import { findProduct, searchProducts } from '../store/catalog'
import { useStore } from '../store/create-store'

const HINTS = ['cart.state']
const yuan = (n: number) => `¥${n.toFixed(2)}`

export function CartPage() {
  const items = useStore(cartStore)
  const lastOrder = useStore(lastOrderStore)
  // 购物车非空时保持连接（模型可能继续结算）；只在 idle / on-demand 模式下有效果
  useHold(items.length > 0)
  const [keyword, setKeyword] = useState('')
  const [addressId, setAddressId] = useState(ADDRESSES[0]!.id)
  const [paying, setPaying] = useState(false)
  const summary = cartSummary(items)
  const products = searchProducts(keyword)

  useResource('cart.state', {
    description: '当前购物车：条目（itemId、商品、单价、数量）、总价，以及可用于结算的收货地址',
    read: () => ({ ...cartSummary(items), addresses: ADDRESSES }),
    deps: [items],
  })

  useTool('cart.add', {
    title: '加入购物车',
    description: '把商品加入购物车（同一商品合并数量），返回更新后的购物车摘要',
    input: z.object({
      productId: z.string().describe('商品 ID，来自 catalog.search'),
      qty: z.number().int().min(1).max(99).default(1).describe('数量'),
    }),
    risk: 'write',
    handler: ({ productId, qty }) => {
      if (!findProduct(productId)) {
        throw new ToolCallError('INVALID_INPUT', `商品不存在：${productId}。请先用 catalog.search 查询商品 ID`)
      }
      const item = addToCart(productId, qty)
      return { data: { added: item, cart: cartSummary() }, stateHints: HINTS }
    },
  })

  useTool('cart.removeItem', {
    title: '移除购物车条目',
    description: '从购物车移除一个条目，返回更新后的购物车摘要',
    input: z.object({ itemId: z.string().describe('条目 ID，来自 cart.state') }),
    risk: 'write',
    handler: ({ itemId }) => {
      const removed = removeFromCart(itemId)
      if (!removed) throw new ToolCallError('INVALID_INPUT', `购物车中没有条目 ${itemId}。请先读取 cart.state`)
      return { data: { removed, cart: cartSummary() }, stateHints: HINTS }
    },
  })

  // 购物车为空时禁用：工具从列表中移除，模型不会尝试结算。
  useTool('cart.checkout', {
    title: '结算',
    description: '结算当前购物车并下单（支付操作），返回订单号、金额与收货地址',
    input: z.object({
      addressId: z.enum(ADDRESSES.map((a) => a.id) as [string, ...string[]]).describe('收货地址 ID，来自 cart.state'),
    }),
    risk: 'payment',
    activation: 'foreground',
    enabled: items.length > 0,
    handler: async ({ addressId }) => ({ data: await checkout(addressId), stateHints: HINTS }),
  })

  const onCheckout = async () => {
    setPaying(true)
    try {
      await checkout(addressId)
    } finally {
      setPaying(false)
    }
  }

  return (
    <div className="grid">
      <section className="card">
        <h2>商品</h2>
        <input
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
          placeholder="搜索商品名或分类"
          aria-label="搜索商品"
        />
        <ul className="list">
          {products.map((p) => (
            <li key={p.id}>
              <span>
                {p.name} <small className="muted">{p.category}</small>
              </span>
              <span className="row">
                <span className="price">{yuan(p.price)}</span>
                <button onClick={() => addToCart(p.id, 1)}>加入购物车</button>
              </span>
            </li>
          ))}
          {products.length === 0 && <p className="empty">没有找到商品</p>}
        </ul>
      </section>

      <section className="card">
        <h2>购物车</h2>
        {items.length === 0 ? (
          <p className="empty">购物车是空的</p>
        ) : (
          <ul className="list">
            {items.map((item) => (
              <li key={item.itemId}>
                <span>
                  {item.name} × {item.qty}
                </span>
                <span className="row">
                  <span className="price">{yuan(item.price * item.qty)}</span>
                  <button className="link danger" onClick={() => removeFromCart(item.itemId)}>
                    移除
                  </button>
                </span>
              </li>
            ))}
          </ul>
        )}
        <div className="total">
          <span>
            共 {summary.itemCount} 件，合计 <strong>{yuan(summary.total)}</strong>
          </span>
        </div>
        <label className="field">
          收货地址
          <select value={addressId} onChange={(e) => setAddressId(e.target.value)}>
            {ADDRESSES.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
          </select>
        </label>
        <button className="primary block" disabled={items.length === 0 || paying} onClick={onCheckout}>
          {paying ? '结算中…' : '结算'}
        </button>
        {lastOrder && (
          <p className="notice">
            已下单 {lastOrder.orderId}：{lastOrder.itemCount} 件，{yuan(lastOrder.total)}，送至{lastOrder.address.label}
          </p>
        )}
      </section>
    </div>
  )
}
