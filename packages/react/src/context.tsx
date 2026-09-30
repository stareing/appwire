import { createContext, useContext, type ReactNode } from 'react'
import type { AppMcp, Registrar } from '@app-mcp/web'

const AppMcpContext = createContext<AppMcp | null>(null)

/** 当前生效的注册入口：最近的 `<ToolScope>`，或者 Provider 提供的 AppMcp 实例。 */
export const RegistrarContext = createContext<Registrar | null>(null)

export interface AppMcpProviderProps {
  value: AppMcp
  children?: ReactNode
}

/** 向子组件提供 AppMcp 实例。 */
export function AppMcpProvider({ value, children }: AppMcpProviderProps) {
  return (
    <AppMcpContext.Provider value={value}>
      <RegistrarContext.Provider value={value}>{children}</RegistrarContext.Provider>
    </AppMcpContext.Provider>
  )
}

/** 读取 Provider 提供的 AppMcp 实例；没有 Provider 时抛出错误。 */
export function useAppMcp(): AppMcp {
  const appMcp = useContext(AppMcpContext)
  if (!appMcp) {
    throw new Error('useAppMcp 必须在 <AppMcpProvider> 内部使用')
  }
  return appMcp
}

/** 内部使用：没有 Provider 时返回 null（hooks 此时为空操作）。 */
export function useRegistrar(): Registrar | null {
  return useContext(RegistrarContext)
}
