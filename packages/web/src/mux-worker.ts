/**
 * SharedWorker 入口：同一来源的所有标签页共用这里的连接持有方（每个 Host 地址一条多路复用 WebSocket）。
 *
 * 由 `shared-connection.ts` 以 `new SharedWorker(new URL('./mux-worker.ts', import.meta.url), { type: 'module', name: 'app-mcp' })`
 * 创建；Vite / webpack 5 识别这种写法并单独打包本文件，`tsup` 构建时输出为 `dist/mux-worker.js`。
 *
 * Worker 的 CSP 来自其脚本响应头（不继承页面）；`connect-src` 拦截时 worker 全局对象上会派发
 * `securitypolicyviolation`，据此把通道失败报告为 CSP 拦截。
 */

import { MuxOwner, type SocketLike } from './mux/owner'

interface ConnectEvent {
  ports: ReadonlyArray<{
    postMessage(message: unknown): void
    onmessage: ((ev: { data: unknown }) => void) | null
    start(): void
  }>
}

interface ViolationEvent {
  blockedURI?: string
  effectiveDirective?: string
  violatedDirective?: string
}

interface SharedWorkerScope {
  onconnect: ((ev: ConnectEvent) => void) | null
  addEventListener(type: string, listener: (ev: unknown) => void): void
}

const scope = globalThis as unknown as SharedWorkerScope

/** 被 CSP 拦截的 Host（`host:port`）。 */
const cspBlocked = new Set<string>()

function hostOf(url: string): string | undefined {
  try {
    return new URL(url).host
  } catch {
    return undefined
  }
}

scope.addEventListener('securitypolicyviolation', (ev) => {
  const e = ev as ViolationEvent
  const directive = e.effectiveDirective ?? e.violatedDirective ?? ''
  if (!directive.startsWith('connect-src') && !directive.startsWith('default-src')) return
  const host = e.blockedURI ? hostOf(e.blockedURI) : undefined
  if (host) cspBlocked.add(host)
})

const owner = new MuxOwner({
  createWebSocket: (url) => new WebSocket(url) as unknown as SocketLike,
  cspBlocked: (url) => {
    const host = hostOf(url)
    return host !== undefined && cspBlocked.has(host)
  },
})

scope.onconnect = (ev) => {
  const port = ev.ports[0]
  if (!port) return
  const tab = owner.attach((message) => port.postMessage(message))
  port.onmessage = (e) => tab.receive(e.data)
  port.start()
}
