//! 只读结果缓存（spec/hub-api.md 3.20）：上限覆盖与状态记录。

use app_mcp_hub as hub;

/// 结果缓存上限（`HubConfig.result_cache`）。为空的字段取默认值。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct CacheLimitOverrides {
    /// 条目数上限（默认 1024）；超出时淘汰最久未用的条目。`0` = 关闭缓存（不查、不存）。
    #[uniffi(default = None)]
    pub max_entries: Option<u32>,
    /// 全部条目的字节数上限（键 + 序列化后的结果，默认 8 MiB）。
    #[uniffi(default = None)]
    pub max_bytes: Option<u64>,
    /// 单个条目的字节数上限（键 + 序列化后的结果，默认 64 KiB）；超出的结果不存。
    #[uniffi(default = None)]
    pub max_entry_bytes: Option<u64>,
}

impl CacheLimitOverrides {
    pub(crate) fn apply(&self, target: &mut hub::CacheLimits) {
        if let Some(v) = self.max_entries {
            target.max_entries = v as usize;
        }
        if let Some(v) = self.max_bytes {
            target.max_bytes = usize::try_from(v).unwrap_or(usize::MAX);
        }
        if let Some(v) = self.max_entry_bytes {
            target.max_entry_bytes = usize::try_from(v).unwrap_or(usize::MAX);
        }
    }
}

/// `HubStatus.cache`：结果缓存的条目与命中统计。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct CacheStatus {
    /// 当前条目数（含尚未被访问判定的过期条目）。
    pub entries: u64,
    /// 当前条目的字节数合计（键 + 序列化后的结果）。
    pub bytes: u64,
    /// 命中次数。
    pub hits: u64,
    /// 未命中次数（只计声明了 `cache` 的请求；绕过不计）。
    pub misses: u64,
    /// 因条数 / 字节上限淘汰的条目数（失效与过期不计）。
    pub evictions: u64,
}

impl From<hub::CacheStatus> for CacheStatus {
    fn from(s: hub::CacheStatus) -> Self {
        CacheStatus {
            entries: u64::try_from(s.entries).unwrap_or(u64::MAX),
            bytes: s.bytes,
            hits: s.hits,
            misses: s.misses,
            evictions: s.evictions,
        }
    }
}
