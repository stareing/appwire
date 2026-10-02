import { ToolLayer, useTool } from '@app-mcp/react'
import { useRef, useState } from 'react'
import { addToCart, cartSummary } from '../store/cart'
import { findProduct } from '../store/catalog'

const yuan = (n: number) => `¥${n.toFixed(2)}`

/**
 * 商品详情对话框：`<ToolLayer>` 打开期间压住下层（商品页的 view 工具暂停），层内工具启用；关闭后恢复。
 * 对话框打开时 Host 的导航请求被拒绝（NAVIGATION_DENIED，用户可能正在操作）。
 */
export function ProductDialog({ productId, onClose }: { productId: string; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <div className="overlay">
      <div ref={ref} className="dialog card" role="dialog" aria-modal="true" aria-label="商品详情">
        <ToolLayer name="商品详情" anchor={ref}>
          <DialogBody productId={productId} onClose={onClose} />
        </ToolLayer>
      </div>
    </div>
  )
}

function DialogBody({ productId, onClose }: { productId: string; onClose: () => void }) {
  const product = findProduct(productId)
  const [qty, setQty] = useState(1)
  const [added, setAdded] = useState(0)

  useTool<{ qty?: number }>('productDialog.addToCart', {
    title: '从详情加入购物车',
    description: '把详情对话框中的商品加入购物车（对话框保持打开），返回购物车摘要',
    input: {
      type: 'object',
      properties: { qty: { type: 'integer', minimum: 1, maximum: 99, description: '数量，缺省为对话框中的数量' } },
    },
    handler: ({ qty: n }) => {
      const item = addToCart(productId, n ?? qty)
      setAdded((a) => a + (n ?? qty))
      return { added: item, cart: cartSummary() }
    },
  })

  useTool('productDialog.close', {
    title: '关闭商品详情',
    description: '关闭商品详情对话框',
    annotations: { idempotentHint: true },
    handler: () => {
      onClose()
      return null
    },
  })

  if (!product) return <p className="empty">商品不存在</p>
  const addFromUi = (): void => {
    addToCart(product.id, qty)
    setAdded((a) => a + qty)
  }
  return (
    <>
      <h2>{product.name}</h2>
      <p className="muted">
        {product.category} · <span className="price">{yuan(product.price)}</span>
      </p>
      <label className="field">
        数量
        <input type="number" min={1} max={99} value={qty} onChange={(e) => setQty(Math.max(1, Number(e.target.value) || 1))} />
      </label>
      {added > 0 && <p className="notice">已加入购物车 {added} 件</p>}
      <div className="row actions">
        <button className="primary" onClick={addFromUi}>
          加入购物车
        </button>
        <button onClick={onClose}>关闭</button>
      </div>
    </>
  )
}
