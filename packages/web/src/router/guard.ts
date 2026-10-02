/** 路由适配的导航前检查（`guard` 选项）的结果解释，React / Vue 适配共用。 */

/** 守卫返回值：`false` 或字符串（拒绝原因）表示拒绝；`true` / `undefined` 表示允许。 */
export type GuardResult = boolean | string | void

/** 守卫结果 → 拒绝原因（允许时 undefined）。 */
export function guardDenial(result: GuardResult, page: string): string | undefined {
  if (result === false) return `App 不允许现在切换到页面「${page}」`
  if (typeof result === 'string') return result
  return undefined
}
