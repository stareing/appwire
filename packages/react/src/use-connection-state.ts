import { useCallback, useSyncExternalStore } from 'react'
import type { ConnectionState } from '@app-mcp/web'
import { useAppMcp } from './context'

/**
 * 订阅连接状态（用于在界面上显示连接指示）。
 *
 * 与 `useTool` / `useResource` 不同，这个 hook 会在状态变化时重新渲染调用它的组件，
 * 只应在需要显示状态的组件中使用。
 */
export function useConnectionState(): ConnectionState {
  const appMcp = useAppMcp()
  const subscribe = useCallback((onChange: () => void) => appMcp.onStateChange(onChange), [appMcp])
  const getSnapshot = useCallback(() => appMcp.state, [appMcp])
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
