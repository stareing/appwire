//! 工具定义的兼容判定（第 16 项 O4）：同一 App 同名工具新旧两版之间的破坏性 / 可能破坏的变化。
//!
//! 规则的权威表述见 spec/manifest.md 第 6 节，这里是唯一实现，供 `app-mcp-host validate --against` 与 Hub 运行时告警
//! （spec/hub-api.md 3.21）共用。纯函数、无 I/O；兼容的变化不列出。
//!
//! - [`compare_tool`]：工具本身（`surface`、生效注解、`risk`）+ `inputSchema` / `outputSchema`（[`schema`] 子模块）。
//! - [`compare_tools`]：两组工具，含删除判定。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Risk, ToolInfo, ToolSurface};

mod schema;

pub use schema::MAX_SCHEMA_DEPTH;

/// 变化的级别。兼容的变化不列出。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeLevel {
    /// 破坏性：旧调用方 / 旧读取方会出错，必须改用新工具名（spec/protocol.md 3.7）。
    Breaking,
    /// 可能破坏：取决于调用方实际传入 / 读取的内容。
    Warning,
}

impl ChangeLevel {
    /// 序列化取值（`breaking` / `warning`）。
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeLevel::Breaking => "breaking",
            ChangeLevel::Warning => "warning",
        }
    }
}

/// 一条变化。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaChange {
    pub level: ChangeLevel,
    /// JSON Pointer 风格的位置，相对工具定义，如 `/inputSchema/properties/to`、`/surface`；整个工具为空串。
    pub path: String,
    /// 面向开发者的说明（中文）。
    pub message: String,
}

impl SchemaChange {
    pub(crate) fn new(level: ChangeLevel, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { level, path: path.into(), message: message.into() }
    }
}

/// 一个工具的变化（[`compare_tools`]）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolChanges {
    /// 工具局部名。
    pub tool: String,
    /// 新版中已删除。删除已标 `deprecated` 的工具时 `changes` 为空（只是提示，不算破坏）。
    pub removed: bool,
    pub changes: Vec<SchemaChange>,
}

impl ToolChanges {
    /// 是否含破坏性变化。
    pub fn has_breaking(&self) -> bool {
        self.changes.iter().any(|c| c.level == ChangeLevel::Breaking)
    }
}

/// 同名工具新旧定义的比较（spec/manifest.md 第 6 节「工具本身」「inputSchema」「outputSchema」三行）。
///
/// @output 按位置顺序：工具本身、`inputSchema`、`outputSchema`；无变化或只有兼容变化时为空。
/// @invariant 不 panic；schema 嵌套超过 [`MAX_SCHEMA_DEPTH`] 层时记一条 Warning 并停止深入。
pub fn compare_tool(old: &ToolInfo, new: &ToolInfo) -> Vec<SchemaChange> {
    let mut out = tool_level_changes(old, new);
    schema::compare_input(&old.input_schema, &new.input_schema, &mut out);
    match (&old.output_schema, &new.output_schema) {
        (Some(o), Some(n)) => schema::compare_output(o, n, &mut out),
        (Some(_), None) => out.push(SchemaChange::new(
            ChangeLevel::Warning,
            "/outputSchema",
            "不再声明 outputSchema：调用方无法再依赖结果结构",
        )),
        _ => {}
    }
    out
}

/// 两组工具（同一 App）的比较：删除（未经弃用为 Breaking；已标 `deprecated` 的删除不算破坏）与同名工具的变化；
/// 新增与无变化的工具不列出。
///
/// @output 按工具名排序。同一组内重名时取最后一个（清单与 SDK 已保证唯一）。
pub fn compare_tools(old: &[ToolInfo], new: &[ToolInfo]) -> Vec<ToolChanges> {
    let by_name = |tools: &[ToolInfo]| -> BTreeMap<String, ToolInfo> {
        tools.iter().map(|t| (t.name.clone(), t.clone())).collect()
    };
    let (old, new) = (by_name(old), by_name(new));
    old.iter()
        .filter_map(|(name, before)| match new.get(name) {
            None => Some(removed_tool(before)),
            Some(after) => {
                let changes = compare_tool(before, after);
                (!changes.is_empty()).then(|| ToolChanges { tool: name.clone(), removed: false, changes })
            }
        })
        .collect()
}

fn removed_tool(old: &ToolInfo) -> ToolChanges {
    let changes = match old.deprecated {
        Some(_) => Vec::new(),
        None => vec![SchemaChange::new(
            ChangeLevel::Breaking,
            "",
            "删除了未经弃用的工具：应先标 deprecated（指向替代工具）保留一段时间再删除",
        )],
    };
    ToolChanges { tool: old.name.clone(), removed: true, changes }
}

/// 「工具本身」一行：`surface` app → view（Breaking）、生效注解只读 → 非只读、`risk` 升高（Warning）。
fn tool_level_changes(old: &ToolInfo, new: &ToolInfo) -> Vec<SchemaChange> {
    let mut out = Vec::new();
    if old.surface == ToolSurface::App && new.surface == ToolSurface::View {
        out.push(SchemaChange::new(
            ChangeLevel::Breaking,
            "/surface",
            "surface 由 app 变为 view：工具不再能在后台调用与唤醒",
        ));
    }
    let read_only = |t: &ToolInfo| t.effective_annotations().read_only_hint == Some(true);
    if read_only(old) && !read_only(new) {
        out.push(SchemaChange::new(
            ChangeLevel::Warning,
            "/annotations/readOnlyHint",
            "生效注解由只读变为非只读：Agent 的放行 / 确认策略可能改变",
        ));
    }
    if risk_rank(new.risk) > risk_rank(old.risk) {
        out.push(SchemaChange::new(
            ChangeLevel::Warning,
            "/risk",
            format!("risk 升高：{} → {}", risk_name(old.risk), risk_name(new.risk)),
        ));
    }
    out
}

/// `risk` 的高低：read < write < destructive < payment < os-sensitive（与 [`Risk`] 的声明顺序一致）。
fn risk_rank(risk: Risk) -> u8 {
    match risk {
        Risk::Read => 0,
        Risk::Write => 1,
        Risk::Destructive => 2,
        Risk::Payment => 3,
        Risk::OsSensitive => 4,
    }
}

fn risk_name(risk: Risk) -> &'static str {
    match risk {
        Risk::Read => "read",
        Risk::Write => "write",
        Risk::Destructive => "destructive",
        Risk::Payment => "payment",
        Risk::OsSensitive => "os-sensitive",
    }
}

#[cfg(test)]
mod tests;
