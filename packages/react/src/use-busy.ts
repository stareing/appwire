import { useEffect } from 'react'
import { useAppMcp } from './context'

/**
 * 声明用户正在 App 内操作（spec/protocol.md 5.3）：`active` 为 true 期间写调用按 `busyPolicy` 拒绝或排队，
 * 只读调用不受影响。组件卸载或 `active` 变为 false 时撤销。适合编辑框获得焦点、拖拽中等场景，如 `useBusy(focused)`。
 *
 * 基于 `appMcp.beginBusy()` 作用域：多个组件同时使用时按引用计数，任一为 true 即 busy；与 `appMcp.setBusy` 的显式开关
 * 互不清除（有效值为两者之或）。
 */
export function useBusy(active: boolean): void {
  const appMcp = useAppMcp()
  useEffect(() => {
    if (!active) return undefined
    const handle = appMcp.beginBusy()
    return () => handle.release()
  }, [appMcp, active])
}
