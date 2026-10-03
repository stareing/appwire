/**
 * JS 驱动层：把 sans-IO 核心（{@link CoreClient}）接到浏览器的 WebSocket、定时器、
 * 页面可见性与 JS handler 上。
 *
 * 结构：
 * - 注册操作先进入 {@link AppMcpDriver.ops} 队列，核心加载完成后按顺序执行；
 *   需要异步转换 schema 的操作会阻塞其后的操作，保证顺序与加载前后行为一致。
 * - 每次向核心输入（`handle*` / `complete*` / 注册）后调用 {@link AppMcpDriver.pump}：
 *   取尽事件并处理，然后按 `pollTimeout()` 重设定时器。
 *
 * 实现按职责拆在 `driver/` 下，自下而上逐层继承：
 * - driver/base.ts：状态字段、核心加载、操作队列与事件循环；
 * - driver/transport.ts：WebSocket / 共享连接、拦截、可见性、页面生命周期、唤醒交接、导航；
 * - driver/registry.ts：调用执行与工具 / 资源 / scope 注册；
 * - 本文件：对外的 {@link AppMcp} API。
 */

import { ToolCallError } from './types'
import type {
  AppMcp,
  AppMcpOptions,
  ConnectionState,
  HoldHandle,
  LazyToolDefinition,
  NavigationHandler,
  NavigationOptions,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ScopeOptions,
  ToolDefinition,
  ToolHandle,
} from './types'
import { DriverRegistry } from './driver/registry'
import { type AnyDef, type DriverDeps } from './driver/shared'

export {
  DEFAULT_HOST_URL,
  DEFAULT_HOST_URLS,
  type DriverDeps,
  parseWakeTokenJs,
  SDK_VERSION,
  stripWakeFragment,
  type WebSocketFactory,
  type WebSocketLike,
} from './driver/shared'

// ---------------------------------------------------------------------------
// 驱动
// ---------------------------------------------------------------------------

export class AppMcpDriver extends DriverRegistry implements AppMcp {
  // ---- AppMcp ---------------------------------------------------------

  get state(): ConnectionState {
    return this.currentState
  }

  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  tool<I = unknown, O = unknown>(name: string, definition: ToolDefinition<I, O> | LazyToolDefinition<I, O>): ToolHandle {
    return this.registerTool(name, definition as AnyDef, undefined)
  }

  resource<T = unknown>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return this.registerResource(name, definition, undefined)
  }

  scope(name: string, options?: ScopeOptions): Scope {
    return this.createScope(name, undefined, options)
  }

  setNavigationHandler(handler: NavigationHandler | null, options: NavigationOptions = {}): void {
    if (this.disposed) return
    this.navHandler = handler
    this.navOptions = options
    if (!this.core) return
    if (handler && !this.navDeclared && this.currentState.status === 'connected') {
      this.log.warn(`${this.tag} 导航回调在连接建立后才设置：本次连接未声明导航能力，下次连接时生效`)
    }
    this.input((c) => c.setNavigation(handler !== null))
  }

  setNavigateInBackground(enabled: boolean): void {
    if (this.disposed) return
    this.navigateInBackground = enabled
    if (this.core) this.input((c) => c.setNavigateInBackground(enabled))
  }

  wake(): void {
    if (this.retryBlocked()) return
    this.lifecycleOp((c) => c.wake(this.now()))
  }

  sleep(): void {
    this.lifecycleOp((c) => c.sleep(this.now()))
  }

  connectNow(): void {
    if (this.retryBlocked()) return
    this.lifecycleOp((c) => c.connectNow(this.now()))
  }

  hold(): HoldHandle {
    return this.acquireHold(undefined)
  }

  dispose(): void {
    if (this.disposed) return
    const core = this.core
    if (core) {
      try {
        core.stop(this.now())
      } catch (e) {
        this.log.error(`${this.tag} 停止核心失败`, e)
      }
      this.pump()
    }
    this.disposed = true
    this.ops.length = 0
    for (const c of this.calls.values()) c.abort(new ToolCallError('CANCELLED', 'SDK 已停止'))
    this.calls.clear()
    this.pendingLifecycle.length = 0
    this.closeSocket()
    this.clearTimer()
    this.visibility.dispose()
    for (const off of this.pageListeners.splice(0)) off()
    this.guard.dispose()
    this.handoff?.dispose()
    this.net.dispose()
    this.blocked = undefined
    clearTimeout(this.blockedTimer)
    this.link?.dispose()
    this.link = null
    for (const rec of [...this.toolNames.values()]) this.forgetTool(rec)
    this.hubListeners.clear()
    this.hub.yieldable.clear()
    this.toolNames.clear()
    this.resourceNames.clear()
    this.toolsByCoreId.clear()
    this.resourcesByCoreId.clear()
    this.core = undefined
    try {
      core?.free?.()
    } catch {
      // 忽略
    }
    this.setState({ status: 'stopped' })
    this.listeners.clear()
  }
}

export function createDriver(options: AppMcpOptions, deps: DriverDeps): AppMcp {
  return new AppMcpDriver(options, deps)
}
