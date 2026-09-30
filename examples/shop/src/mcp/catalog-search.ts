/**
 * catalog.search 的 handler：由 App.tsx 以惰性方式注册（`load: () => import('./mcp/catalog-search')`），
 * 首次被调用时才加载本模块（Vite 会把它拆成单独的 chunk）。
 */
import { searchProducts } from '../store/catalog'

export default function catalogSearch({ keyword }: { keyword?: string }) {
  const products = searchProducts(keyword)
  return { keyword: keyword ?? '', count: products.length, products }
}
