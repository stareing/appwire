//! 配置 `resultCache`（spec/hub-api.md 3.20）：只读结果缓存上限的可选覆盖。

use app_mcp_hub::CacheLimits;
use serde::Deserialize;

/// [`CacheLimits`] 的可选覆盖（camelCase）；缺省字段沿用默认值（1024 条 / 8 MiB / 64 KiB）。`maxEntries: 0` 关闭缓存。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CacheLimitOverrides {
    /// 条目数上限；`0` = 关闭缓存。
    max_entries: Option<usize>,
    /// 全部条目的字节数上限（键 + 序列化后的结果）。
    max_bytes: Option<usize>,
    /// 单个条目的字节数上限；超出的结果不存。
    max_entry_bytes: Option<usize>,
}

impl CacheLimitOverrides {
    pub(crate) fn apply(&self, target: &mut CacheLimits) {
        *target = CacheLimits::with_overrides(self.max_entries, self.max_bytes, self.max_entry_bytes);
    }
}
