/**
 * Host 地址 → 传输类别（spec/lifecycle.md 第 11 节），交给核心决定是否发心跳。
 *
 * 与原生 SDK 的 `Endpoint::transport_kind`（crates/protocol）同一规则的网页部分：回环主机（`localhost`、`127.0.0.0/8`、
 * `::1`）为 `loopback`（本机 Host 退出时浏览器立即收到 close），其他为 `remote`。网页没有 IPC；经 SharedWorker
 * 的共享连接仍是到本机回环的 WebSocket，同样按 `loopback` 处理。
 *
 * @why 网页拿不到"是否经 adb reverse 等转发"的信息：手机浏览器访问本机回环属转发场景时，用
 * `heartbeat: 'always'` 打开心跳。
 */

export type HostTransport = 'loopback' | 'remote'

/** 主机名是否为本机回环。 */
export function isLoopbackHost(host: string): boolean {
  const h = host.replace(/^\[|\]$/g, '').toLowerCase()
  if (h === 'localhost' || h === '::1') return true
  const m = /^127\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(h)
  return m !== null && m.slice(1).every((p) => Number(p) <= 255)
}

/** 全部候选地址都是回环时为 `loopback`，否则（含无法解析）为 `remote`。 */
export function hostTransport(urls: readonly string[]): HostTransport {
  const loopback = urls.every((u) => {
    try {
      return isLoopbackHost(new URL(u).hostname)
    } catch {
      return false
    }
  })
  return loopback && urls.length > 0 ? 'loopback' : 'remote'
}
