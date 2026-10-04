//! 工具演进（第 16 项 O4，spec/hub-api.md 3.21）：工具弃用声明 [`Deprecation`] 与 `HubStatus.schema_changes` 的状态记录。

use app_mcp_hub as hub;

/// App 工具的弃用声明（原样，spec/protocol.md 3.7）。弃用的工具照常列出与调用，Hub 不拦截。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Deprecation {
    /// 面向模型：为什么弃用、该怎么做。
    pub message: String,
    /// 替代工具在同一 App 中的局部名；未声明时为空。
    #[uniffi(default = None)]
    pub replacement: Option<String>,
    /// 计划移除的日期 `YYYY-MM-DD`（只作提示）；未声明时为空。
    #[uniffi(default = None)]
    pub until: Option<String>,
}

impl From<app_mcp_protocol::Deprecation> for Deprecation {
    fn from(d: app_mcp_protocol::Deprecation) -> Self {
        Deprecation { message: d.message, replacement: d.replacement, until: d.until }
    }
}

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
