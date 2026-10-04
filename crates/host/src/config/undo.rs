//! 撤销记录的上限：配置文件 `undo`、命令行 `--undo-ttl-ms` / `--undo-max-per-task`（第 15 项 X2，spec/hub-api.md 3.23）。

use app_mcp_hub::UndoLimits;
use serde::{Deserialize, Serialize};

/// `undo` 分节：`{"ttlMs","maxPerTask"}`，缺省字段取默认值；`maxPerTask: 0` 关闭撤销。
///
/// @invariant 未知字段报错（与 `resultCache` 一致），拼错的字段不会被静默忽略。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct UndoSection {
    /// 记录自登记起的有效期（毫秒），默认 1800000（30 分钟）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    /// 每个 Agent 任务保留的记录数上限，默认 32；0 = 关闭撤销。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_per_task: Option<usize>,
}

impl UndoSection {
    /// 合并：`other` 中给出的字段覆盖本对象（配置文件 ← 命令行）。
    pub fn merge(&mut self, other: &UndoSection) {
        if other.ttl_ms.is_some() {
            self.ttl_ms = other.ttl_ms;
        }
        if other.max_per_task.is_some() {
            self.max_per_task = other.max_per_task;
        }
    }

    /// 解析为生效上限（默认值与校验都在 Hub：[`UndoLimits::with_overrides`]、[`UndoLimits::validate`]）。
    ///
    /// @error 开启撤销（`maxPerTask > 0`）时 `ttlMs` 为 0。
    pub fn resolve(&self) -> anyhow::Result<UndoLimits> {
        let limits = UndoLimits::with_overrides(self.ttl_ms, self.max_per_task);
        limits.validate().map_err(anyhow::Error::msg)?;
        Ok(limits)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn section(json: &str) -> UndoSection {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn defaults_explicit_and_disable() {
        assert_eq!(UndoSection::default().resolve().unwrap(), UndoLimits::default());
        let l = section(r#"{"ttlMs":5000,"maxPerTask":4}"#).resolve().unwrap();
        assert_eq!((l.ttl, l.max_per_task), (Duration::from_secs(5), 4));
        let off = section(r#"{"ttlMs":0,"maxPerTask":0}"#).resolve().unwrap();
        assert!(!off.enabled(), "关闭时不校验 TTL");
    }

    #[test]
    fn invalid_values_are_rejected() {
        let e = section(r#"{"ttlMs":0}"#).resolve().unwrap_err().to_string();
        assert!(e.contains("undo.ttlMs 必须大于 0"), "{e}");
        assert!(serde_json::from_str::<UndoSection>(r#"{"ttl":1}"#).is_err(), "未知字段");
        assert!(serde_json::from_str::<UndoSection>(r#"{"maxPerTask":-1}"#).is_err(), "负数");
    }

    #[test]
    fn merge_overrides_given_fields_only() {
        let mut s = section(r#"{"ttlMs":1000,"maxPerTask":5}"#);
        s.merge(&UndoSection { max_per_task: Some(0), ..Default::default() });
        assert_eq!(s, section(r#"{"ttlMs":1000,"maxPerTask":0}"#));
        s.merge(&UndoSection { ttl_ms: Some(9), ..Default::default() });
        assert_eq!(s, section(r#"{"ttlMs":9,"maxPerTask":0}"#));
    }
}
