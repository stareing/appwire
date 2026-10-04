//! 只读结果缓存（第 16 项 O3，spec/hub-api.md 3.20）：App 在工具 / 资源声明中给出 `cache`（spec/protocol.md 3.6）时，
//! Hub 在 TTL 内复用结果，命中不唤醒 App、不发 `tools/invoke` / `resources/read`。缓存多久、能否跨调用方共用是 App 的策略，
//! Hub 只执行。
//!
//! - [`key`]：键（范围, appId, 工具局部名 + 规范化参数 | 资源名）与参数规范化。
//! - [`store`]：纯状态存储（不读时钟、不做 I/O）：LRU、条数 / 字节上限、惰性 TTL、失效与统计。
//! - [`hooks`]：接入调用与资源读取（查询、存入、失效），时钟取 `tokio::time::Instant`（测试可暂停）。
//!
//! @invariant 不新增定时器或线程：TTL 在访问时惰性判断，淘汰在插入时做（CLAUDE.md「召之即来」）。

use serde::{Deserialize, Serialize};

mod hooks;
mod key;
mod store;
#[cfg(test)]
mod tests;

pub(crate) use hooks::{CachedValue, ResourceCacheHint};
pub(crate) use key::CacheKey;
pub(crate) use store::ResultCache;

/// [`CacheLimits::max_entries`] 的默认值。
pub const DEFAULT_CACHE_MAX_ENTRIES: usize = 1024;
/// [`CacheLimits::max_bytes`] 的默认值（8 MiB）。
pub const DEFAULT_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// [`CacheLimits::max_entry_bytes`] 的默认值（64 KiB）。
pub const DEFAULT_CACHE_MAX_ENTRY_BYTES: usize = 64 * 1024;

/// 结果缓存的资源上限（`HubConfig::result_cache`，B-07）。只在内存，Hub 重启清空。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    /// 条目数上限；超出时淘汰最久未用的条目。`0` = 关闭缓存（不查、不存）。
    pub max_entries: usize,
    /// 全部条目的字节数上限（键 + 序列化后的结果）；超出时淘汰最久未用的条目。
    pub max_bytes: usize,
    /// 单个条目的字节数上限（键 + 序列化后的结果）；超出的结果不存。
    pub max_entry_bytes: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_CACHE_MAX_ENTRIES,
            max_bytes: DEFAULT_CACHE_MAX_BYTES,
            max_entry_bytes: DEFAULT_CACHE_MAX_ENTRY_BYTES,
        }
    }
}

impl CacheLimits {
    /// 缓存是否开启（`max_entries > 0`）。
    pub fn enabled(&self) -> bool {
        self.max_entries > 0
    }
}

/// 结果缓存的统计（`HubStatus.cache`、`app-mcp://apps/hub`）。计数自 Hub 启动起累计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CacheStatus {
    /// 当前条目数（含尚未被访问判定的过期条目）。
    pub entries: usize,
    /// 当前条目的字节数合计（键 + 序列化后的结果）。
    pub bytes: u64,
    /// 命中次数。
    pub hits: u64,
    /// 未命中次数（只计声明了 `cache` 的请求；绕过不计）。
    pub misses: u64,
    /// 因条数 / 字节上限淘汰的条目数（失效与过期不计）。
    pub evictions: u64,
}
