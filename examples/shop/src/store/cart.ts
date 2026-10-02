import { type Address, ADDRESSES } from './addresses'
import { findProduct } from './catalog'
import { createStore } from './create-store'

export { ADDRESSES, type Address }

export interface CartItem {
  itemId: string
  productId: string
  name: string
  price: number
  qty: number
}

export interface Order {
  orderId: string
  total: number
  itemCount: number
  address: Address
}

export const cartStore = createStore<CartItem[]>([])
export const lastOrderStore = createStore<Order | null>(null)
/** 本次会话的全部订单（订单页）。 */
export const ordersStore = createStore<readonly Order[]>([])

let nextItemId = 1
let nextOrderId = 1024

export function cartSummary(items: readonly CartItem[] = cartStore.get()) {
  return {
    items: items.map(({ itemId, productId, name, price, qty }) => ({ itemId, productId, name, price, qty })),
    itemCount: items.reduce((n, i) => n + i.qty, 0),
    total: items.reduce((sum, i) => sum + i.price * i.qty, 0),
  }
}

/** 加入购物车（同一商品合并数量），返回更新后的条目。 */
export function addToCart(productId: string, qty = 1): CartItem {
  const product = findProduct(productId)
  if (!product) throw new Error(`商品不存在：${productId}`)
  const items = cartStore.get()
  const existing = items.find((i) => i.productId === productId)
  if (existing) {
    const updated = { ...existing, qty: existing.qty + qty }
    cartStore.set(items.map((i) => (i === existing ? updated : i)))
    return updated
  }
  const item: CartItem = { itemId: `i${nextItemId++}`, productId, name: product.name, price: product.price, qty }
  cartStore.set([...items, item])
  return item
}

/** 移除购物车条目，返回被移除的条目；不存在时返回 undefined。 */
export function removeFromCart(itemId: string): CartItem | undefined {
  const items = cartStore.get()
  const item = items.find((i) => i.itemId === itemId)
  if (item) cartStore.set(items.filter((i) => i !== item))
  return item
}

/** 结算：生成订单并清空购物车（模拟网络延迟）。 */
export async function checkout(addressId: string): Promise<Order> {
  const address = ADDRESSES.find((a) => a.id === addressId)
  if (!address) throw new Error(`收货地址不存在：${addressId}`)
  const summary = cartSummary()
  if (summary.items.length === 0) throw new Error('购物车为空，无法结算')
  await new Promise((r) => setTimeout(r, 300))
  const order: Order = {
    orderId: `A${nextOrderId++}`,
    total: summary.total,
    itemCount: summary.itemCount,
    address,
  }
  cartStore.set([])
  lastOrderStore.set(order)
  ordersStore.set([...ordersStore.get(), order])
  return order
}
