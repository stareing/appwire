//! 只读结果缓存的上限：配置文件 `resultCache`、命令行 `--cache-max-*`（第 16 项 O3，spec/hub-api.md 3.20）。

use app_mcp_hub::CacheLimits;
use serde::{Deserialize, Serialize};

/// `resultCache` 分节：`{"maxEntries","maxBytes","maxEntryBytes"}`，缺省字段取默认值；`maxEntries: 0` 关闭缓存。
///
/// @invariant 未知字段报错（与 `limits` 一致），拼错的字段不会被静默忽略。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ResultCacheSection {
    /// 条目数上限，默认 1024；0 = 关闭缓存。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<usize>,
    /// 全部条目的字节数上限（键 + 序列化结果），默认 8 MiB。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    /// 单个条目的字节数上限，默认 64 KiB（未给出时不超过 `maxBytes`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_entry_bytes: Option<usize>,
}

impl ResultCacheSection {
    /// 合并：`other` 中给出的字段覆盖本对象（配置文件 ← 命令行）。
    pub fn merge(&mut self, other: &ResultCacheSection) {
        let take = |dst: &mut Option<usize>, src: Option<usize>| {
            if src.is_some() {
                *dst = src;
            }
        };
        take(&mut self.max_entries, other.max_entries);
        take(&mut self.max_bytes, other.max_bytes);
        take(&mut self.max_entry_bytes, other.max_entry_bytes);
    }

    /// 解析为生效上限（默认值与校验都在 Hub：[`CacheLimits::with_overrides`]、[`CacheLimits::validate`]）。
    ///
    /// @error 开启缓存（`maxEntries > 0`）时 `maxBytes` / `maxEntryBytes` 为 0，或 `maxEntryBytes` 大于 `maxBytes`。
    pub fn resolve(&self) -> anyhow::Result<CacheLimits> {
        let limits = CacheLimits::with_overrides(self.max_entries, self.max_bytes, self.max_entry_bytes);
        limits.validate().map_err(anyhow::Error::msg)?;
        Ok(limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(json: &str) -> ResultCacheSection {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn defaults_when_absent() {
        assert_eq!(ResultCacheSection::default().resolve().unwrap(), CacheLimits::default());
    }

    #[test]
    fn explicit_values_and_disable() {
        let l = section(r#"{"maxEntries":5,"maxBytes":4096,"maxEntryBytes":512}"#).resolve().unwrap();
        assert_eq!(l, CacheLimits { max_entries: 5, max_bytes: 4096, max_entry_bytes: 512 });
        let off = section(r#"{"maxEntries":0,"maxBytes":0}"#).resolve().unwrap();
        assert!(!off.enabled(), "关闭时不校验其余字段");
    }

    #[test]
    fn small_max_bytes_narrows_default_entry_limit() {
        let l = section(r#"{"maxBytes":1000}"#).resolve().unwrap();
        assert_eq!((l.max_bytes, l.max_entry_bytes), (1000, 1000));
    }

    #[test]
    fn invalid_values_are_rejected() {
        let err = |json: &str| section(json).resolve().unwrap_err().to_string();
        assert!(err(r#"{"maxBytes":0}"#).contains("resultCache.maxBytes 必须大于 0"));
        assert!(err(r#"{"maxEntryBytes":0}"#).contains("resultCache.maxEntryBytes 必须大于 0"));
        let e = err(r#"{"maxBytes":100,"maxEntryBytes":200}"#);
        assert!(e.contains("不能大于") && e.contains("200") && e.contains("100"), "{e}");
        assert!(serde_json::from_str::<ResultCacheSection>(r#"{"maxEntry":1}"#).is_err(), "未知字段");
        assert!(serde_json::from_str::<ResultCacheSection>(r#"{"maxEntries":-1}"#).is_err(), "负数");
    }

    #[test]
    fn merge_overrides_given_fields_only() {
        let mut s = section(r#"{"maxEntries":5,"maxBytes":4096}"#);
        s.merge(&ResultCacheSection { max_bytes: Some(8192), max_entry_bytes: Some(10), ..Default::default() });
        assert_eq!(s, section(r#"{"maxEntries":5,"maxBytes":8192,"maxEntryBytes":10}"#));
    }
}
