//! 弃用与 `schemaHash` 的呈现（spec/hub-api.md 3.21）：MCP 描述前缀、MCP 工具 `_meta`、参数不符时的提示。

#[cfg(feature = "mcp-server")]
use app_mcp_protocol::Deprecation;
use app_mcp_protocol::{ErrorKind, ToolError};
#[cfg(feature = "mcp-server")]
use rmcp::model::MetaObject;
use serde_json::json;
#[cfg(feature = "mcp-server")]
use serde_json::{Value, to_value};

#[cfg(feature = "mcp-server")]
use crate::call::UNAVAILABLE_PREFIX;
#[cfg(feature = "mcp-server")]
use crate::names::{META_DEPRECATED, META_SCHEMA_HASH, META_UNDOABLE};
use crate::tool_def::ToolDef;
#[cfg(feature = "mcp-server")]
use crate::types::Availability;

/// 弃用工具在 MCP `tools/list` 中的描述前缀（其后为弃用说明）。
pub const DEPRECATED_PREFIX: &str = "[已弃用] ";

/// 参数不符 `inputSchema` 时追加的提示：Agent 手里的定义可能是旧的。
pub(crate) const REFETCH_HINT: &str = "工具定义可能已变化，请重新获取（apps.tools / tools/list）后再调用";

/// 弃用说明：`[已弃用] <message>`，有 `replacement` 时追加 `（改用 <appId>.<replacement>）`。
#[cfg(feature = "mcp-server")]
fn deprecation_text(app_id: &str, d: &Deprecation) -> String {
    let replacement = d.replacement.as_deref().map(|r| format!("（改用 {app_id}.{r}）")).unwrap_or_default();
    format!("{DEPRECATED_PREFIX}{}{replacement}", d.message)
}

/// MCP `tools/list` 的工具描述：`[当前不可用] `（静态工具没有实例注册时）在最前，弃用说明其后（与原描述以空格隔开），再接原描述。
///
/// @why 可用性决定此刻能否调用，比弃用更紧急，放在最前；两者都是前缀，原描述保持完整。
#[cfg(feature = "mcp-server")]
pub(crate) fn mcp_description(app_id: &str, def: &ToolDef, availability: Availability) -> String {
    let unavailable = match availability {
        Availability::NotRegistered => UNAVAILABLE_PREFIX,
        Availability::Available | Availability::Disconnected | Availability::Dormant => "",
    };
    let Some(d) = &def.deprecated else {
        return format!("{unavailable}{}", def.description);
    };
    let deprecation = deprecation_text(app_id, d);
    if def.description.is_empty() {
        return format!("{unavailable}{deprecation}");
    }
    format!("{unavailable}{deprecation} {}", def.description)
}

/// MCP `tools/list` 工具的 `_meta`：`dev.appwire/schemaHash`，弃用时另有 `dev.appwire/deprecated`（原声明），声明了 `undoable` 时
/// 另有 `dev.appwire/undoable: true`（spec/hub-api.md 3.23）。
#[cfg(feature = "mcp-server")]
pub(crate) fn mcp_tool_meta(def: &ToolDef) -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert(META_SCHEMA_HASH.to_owned(), json!(def.schema_hash()));
    if let Some(d) = &def.deprecated {
        meta.insert(META_DEPRECATED.to_owned(), to_value(d).unwrap_or(Value::Null));
    }
    if def.undoable {
        meta.insert(META_UNDOABLE.to_owned(), Value::Bool(true));
    }
    meta
}

/// 参数不符 `inputSchema`（Hub 侧校验）的 `INVALID_INPUT`：消息追加 [`REFETCH_HINT`]，`data` 带校验所用定义的 `schemaHash`。
///
/// @input msg schema 校验的错误说明（[`crate::schema::SchemaCheck::Invalid`]）。
pub(crate) fn invalid_arguments(app_id: &str, tool: &str, def: &ToolDef, msg: &str) -> ToolError {
    ToolError::new(ErrorKind::InvalidInput, format!("参数不符合工具「{app_id}.{tool}」的 inputSchema：{msg}。{REFETCH_HINT}。"))
        .with_details(json!({ "schemaHash": def.schema_hash() }))
}
