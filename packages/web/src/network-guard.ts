/**
 * 连接被浏览器拦截的诊断：Chrome 本地网络访问（Local Network Access，LNA）与 CSP `connect-src`。
 *
 * 浏览器拦截时 WebSocket 只报告一个不带原因的 `error` / `close`，与"Host 没有运行"无法区分。
 * 这里收集旁证，在连接失败后判断原因：
 *
 * - **LNA**（Chrome 142 起 fetch，147 起 WebSocket）：公网页面（或局域网页面）连接本机回环地址需要用户授权
 *   `loopback-network`（Chrome 145 起；之前及别名为 `local-network-access`），连接局域网地址需要 `local-network`。
 *   只有安全上下文（HTTPS）能获得授权，非安全上下文一律失败；页面本身在本机（localhost）时不受限制。
 *   权限经 `navigator.permissions.query()` 读取，`change` 事件通知授权变化。
 *   授权提示只能由页面（文档）发起：SharedWorker 中的请求要求来源**事先**已获授权（不会弹出提示）。
 * - **CSP**：`connect-src` 不允许 Host 地址时，文档（或 worker 全局对象）上派发 `securitypolicyviolation`。
 *
 * Private Network Access 的 CORS 预检（`Access-Control-Allow-Private-Network`）已被 LNA 的权限提示取代，
 * 且 WebSocket 从不预检，因此 Host 无需（也无法）通过响应头放行。
 */

import type { ConnectionBlockCause } from './types'

/** 地址空间（LNA 规范）：数值越小越"私有"。 */
type AddressSpace = 'loopback' | 'local' | 'public'
const RANK: Record<AddressSpace, number> = { loopback: 0, local: 1, public: 2 }

export type PermissionState = 'granted' | 'denied' | 'prompt'

/** `navigator.permissions` 的子集。 */
export interface PermissionsLike {
  query(descriptor: { name: string }): Promise<PermissionStatusLike>
}

export interface PermissionStatusLike {
  readonly state: string
  addEventListener?(type: 'change', listener: () => void): void
  removeEventListener?(type: 'change', listener: () => void): void
  onchange?: (() => void) | null
}

export interface ConnectionBlock {
  cause: ConnectionBlockCause
  message: string
}

export interface NetworkGuardEnv {
  /** 页面地址（`location.href`）。 */
  pageUrl?: string | undefined
  isSecureContext?: boolean | undefined
  permissions?: PermissionsLike | undefined
  /** 监听 `securitypolicyviolation` 的目标（document）。 */
  doc?: EventTarget | undefined
}

/** IP 字面量 / 主机名所属的地址空间（非 IP 字面量的主机名按公网处理，`localhost` 除外）。 */
export function addressSpaceOf(hostname: string): AddressSpace {
  const h = hostname.toLowerCase().replace(/^\[|\]$/g, '')
  if (h === 'localhost' || h.endsWith('.localhost')) return 'loopback'
  if (h === '::1' || h === '0:0:0:0:0:0:0:1') return 'loopback'
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(h)
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])]
    if (a === 127) return 'loopback'
    if (a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254)) return 'local'
    return 'public'
  }
  if (/^f[cd][0-9a-f]{2}:/.test(h) || /^fe[89ab][0-9a-f]:/.test(h)) return 'local'
  return 'public'
}

function hostOf(url: string): URL | undefined {
  try {
    return new URL(url)
  } catch {
    return undefined
  }
}

/** 主机标识（`host:port`，用于比对 CSP 违规的 blockedURI）。 */
export function hostKey(url: string): string | undefined {
  return hostOf(url)?.host
}

export class NetworkGuard {
  /** LNA 是否适用于本页 → Host 的连接。 */
  readonly lnaApplies: boolean
  private permission: PermissionState | undefined
  private readonly ready: Promise<void>
  private readonly cspHosts = new Set<string>()
  private readonly offs: Array<() => void> = []
  private disposed = false

  constructor(
    private readonly hostUrl: string,
    private readonly env: NetworkGuardEnv,
    /** 授权状态变化（如用户在网站设置中改为允许）。 */
    private readonly onPermissionChange: (state: PermissionState) => void,
  ) {
    const target = hostOf(hostUrl)
    const page = env.pageUrl ? hostOf(env.pageUrl) : undefined
    const targetSpace = target ? addressSpaceOf(target.hostname) : 'public'
    const pageSpace = page && /^https?:$/.test(page.protocol) ? addressSpaceOf(page.hostname) : undefined
    this.lnaApplies = pageSpace !== undefined && RANK[targetSpace] < RANK[pageSpace]
    this.ready = this.lnaApplies ? this.queryPermission(targetSpace) : Promise.resolve()

    const doc = env.doc
    if (doc) {
      const onViolation = (ev: Event): void => this.onViolation(ev)
      doc.addEventListener('securitypolicyviolation', onViolation)
      this.offs.push(() => doc.removeEventListener('securitypolicyviolation', onViolation))
    }
  }

  /** 权限查询完成（不适用或不支持时立即完成）。 */
  whenReady(): Promise<void> {
    return this.ready
  }

  /** 当前 LNA 授权；不适用或浏览器不支持该权限名时为 undefined。 */
  get lnaPermission(): PermissionState | undefined {
    return this.permission
  }

  /** 页面 CSP 是否拦截了到 `url` 的连接（已观察到违规事件）。 */
  cspBlocks(url: string): boolean {
    const key = hostKey(url)
    return key !== undefined && this.cspHosts.has(key)
  }

  /**
   * 连接（在打开前）失败后调用：判断是否被浏览器拦截。`workerCsp` 为共享连接持有方报告的 CSP 拦截。
   *
   * 授权为 `prompt` 时不判定为拦截：实测（Chrome 153，2026-10）该状态下 WebSocket 仍可能直接连通，
   * 此时失败与"Host 未运行"无法区分，按普通断开退避重连。
   */
  diagnose(workerCsp = false): ConnectionBlock | undefined {
    if (workerCsp || this.cspBlocks(this.hostUrl)) {
      const where = workerCsp ? '共享连接 SharedWorker 脚本响应' : '页面'
      return {
        cause: 'csp',
        message:
          `${where}的内容安全策略（CSP）不允许连接 ${this.hostUrl}：请在 connect-src 中加入 ${this.hostUrl}` +
          '（如 `connect-src \'self\' ws://127.0.0.1:7717`）。修改策略后刷新页面生效。',
      }
    }
    if (!this.lnaApplies || this.permission === undefined) return undefined
    if (this.env.isSecureContext === false) {
      return {
        cause: 'insecure-context',
        message:
          '页面不是安全上下文（非 HTTPS），Chrome 的本地网络访问限制禁止它连接本机的 app-mcp Host，且不会询问用户。' +
          '请改用 HTTPS 部署，或在 localhost 上打开页面。',
      }
    }
    if (this.permission === 'denied') {
      return {
        cause: 'local-network-access',
        message:
          '浏览器已禁止本网站访问本机应用（本地网络访问权限）。请点击地址栏左侧的网站信息图标 → 网站设置，' +
          '把「本机上的应用」（Apps on device / 本地网络访问）改为允许；授权后自动重新连接。',
      }
    }
    return undefined
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    for (const off of this.offs.splice(0)) off()
  }

  private onViolation(ev: Event): void {
    const e = ev as Event & { blockedURI?: string; effectiveDirective?: string; violatedDirective?: string }
    const directive = e.effectiveDirective ?? e.violatedDirective ?? ''
    if (!directive.startsWith('connect-src') && !directive.startsWith('default-src')) return
    const key = e.blockedURI ? hostKey(e.blockedURI) : undefined
    if (key) this.cspHosts.add(key)
  }

  private async queryPermission(target: AddressSpace): Promise<void> {
    const permissions = this.env.permissions
    if (!permissions || typeof permissions.query !== 'function') return
    // 新名称优先，旧名称 `local-network-access`（Chrome 142–144，之后为别名）其次；不认识的名称会抛 TypeError。
    const names = target === 'loopback' ? ['loopback-network', 'local-network-access'] : ['local-network', 'local-network-access']
    for (const name of names) {
      let status: PermissionStatusLike
      try {
        status = await permissions.query({ name })
      } catch {
        continue
      }
      if (this.disposed) return
      this.permission = normalize(status.state)
      const onChange = (): void => {
        const next = normalize(status.state)
        if (next === undefined || next === this.permission) return
        this.permission = next
        this.onPermissionChange(next)
      }
      if (typeof status.addEventListener === 'function') {
        status.addEventListener('change', onChange)
        this.offs.push(() => status.removeEventListener?.('change', onChange))
      } else {
        status.onchange = onChange
        this.offs.push(() => {
          status.onchange = null
        })
      }
      return
    }
  }
}

function normalize(state: string): PermissionState | undefined {
  return state === 'granted' || state === 'denied' || state === 'prompt' ? state : undefined
}
