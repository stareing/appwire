import { useEffect, useMemo, type ReactNode } from 'react'
import { createViewLayer, type ToolSurface } from '@app-mcp/web'
import { RegistrarContext, useRegistrar } from './context'
import { useIsomorphicLayoutEffect } from './internal'
import { type AnchorProp, compactOptions, LazyScope, useAnchorResolver } from './tool-scope'

export interface ToolLayerProps {
  /** 层名称（出现在导航被拒绝的说明中，如「结算确认」）。 */
  name: string
  /** 层是否打开，缺省 true（挂载即打开）。为 false 时层内 `view` 工具不启用，下层不受压制。 */
  open?: boolean
  /** 层内工具的缺省锚点（如对话框根元素的 ref；原生 `<dialog>` 用 `showModal()` 打开时需要给出）。 */
  anchor?: AnchorProp
  /** 层内工具的缺省 surface，缺省 `view`（层本身就是界面）。 */
  surface?: ToolSurface
  children?: ReactNode
}

/**
 * 界面层（对话框、抽屉、模态框，spec/protocol.md 3.4 / 第 4c 项 D）：打开期间压在之前打开的层之上，
 * 下层的 `view` 工具暂停（对 Host 表现为禁用，`tools/changed`），层内工具启用；关闭或卸载时恢复。
 * 层打开时 Host 的导航请求缺省被拒绝（用户正在与弹层交互，见 `NavigationOptions.whileLayerOpen`）。
 *
 * ```tsx
 * {open && (
 *   <ToolLayer name="结算确认">
 *     <CheckoutDialog />   // 其中 useTool('checkout.confirm', ...) 是层内工具
 *   </ToolLayer>
 * )}
 * ```
 *
 * 层在布局阶段打开（早于子组件的 `useTool` 注册），层内工具注册时即为启用。没有 `<AppMcpProvider>` 时仍维护层栈。
 */
export function ToolLayer({ name, open = true, anchor, surface = 'view', children }: ToolLayerProps) {
  const parent = useRegistrar()
  const layer = useMemo(() => createViewLayer(name), [name])
  const anchorResolver = useAnchorResolver(anchor)

  useIsomorphicLayoutEffect(() => {
    if (!open) return
    layer.open()
    return () => layer.close()
  }, [layer, open])

  const scope = useMemo(
    () => (parent ? new LazyScope(parent, name, compactOptions({ layer, surface, anchor: anchorResolver })) : null),
    [parent, name, layer, surface, anchorResolver],
  )

  useEffect(() => {
    if (!scope) return
    return () => scope.dispose()
  }, [scope])

  return <RegistrarContext.Provider value={scope ?? parent}>{children}</RegistrarContext.Provider>
}
