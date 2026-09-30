/**
 * instanceId 与配对 token 的持久化。
 *
 * - instanceId 存 `sessionStorage`：每个标签页一个，刷新后保持。
 * - token 存 `localStorage`：同一站点的所有标签页共享。
 * 存储不可用（隐私模式、沙箱 iframe、SSR）时退化为内存值。
 */

export const instanceIdKey = (appId: string): string => `app-mcp:${appId}:instance-id`
export const tokenKey = (appId: string): string => `app-mcp:${appId}:token`

type StorageName = 'sessionStorage' | 'localStorage'

function storage(name: StorageName): Storage | undefined {
  try {
    const s = (globalThis as Record<string, unknown>)[name] as Storage | undefined
    return s ?? undefined
  } catch {
    // 访问被拒绝（SecurityError）
    return undefined
  }
}

function get(name: StorageName, key: string): string | undefined {
  try {
    return storage(name)?.getItem(key) ?? undefined
  } catch {
    return undefined
  }
}

function set(name: StorageName, key: string, value: string): boolean {
  try {
    const s = storage(name)
    if (!s) return false
    s.setItem(key, value)
    return true
  } catch {
    return false
  }
}

export function randomId(): string {
  const c = (globalThis as { crypto?: Crypto }).crypto
  if (c && typeof c.randomUUID === 'function') return c.randomUUID()
  const bytes = new Uint8Array(16)
  if (c && typeof c.getRandomValues === 'function') c.getRandomValues(bytes)
  else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256)
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
}

/** 读取或生成本标签页的 instanceId。 */
export function loadInstanceId(appId: string): string {
  const key = instanceIdKey(appId)
  const existing = get('sessionStorage', key)
  if (existing) return existing
  const id = randomId()
  set('sessionStorage', key, id)
  return id
}

/** 覆盖本标签页的 instanceId（复制标签页冲突时重新生成）。 */
export function saveInstanceId(appId: string, id: string): boolean {
  return set('sessionStorage', instanceIdKey(appId), id)
}

export function loadToken(appId: string): string | undefined {
  return get('localStorage', tokenKey(appId)) || undefined
}

export function saveToken(appId: string, token: string): boolean {
  return set('localStorage', tokenKey(appId), token)
}
