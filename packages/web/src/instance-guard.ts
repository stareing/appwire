/**
 * 复制标签页的 instanceId 冲突检测。
 *
 * instanceId 存在 `sessionStorage` 中；浏览器“复制标签页”会连同 `sessionStorage` 一起复制，
 * 两个标签页因此拿到相同的 instanceId，Host 会把它们当成同一个实例（后连接的顶掉先连接的）。
 *
 * 做法：每个 SDK 实例按 appId 打开一个 `BroadcastChannel`，
 * - 启动时广播 `probe`（带自己的 instanceId），在 {@link DEFAULT_PROBE_WINDOW_MS} 内等待回复；
 * - 始终监听其他实例的 `probe`，若 instanceId 与自己相同则回复 `taken`；
 * - 收到 `taken` 的一方（后启动的标签页）重新生成 instanceId 并写回 `sessionStorage`。
 * 探测与 WASM 加载并行进行，只在核心创建（`app/hello` 使用 instanceId）之前生效。
 * 环境没有 `BroadcastChannel` 时不做检测（退化为原有行为）。
 */

import { randomId, saveInstanceId } from './storage'

/** 驱动层需要的 BroadcastChannel 子集（便于测试替换）。 */
export interface BroadcastChannelLike {
  postMessage(message: unknown): void
  onmessage: ((event: { data: unknown }) => void) | null
  close(): void
}

export type BroadcastChannelFactory = (name: string) => BroadcastChannelLike

/** 默认等待回复的时间。同一浏览器内的 BroadcastChannel 投递通常在几毫秒内完成。 */
export const DEFAULT_PROBE_WINDOW_MS = 60

export const instanceChannelName = (appId: string): string => `app-mcp:${appId}:instance`

type Message = { t: 'probe'; id: string; from: string } | { t: 'taken'; id: string; to: string }

function isMessage(data: unknown): data is Message {
  if (typeof data !== 'object' || data === null) return false
  const m = data as Record<string, unknown>
  return (
    typeof m.id === 'string' &&
    ((m.t === 'probe' && typeof m.from === 'string') || (m.t === 'taken' && typeof m.to === 'string'))
  )
}

/** 全局 BroadcastChannel 构造器；不存在或构造失败时返回 undefined。 */
export function globalBroadcastChannel(): BroadcastChannelFactory | undefined {
  const Ctor = (globalThis as { BroadcastChannel?: new (name: string) => BroadcastChannelLike }).BroadcastChannel
  if (typeof Ctor !== 'function') return undefined
  return (name) => new Ctor(name)
}

export interface InstanceGuardOptions {
  appId: string
  instanceId: string
  createChannel?: BroadcastChannelFactory | undefined
  windowMs?: number
  /** 冲突已解决（instanceId 已重新生成）时回调。 */
  onRegenerated?: (previous: string, next: string) => void
  /** 核心创建之后才发现的冲突（无法再更换 instanceId）。 */
  onLateConflict?: (id: string) => void
}

export class InstanceGuard {
  private current: string
  private readonly nonce = randomId()
  private channel: BroadcastChannelLike | undefined
  private probing = false
  private probed: Promise<string> | undefined
  private locked = false
  private disposed = false
  private readonly options: InstanceGuardOptions

  constructor(options: InstanceGuardOptions) {
    this.options = options
    this.current = options.instanceId
    if (!options.createChannel) return
    try {
      this.channel = options.createChannel(instanceChannelName(options.appId))
    } catch {
      this.channel = undefined
      return
    }
    this.channel.onmessage = (event) => this.onMessage(event.data)
  }

  get instanceId(): string {
    return this.current
  }

  /** 是否启用了检测（有可用的 BroadcastChannel）。 */
  get active(): boolean {
    return this.channel !== undefined
  }

  /**
   * 广播探测并等待回复窗口结束，返回最终的 instanceId。没有 BroadcastChannel 时立即返回。
   * 窗口结束后 instanceId 锁定（之后发现的冲突只回调 `onLateConflict`）。
   */
  probe(): Promise<string> {
    if (this.probed) return this.probed
    const channel = this.channel
    if (!channel || this.disposed) {
      this.locked = true
      this.probed = Promise.resolve(this.current)
      return this.probed
    }
    this.probing = true
    this.post({ t: 'probe', id: this.current, from: this.nonce })
    this.probed = new Promise((resolve) => {
      setTimeout(() => {
        this.probing = false
        this.locked = true
        resolve(this.current)
      }, this.options.windowMs ?? DEFAULT_PROBE_WINDOW_MS)
    })
    return this.probed
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    const channel = this.channel
    this.channel = undefined
    if (!channel) return
    channel.onmessage = null
    try {
      channel.close()
    } catch {
      // 忽略
    }
  }

  private post(message: Message): void {
    try {
      this.channel?.postMessage(message)
    } catch {
      // 忽略（通道已关闭等）
    }
  }

  private onMessage(data: unknown): void {
    if (this.disposed || !isMessage(data)) return
    if (data.t === 'probe') {
      if (data.from !== this.nonce && data.id === this.current) this.post({ t: 'taken', id: data.id, to: data.from })
      return
    }
    if (data.to !== this.nonce || data.id !== this.current) return
    if (this.locked || !this.probing) {
      this.options.onLateConflict?.(data.id)
      return
    }
    const previous = this.current
    this.current = randomId()
    saveInstanceId(this.options.appId, this.current)
    this.options.onRegenerated?.(previous, this.current)
    // 新 ID 也可能与其他标签页冲突（极少见）：再探测一次。
    this.post({ t: 'probe', id: this.current, from: this.nonce })
  }
}
