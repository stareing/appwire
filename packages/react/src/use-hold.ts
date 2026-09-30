import { useEffect } from 'react'
import { useAppMcp } from './context'

/**
 * 组件挂载期间阻止自动休眠（spec/lifecycle.md 第 3 节「持有」）。
 *
 * 适合模型可能连续操作的界面（如结算页、正在编辑的表单）：组件卸载或 `active` 变为 false 时释放。
 * 只影响 `lifecycle.mode` 为 `idle` / `on-demand` 的实例；`persistent` 模式本来就不休眠。
 */
export function useHold(active = true): void {
  const appMcp = useAppMcp()
  useEffect(() => {
    if (!active) return undefined
    const handle = appMcp.hold()
    return () => handle.release()
  }, [appMcp, active])
}
