export interface Product {
  id: string
  name: string
  price: number
  category: string
}

export const PRODUCTS: readonly Product[] = [
  { id: 'p1', name: '机械键盘', price: 399, category: '数码' },
  { id: 'p2', name: '无线鼠标', price: 129, category: '数码' },
  { id: 'p3', name: '降噪耳机', price: 899, category: '数码' },
  { id: 'p4', name: '保温杯', price: 79, category: '家居' },
  { id: 'p5', name: '台灯', price: 159, category: '家居' },
  { id: 'p6', name: '咖啡豆 500g', price: 88, category: '食品' },
]

export function findProduct(id: string): Product | undefined {
  return PRODUCTS.find((p) => p.id === id)
}

/** 按名称或分类搜索商品；关键词为空时返回全部。界面搜索框与 catalog.search 共用。 */
export function searchProducts(keyword?: string): Product[] {
  const k = keyword?.trim().toLowerCase()
  if (!k) return [...PRODUCTS]
  return PRODUCTS.filter((p) => p.name.toLowerCase().includes(k) || p.category.includes(k))
}
