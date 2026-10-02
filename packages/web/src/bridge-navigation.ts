/**
 * 桥接实现（Electron / Tauri 页面侧）的导航（spec/protocol.md 3.4）：主进程 / Rust 侧把 Host 的 `app/navigate` 以
 * `navigate` 事件转给最近一次开启导航（`navigation.set`）的页面，页面执行回调后以 `navigate.result` 回复。
 * 消息类型定义在 electron-bridge.ts（`RendererOp` / `MainEvent`）；执行规则与驱动层相同（{@link runNavigation}）。
 */

import type { AppMcpBridge, NavigateEvent } from './electron-bridge'
import { type NavigationEnv, navigationParams, runNavigation } from './navigation'
import type { NavigationHandler, NavigationOptions } from './types'

export interface BridgeNavigation {
  /** 主进程 / Rust 侧接受本页处理导航后兑现；对方未开启导航转发或版本过旧时以对方的说明拒绝。 */
  readonly ready: Promise<void>
  /** 停止处理导航（通知对方、取消订阅）。幂等。 */
  dispose(): void
}

function defaultEnv(): NavigationEnv {
  return {
    doc: typeof document === 'undefined' ? undefined : document,
    win: typeof window === 'undefined' ? undefined : window,
  }
}

function isNavigateEvent(event: unknown): event is NavigateEvent {
  const e = event as Partial<NavigateEvent> | null
  return typeof e === 'object' && e !== null && e.type === 'navigate' && typeof e.navId === 'number' && typeof e.page === 'string'
}

/**
 * 让本页处理导航请求。`createAppMcp` 在桥接模式下的 `setNavigationHandler` 即用本函数；没有 AppMcp 实例时也可直接调用。
 *
 * @error 回调抛出 `kind` 为 `NAVIGATION_DENIED` 的错误 → 拒绝；`USER_ACTION_REQUIRED`（`ToolCallError.userActionRequired`）
 *   → 需要用户操作（带 `reason` / `uri`）；其他 → 失败（消息为异常的 `message`）。
 */
export function attachBridgeNavigation(
  bridge: AppMcpBridge,
  handler: NavigationHandler,
  options: NavigationOptions = {},
  env: NavigationEnv = defaultEnv(),
): BridgeNavigation {
  let disposed = false
  const unsubscribe = bridge.onMessage((event) => {
    if (disposed || !isNavigateEvent(event)) return
    const request = { page: event.page, params: navigationParams(event.params) }
    void runNavigation(handler, request, options, env)
      .then((result) =>
        bridge.request(
          result.ok
            ? { op: 'navigate.result', navId: event.navId, ok: true }
            : {
                op: 'navigate.result',
                navId: event.navId,
                ok: false,
                kind: result.kind,
                message: result.message,
                ...(result.kind === 'USER_ACTION_REQUIRED' && { details: result.details }),
              },
        ),
      )
      .catch(() => {
        // @why 对方已停止或本页会话已注销：对方的等待随会话结束失败，这里无需处理
      })
  })
  const ready = bridge.request({ op: 'navigation.set', enabled: true }).then((reply) => {
    if (!reply.ok) throw new Error(reply.message)
  })
  return {
    ready,
    dispose() {
      if (disposed) return
      disposed = true
      unsubscribe()
      void bridge.request({ op: 'navigation.set', enabled: false }).catch(() => {})
    },
  }
}
