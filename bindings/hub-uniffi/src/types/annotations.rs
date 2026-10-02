//! MCP 工具注解与内容注解。

use app_mcp_hub as hub;

use super::Audience;

/// 标准 MCP 工具注解。为空 = 未声明。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct ToolAnnotations {
    /// 给人看的工具标题。
    #[uniffi(default = None)]
    pub title: Option<String>,
    /// 不修改任何状态。
    #[uniffi(default = None)]
    pub read_only_hint: Option<bool>,
    /// 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。
    #[uniffi(default = None)]
    pub destructive_hint: Option<bool>,
    /// 以相同参数重复调用没有额外效果（只在非只读时有意义）。
    #[uniffi(default = None)]
    pub idempotent_hint: Option<bool>,
    /// 会与外部世界交互。
    #[uniffi(default = None)]
    pub open_world_hint: Option<bool>,
}

impl From<hub::ToolAnnotations> for ToolAnnotations {
    fn from(a: hub::ToolAnnotations) -> Self {
        ToolAnnotations {
            title: a.title,
            read_only_hint: a.read_only_hint,
            destructive_hint: a.destructive_hint,
            idempotent_hint: a.idempotent_hint,
            open_world_hint: a.open_world_hint,
        }
    }
}

/// 内容标注（MCP 内容注解），Hub 原样转发 App 的声明。
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct ContentAnnotations {
    #[uniffi(default = None)]
    pub audience: Option<Vec<Audience>>,
    /// 重要程度，0（可选）到 1（必需）。
    #[uniffi(default = None)]
    pub priority: Option<f64>,
    /// 最后修改时刻（ISO 8601）。
    #[uniffi(default = None)]
    pub last_modified: Option<String>,
}

impl From<hub::ContentAnnotations> for ContentAnnotations {
    fn from(a: hub::ContentAnnotations) -> Self {
        ContentAnnotations {
            audience: a.audience.map(|v| v.into_iter().map(Into::into).collect()),
            priority: a.priority,
            last_modified: a.last_modified,
        }
    }
}
