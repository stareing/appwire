/**
 * 用 `@mcp` JSDoc 注释声明的工具：@app-mcp/build 在编译期扫描这些导出函数，
 * 从 TypeScript 参数类型推导 JSON Schema，写入 app-mcp.json，并由 `virtual:app-mcp/annotated`
 * 的 `registerAnnotated(appMcp)` 在运行时注册（见 main.tsx）。
 */

export interface ShopInfo {
  name: string
  /** 营业时间，如 "09:00-22:00"。 */
  hours: string
  open: boolean
  hotline: string
}

/**
 * 获取店铺营业信息（名称、营业时间、当前是否营业、客服电话）
 * @mcp shop.info
 * @risk read
 * @activation headless
 */
export function shopInfo(): ShopInfo {
  const hour = new Date().getHours()
  return { name: '示例商城', hours: '09:00-22:00', open: hour >= 9 && hour < 22, hotline: '400-000-0000' }
}

/**
 * 估算配送到指定城市所需的天数
 * @mcp shop.deliveryEstimate
 * @risk read
 * @activation headless
 * @param city 收货城市，如“上海”
 * @param express 是否加急配送
 */
export function deliveryEstimate(city: string, express?: boolean): { city: string; days: number } {
  const base = ['北京', '上海', '广州', '深圳'].includes(city) ? 2 : 4
  return { city, days: express ? Math.max(1, base - 1) : base }
}
