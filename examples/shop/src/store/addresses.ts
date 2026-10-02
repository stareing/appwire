/** 收货地址（构建期的页面定义 src/mcp/pages.ts 也会导入，保持无副作用、不依赖 React）。 */
export interface Address {
  id: string
  label: string
}

export const ADDRESSES: readonly Address[] = [
  { id: 'home', label: '家 · 北京市朝阳区幸福路 1 号' },
  { id: 'office', label: '公司 · 北京市海淀区中关村大街 27 号' },
]
