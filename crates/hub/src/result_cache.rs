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
///
/// 序列化形式（`HubStatus.cache.limits`）：`{"maxEntries","maxBytes","maxEntryBytes"}`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

    /// 由可选覆盖得到上限（Host 配置 `resultCache`、各绑定的 `resultCache` 共用）：未给的字段取默认值。
    ///
    /// @why 只调小 `max_bytes` 时，未给出的单条上限随之收窄（不超过 `max_bytes`），不因用户未写的字段使 [`Self::validate`] 失败。
    pub fn with_overrides(max_entries: Option<usize>, max_bytes: Option<usize>, max_entry_bytes: Option<usize>) -> Self {
        let max_bytes = max_bytes.unwrap_or(DEFAULT_CACHE_MAX_BYTES);
        Self {
            max_entries: max_entries.unwrap_or(DEFAULT_CACHE_MAX_ENTRIES),
            max_bytes,
            max_entry_bytes: max_entry_bytes.unwrap_or(DEFAULT_CACHE_MAX_ENTRY_BYTES.min(max_bytes)),
        }
    }

    /// 校验上限（Hub 启动时，[`crate::Hub::start`]）；关闭缓存（`max_entries == 0`）时不校验其余字段。
    ///
    /// @error 开启时 `max_bytes` / `max_entry_bytes` 为 0，或 `max_entry_bytes` 大于 `max_bytes`；消息按配置形式
    /// （`resultCache.*`）称呼字段。
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        if self.max_bytes == 0 {
            return Err("resultCache.maxBytes 必须大于 0（关闭结果缓存请设 resultCache.maxEntries 为 0）".to_owned());
        }
        if self.max_entry_bytes == 0 {
            return Err("resultCache.maxEntryBytes 必须大于 0（关闭结果缓存请设 resultCache.maxEntries 为 0）".to_owned());
        }
        if self.max_entry_bytes > self.max_bytes {
            return Err(format!(
                "resultCache.maxEntryBytes（{}）不能大于 resultCache.maxBytes（{}）",
                self.max_entry_bytes, self.max_bytes
            ));
        }
        Ok(())
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
    /// 生效上限（[`CacheLimits`]，含关闭时的 `maxEntries: 0`）；旧版 Hub 不报告时为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<CacheLimits>,
}

impl CacheStatus {
    /// 是否尚无缓存活动（条目、字节与各计数都为 0；不看 `limits`）。
    pub fn is_idle(&self) -> bool {
        Self { limits: None, ..*self } == Self::default()
    }
}
