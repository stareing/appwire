import { ToolScope, useTool } from '@app-mcp/react'
import { useRef } from 'react'
import { ordersStore } from '../store/cart'
import { useStore } from '../store/create-store'

const yuan = (n: number) => `¥${n.toFixed(2)}`

/** 订单页：`orders.list` 是页面工具（surface view）；在其他页面调用时 Host 先导航到本页。 */
export function OrdersPage() {
  const ref = useRef<HTMLElement>(null)
  return (
    <section ref={ref} className="card">
      <ToolScope name="orders" page="orders" anchor={ref}>
        <OrdersView />
      </ToolScope>
    </section>
  )
}

function OrdersView() {
  const orders = useStore(ordersStore)

  useTool('orders.list', {
    title: '订单列表',
    description: '列出本次会话中已下的订单（订单号、件数、金额、收货地址）',
    annotations: { readOnlyHint: true },
    surface: 'view',
    handler: () => ({ orders: ordersStore.get() }),
  })

  return (
    <>
      <h2>订单</h2>
      {orders.length === 0 ? (
        <p className="empty">还没有订单</p>
      ) : (
        <ul className="list orders">
          {orders.map((o) => (
            <li key={o.orderId}>
              <span>
                {o.orderId} · {o.itemCount} 件 · 送至{o.address.label}
              </span>
              <span className="price">{yuan(o.total)}</span>
            </li>
          ))}
        </ul>
      )}
    </>
  )
}
