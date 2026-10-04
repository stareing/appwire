/** @app-mcp/hub 的公开类型：只读结果缓存的上限与统计（第 16 项 O3，spec/hub-api.md 3.20）。 */

/**
 * 只读结果缓存的上限（`HubConfig.resultCache`）。每项可选，缺省取默认值；只在内存，Hub 重启清空。
 * 缓存只对 App 显式声明了 `cache` 的只读工具与资源生效（spec/protocol.md 3.6）。
 */
export interface ResultCacheConfig {
  /** 条目数上限，缺省 1024；超出时淘汰最久未用的条目。`0` 关闭缓存（不查、不存）。 */
  maxEntries?: number
  /** 全部条目的字节数上限（键 + 序列化后的结果），缺省 8 MiB；超出时淘汰最久未用的条目。 */
  maxBytes?: number
  /** 单个条目的字节数上限，缺省 64 KiB；超出的结果不存（并作废同键旧值）。 */
  maxEntryBytes?: number
}

/** 结果缓存的统计（`HubStatus.cache`，与 `app-mcp://apps/hub` 相同）。计数自 Hub 启动起累计。 */
export interface CacheStatus {
  /** 当前条目数（含尚未被访问判定的过期条目）。 */
  entries: number
  /** 当前条目的字节数合计。 */
  bytes: number
  /** 命中次数。 */
  hits: number
  /** 未命中次数（只计声明了 `cache` 的请求；绕过不计）。 */
  misses: number
  /** 因条数 / 字节上限淘汰的条目数（失效与过期不计）。 */
  evictions: number
  /** 生效上限（全部字段给出；`maxEntries: 0` 表示已关闭）。旧版 Hub 不报告时缺省。 */
  limits?: CacheLimits
}

/** 结果缓存的生效上限（`HubStatus.cache.limits`）。 */
export interface CacheLimits {
  maxEntries: number
  maxBytes: number
  maxEntryBytes: number
}
