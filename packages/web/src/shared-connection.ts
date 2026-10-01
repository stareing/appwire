/**
 * 共享连接（spec/protocol.md 第 9 节）：同一来源的多个标签页共用一条到 Host 的 WebSocket，
 * 每个标签页仍是独立实例（各自 instanceId、各自的工具，随标签页存亡）。
 *
 * 持有连接的一方：
 * 1. SharedWorker（`mux-worker.ts`）——首选；
 * 2. 没有 SharedWorker（Chrome Android 148 之前等）但有 Web Locks 与 BroadcastChannel 时，选出的主标签页；
 * 3. 都没有时不共享，每个标签页直接连接（返回 undefined）。
 *
 * Host 不支持多路复用（旧 Host）时通道以 `unsupported` 关闭，驱动层改为直接连接。
 */

import { globalBroadcastChannel } from './instance-guard'
import { LeaderEndpoint, type LocksLike, portEndpoint, type SharedWorkerLike } from './mux/endpoints'
import { MuxLink } from './mux/link'
import { MuxOwner, type SocketLike } from './mux/owner'

export type SharedLinkFactory = () => MuxLink | undefined

/** 页面 CSP 拦截判定（主标签页模式下持有方运行在页面里，用页面的 `securitypolicyviolation` 结果）。 */
export type CspCheck = (url: string) => boolean

export function createSharedLink(cspBlocked?: CspCheck): MuxLink | undefined {
  const g = globalThis as {
    SharedWorker?: new (url: URL, options: { type: 'module'; name: string }) => SharedWorkerLike
    navigator?: { locks?: LocksLike }
    window?: EventTarget
    document?: EventTarget
  }
  if (typeof g.SharedWorker === 'function') {
    try {
      // 写法须保持字面量形式，打包器据此识别并输出 worker 文件（tsup 构建时改写为 ./mux-worker.js）。
      const worker = new SharedWorker(new URL('./mux-worker.ts', import.meta.url), { type: 'module', name: 'app-mcp' })
      return new MuxLink(portEndpoint(worker as unknown as SharedWorkerLike))
    } catch {
      // CSP worker-src 等：继续尝试选主
    }
  }
  const locks = g.navigator?.locks
  const createChannel = globalBroadcastChannel()
  if (locks && typeof locks.request === 'function' && createChannel) {
    const endpoint = new LeaderEndpoint({
      name: 'app-mcp:mux',
      createChannel,
      locks,
      createOwner: () =>
        new MuxOwner({
          createWebSocket: (url) => new WebSocket(url) as unknown as SocketLike,
          ...(cspBlocked && { cspBlocked }),
        }),
      win: g.window,
      doc: g.document,
    })
    return new MuxLink(endpoint)
  }
  return undefined
}
