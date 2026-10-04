//! 结果缓存声明（spec/protocol.md 3.6，第 16 项 O3）：App 声明工具 / 资源结果在多长时间内可复用、能否跨调用方共用；
//! Hub 执行（spec/hub-api.md 3.20），SDK 只校验格式。

use super::*;

/// `ttlMs` 上限：24 小时。
///
/// @why 与休眠快照保留期（`dormant_ttl` 24 h）一致；更长的复用应由 App 自己持久化。
pub const MAX_CACHE_TTL_MS: u64 = 86_400_000;

/// 工具 / 资源的结果缓存声明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachePolicy {
    /// 1..=[`MAX_CACHE_TTL_MS`]。
    pub ttl_ms: u64,
    /// 缺省 [`CacheScope::Private`]（不序列化，`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "CacheScope::is_private")]
    pub scope: CacheScope,
}

/// 缓存范围。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheScope {
    /// 按调用方的记账主体隔离（缺省）。
    #[default]
    Private,
    /// 全体调用方共用：只用于与调用方无关的数据。
    Shared,
}

impl CacheScope {
    pub fn is_private(&self) -> bool {
        matches!(self, CacheScope::Private)
    }
}

impl CachePolicy {
    /// 格式校验。
    ///
    /// @error `ttlMs` 为 0 或超过 [`MAX_CACHE_TTL_MS`] → 原因文本（面向开发者）。
    pub fn validate(&self) -> Result<(), String> {
        if self.ttl_ms == 0 || self.ttl_ms > MAX_CACHE_TTL_MS {
            return Err(format!("cache.ttlMs 须在 1..={MAX_CACHE_TTL_MS} 之间（为 {}）", self.ttl_ms));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn serde_and_bounds() {
        let p: CachePolicy = serde_json::from_value(json!({"ttlMs": 5000})).unwrap();
        assert_eq!(p, CachePolicy { ttl_ms: 5000, scope: CacheScope::Private });
        assert_eq!(serde_json::to_value(p).unwrap(), json!({"ttlMs": 5000}));
        let shared = CachePolicy { ttl_ms: 1, scope: CacheScope::Shared };
        assert_eq!(serde_json::to_value(shared).unwrap(), json!({"ttlMs": 1, "scope": "shared"}));
        assert!(serde_json::from_value::<CachePolicy>(json!({"ttlMs": 1, "scope": "public"})).is_err());
        assert!(shared.validate().is_ok());
        assert!(CachePolicy { ttl_ms: MAX_CACHE_TTL_MS, ..p }.validate().is_ok());
        assert!(CachePolicy { ttl_ms: 0, ..p }.validate().is_err());
        assert!(CachePolicy { ttl_ms: MAX_CACHE_TTL_MS + 1, ..p }.validate().is_err());
    }
}
