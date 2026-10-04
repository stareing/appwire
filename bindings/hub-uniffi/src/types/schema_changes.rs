//! 工具定义的不兼容变化（第 16 项 O4，spec/hub-api.md 3.21）：`HubStatus.schema_changes` 的状态记录。

use app_mcp_hub as hub;

/// 变化的级别（兼容的变化不记录）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ChangeLevel {
    /// 破坏性：Agent 按旧定义的调用会出错。
    Breaking,
    /// 可能破坏：取决于调用方实际传入 / 读取的内容。
    Warning,
}

enum_map!(ChangeLevel <=> hub::ChangeLevel { Breaking, Warning });

/// 一条变化。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SchemaChange {
    pub level: ChangeLevel,
    /// JSON Pointer 风格的位置（相对工具定义），如 `/inputSchema/properties/to`。
    pub path: String,
    /// 面向开发者的说明。
    pub message: String,
}

/// 一个工具的一次不兼容变化：`level` 为 `changes` 中最高的级别，`at` 为 Hub 收到新声明的时刻（Unix 毫秒）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SchemaChangeRecord {
    pub app_id: String,
    /// 工具局部名。
    pub tool: String,
    pub level: ChangeLevel,
    pub changes: Vec<SchemaChange>,
    pub at: u64,
}

impl From<hub::SchemaChangeRecord> for SchemaChangeRecord {
    fn from(r: hub::SchemaChangeRecord) -> Self {
        SchemaChangeRecord {
            app_id: r.app_id,
            tool: r.tool,
            level: r.level.into(),
            changes: r
                .changes
                .into_iter()
                .map(|c| SchemaChange { level: c.level.into(), path: c.path, message: c.message })
                .collect(),
            at: r.at,
        }
    }
}
