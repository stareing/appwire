import { ToolScope, useTool } from '@app-mcp/react'
import { ToolCallError } from '@app-mcp/web'
import { useRef, useState } from 'react'
import { ProductDialog } from '../components/ProductDialog'
import { findProduct, searchProducts } from '../store/catalog'

const yuan = (n: number) => `¥${n.toFixed(2)}`

/**
 * 商品列表页（keep-alive：离开后保持挂载、只隐藏，筛选条件保留）。页面工具是 surface view：页面隐藏时暂停，
 * 打开商品详情对话框（ToolLayer）时也暂停，对话框内的工具启用。
 */
export function ProductsPage() {
  const ref = useRef<HTMLElement>(null)
  return (
    <section ref={ref} className="card products">
      <ToolScope name="products" page="products" anchor={ref}>
        <ProductsView />
      </ToolScope>
    </section>
  )
}

function ProductsView() {
  const [keyword, setKeyword] = useState('')
  const [openId, setOpenId] = useState<string | null>(null)
  const products = searchProducts(keyword)

  useTool<{ keyword: string }>('products.filter', {
    title: '筛选商品',
    description: '在商品列表页按关键词（商品名或分类）筛选，与页面搜索框相同；返回筛选后的商品',
    input: {
      type: 'object',
      properties: { keyword: { type: 'string', description: '商品名或分类；空字符串显示全部' } },
      required: ['keyword'],
    },
    annotations: { readOnlyHint: false, idempotentHint: true },
    surface: 'view',
    handler: ({ keyword }) => {
      setKeyword(keyword)
      return { keyword, products: searchProducts(keyword) }
    },
  })

  useTool<{ productId: string }>('products.open', {
    title: '打开商品详情',
    description: '打开商品详情对话框；打开后可用 productDialog.addToCart 加入购物车、productDialog.close 关闭',
    input: {
      type: 'object',
      properties: { productId: { type: 'string', description: '商品 ID，来自 catalog.search 或 products.filter' } },
      required: ['productId'],
    },
    surface: 'view',
    handler: ({ productId }) => {
      const product = findProduct(productId)
      if (!product) throw new ToolCallError('INVALID_INPUT', `商品不存在：${productId}`)
      setOpenId(productId)
      return { opened: product }
    },
  })

  return (
    <>
      <h2>商品</h2>
      <input
        value={keyword}
        onChange={(e) => setKeyword(e.target.value)}
        placeholder="搜索商品名或分类"
        aria-label="筛选商品"
      />
      <ul className="list">
        {products.map((p) => (
          <li key={p.id}>
            <span>
              {p.name} <small className="muted">{p.category}</small>
            </span>
            <span className="row">
              <span className="price">{yuan(p.price)}</span>
              <button onClick={() => setOpenId(p.id)}>详情</button>
            </span>
          </li>
        ))}
        {products.length === 0 && <p className="empty">没有找到商品</p>}
      </ul>
      {openId && <ProductDialog productId={openId} onClose={() => setOpenId(null)} />}
    </>
  )
}
