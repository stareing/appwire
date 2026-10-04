//! 接入点：`tools/sync` / `tools/changed` 写注册表前比较同名工具的新旧定义，有不兼容变化时记 `warn` 日志并记入
//! [`super::SchemaChangeLog`]。照常发 `list_changed`（调用方不受影响）。

use app_mcp_protocol::schema_compat::compare_tool;

use crate::hub::{HubShared, lock, unix_millis};
use crate::tool_def::SharedTool;

use super::SchemaChangeRecord;

impl HubShared {
    /// 记录同名工具相对此前已知定义的不兼容变化。
    ///
    /// @input pairs `(此前已知, 新)`，由 [`crate::registry::Registry::redefined_tools`] 在写注册表前取得（只含定义不等的同名工具）。
    /// @side-effect 有不兼容变化时记 `warn` 日志、追加到 `HubStatus.schema_changes`。
    pub(crate) fn note_schema_changes(&self, app_id: &str, pairs: Vec<(SharedTool, SharedTool)>) {
        for (old, new) in pairs {
            let changes = compare_tool(&old.to_info(), &new.to_info());
            let Some(record) = SchemaChangeRecord::new(app_id, &new.name, changes, unix_millis()) else {
                continue;
            };
            let summary: Vec<String> = record.changes.iter().map(|c| format!("{} {}", c.path, c.message)).collect();
            tracing::warn!(
                app_id,
                tool = %new.name,
                level = record.level.as_str(),
                changes = %summary.join("；"),
                "工具定义出现不兼容变化：同名工具的 schema 变更可能使 Agent 已有的调用出错（不兼容变更应使用新工具名，spec/protocol.md 3.7）"
            );
            lock(&self.schema_changes).push(record);
        }
    }

    /// `HubStatus.schema_changes`：最近的记录，旧的在前。
    pub(crate) fn schema_change_records(&self) -> Vec<SchemaChangeRecord> {
        lock(&self.schema_changes).snapshot()
    }
}
