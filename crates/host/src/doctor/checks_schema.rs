//! 工具定义的不兼容变化（第 16 项 O4，spec/hub-api.md 3.21）：运行中 Host 记下的 `schemaChanges`。

use app_mcp_protocol::schema_compat::ChangeLevel;
use app_mcp_hub::{HubStatus, SchemaChangeRecord};
use serde_json::json;

use super::{Check, Level};

const ID: &str = "schema-changes";
const TITLE: &str = "工具定义变化";

/// 一条记录的一句描述：`shop.send（breaking）：/inputSchema/required 新增必填参数 to`（多条变化只列第一条并注明总数）。
fn describe(r: &SchemaChangeRecord) -> String {
    let first = r.changes.first().map(|c| format!("{} {}", c.path, c.message)).unwrap_or_default();
    let more = if r.changes.len() > 1 { format!(" 等 {} 处", r.changes.len()) } else { String::new() };
    format!("{}.{}（{}）：{first}{more}", r.app_id, r.tool, r.level.as_str())
}

/// 有破坏性变化为注意（Agent 按旧定义的调用会出错，App 应改用新工具名）；只有可能破坏的变化为信息；没有为通过；
/// 旧 Host 不报告时跳过。摘要列最近的记录（新的在前）。
pub(super) fn schema_changes_check(status: Option<&Result<HubStatus, String>>) -> Check {
    let Some(Ok(st)) = status else {
        return Check::new(ID, TITLE, Level::Skip, "Host 未运行或状态不可读");
    };
    let Some(records) = &st.schema_changes else {
        return Check::new(ID, TITLE, Level::Skip, "运行中的 Host 版本不报告工具定义变化");
    };
    let details = json!({ "schemaChanges": records });
    if records.is_empty() {
        return Check::new(ID, TITLE, Level::Ok, "启动以来没有不兼容的工具定义变化").details(details);
    }
    let summary = records.iter().rev().map(describe).collect::<Vec<_>>().join("；");
    if records.iter().any(|r| r.level == ChangeLevel::Breaking) {
        return Check::new(ID, TITLE, Level::Warn, summary)
            .hint("同名工具出现破坏性变化：Agent 按旧定义的调用会出错。不兼容的变更应使用新工具名，旧工具标 deprecated 并以 replacement 指向新工具（spec/protocol.md 3.7）")
            .details(details);
    }
    Check::new(ID, TITLE, Level::Info, summary).details(details)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(changes: serde_json::Value) -> HubStatus {
        let mut st: HubStatus = serde_json::from_value(json!({
            "service": "app-mcp", "version": "t", "user": "u", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
            "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 0, "apps": [], "reports": []
        }))
        .unwrap();
        st.schema_changes = serde_json::from_value(changes).unwrap();
        st
    }

    fn record(tool: &str, level: &str, n: usize) -> serde_json::Value {
        let changes: Vec<_> =
            (0..n).map(|i| json!({"level": level, "path": format!("/inputSchema/p{i}"), "message": "变了"})).collect();
        json!({"appId": "shop", "tool": tool, "level": level, "changes": changes, "at": 1})
    }

    #[test]
    fn levels_and_summary() {
        assert!(matches!(schema_changes_check(None).status, Level::Skip));
        assert!(matches!(schema_changes_check(Some(&Ok(status(json!(null))))).status, Level::Skip), "旧 Host 不报告");
        assert!(matches!(schema_changes_check(Some(&Ok(status(json!([]))))).status, Level::Ok));

        let warn_only = status(json!([record("a", "warning", 1)]));
        let c = schema_changes_check(Some(&Ok(warn_only)));
        assert!(matches!(c.status, Level::Info), "{c:?}");
        assert_eq!(c.summary, "shop.a（warning）：/inputSchema/p0 变了");

        let mixed = status(json!([record("a", "warning", 1), record("b", "breaking", 3)]));
        let c = schema_changes_check(Some(&Ok(mixed)));
        assert!(matches!(c.status, Level::Warn) && c.hint.is_some(), "{c:?}");
        assert_eq!(c.summary, "shop.b（breaking）：/inputSchema/p0 变了 等 3 处；shop.a（warning）：/inputSchema/p0 变了", "新的在前");
        assert_eq!(c.details["schemaChanges"][1]["tool"], "b");
    }
}
