//! 运行时告警记录（`HubStatus.schema_changes`）：最近 [`MAX_SCHEMA_CHANGES`] 条，旧的在前，只在内存。

use std::collections::VecDeque;

use app_mcp_protocol::schema_compat::{ChangeLevel, SchemaChange};
use serde::{Deserialize, Serialize};

/// [`crate::HubStatus::schema_changes`] 保留的条数。
pub const MAX_SCHEMA_CHANGES: usize = 32;

/// 一个工具的一次不兼容变化（spec/hub-api.md 3.21）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaChangeRecord {
    pub app_id: String,
    /// 工具局部名。
    pub tool: String,
    /// `changes` 中最高的级别（有任一 `breaking` 即为 `breaking`）。
    pub level: ChangeLevel,
    /// 破坏性与可能破坏的变化（兼容的变化不列出），按位置顺序。
    pub changes: Vec<SchemaChange>,
    /// Hub 收到新声明的时刻（Unix 毫秒）。
    pub at: u64,
}

impl SchemaChangeRecord {
    /// 由比较结果构造；没有不兼容变化时为 `None`（不记录）。
    pub(crate) fn new(app_id: &str, tool: &str, changes: Vec<SchemaChange>, at: u64) -> Option<Self> {
        let level = if changes.iter().any(|c| c.level == ChangeLevel::Breaking) {
            ChangeLevel::Breaking
        } else {
            changes.first()?.level
        };
        Some(Self { app_id: app_id.to_owned(), tool: tool.to_owned(), level, changes, at })
    }
}

/// 有上限的记录队列（[`MAX_SCHEMA_CHANGES`]，满时丢最旧的）。
#[derive(Debug, Default)]
pub(crate) struct SchemaChangeLog {
    records: VecDeque<SchemaChangeRecord>,
}

impl SchemaChangeLog {
    pub(crate) fn push(&mut self, record: SchemaChangeRecord) {
        if self.records.len() >= MAX_SCHEMA_CHANGES {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    /// 全部记录，旧的在前。
    pub(crate) fn snapshot(&self) -> Vec<SchemaChangeRecord> {
        self.records.iter().cloned().collect()
    }
}
