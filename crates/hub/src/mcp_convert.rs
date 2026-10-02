//! 协议声明 → MCP 形式的转换（spec/hub-api.md 3.2）：工具注解、内容注解、输出 schema 与结果状态。
//!
//! 只做形式转换，不据声明做任何判断（docs/plans/14-safety.md 第 1 节）。

use app_mcp_protocol::{Audience, ContentAnnotations, ResultStatus, ToolAnnotations};
use rmcp::model::{Annotations, JsonObject, Role};
use serde_json::{Value, json};

/// 非对象 `outputSchema` / 结果包装进 `structuredContent` 时使用的键（MCP 要求 `outputSchema` 根类型为 object）。
pub const WRAPPED_RESULT_KEY: &str = "result";

/// 协议工具注解 → MCP（字段一一对应）。
#[cfg_attr(not(feature = "mcp-server"), allow(dead_code))]
pub(crate) fn tool_annotations(a: &ToolAnnotations) -> rmcp::model::ToolAnnotations {
    rmcp::model::ToolAnnotations::from_raw(
        a.title.clone(),
        a.read_only_hint,
        a.destructive_hint,
        a.idempotent_hint,
        a.open_world_hint,
    )
}

/// MCP 工具注解 → 协议（上游工具）。
pub(crate) fn from_mcp_tool_annotations(a: &rmcp::model::ToolAnnotations) -> ToolAnnotations {
    ToolAnnotations {
        title: a.title.clone(),
        read_only_hint: a.read_only_hint,
        destructive_hint: a.destructive_hint,
        idempotent_hint: a.idempotent_hint,
        open_world_hint: a.open_world_hint,
    }
}

/// 协议内容注解 → MCP（原样；`priority` 不做范围修正）。
pub(crate) fn content_annotations(a: &ContentAnnotations) -> Annotations {
    let mut out = Annotations::default();
    if let Some(roles) = &a.audience {
        out = out.with_audience(
            roles
                .iter()
                .map(|r| match r {
                    Audience::User => Role::User,
                    Audience::Assistant => Role::Assistant,
                })
                .collect(),
        );
    }
    if let Some(p) = a.priority {
        out = out.with_priority(p as f32);
    }
    out.last_modified = a.last_modified.clone();
    out
}

/// MCP 内容注解 → 协议（上游资源）。
pub(crate) fn from_mcp_content_annotations(a: &Annotations) -> ContentAnnotations {
    ContentAnnotations {
        audience: a.audience.as_ref().map(|roles| {
            roles
                .iter()
                .map(|r| match r {
                    Role::User => Audience::User,
                    Role::Assistant => Audience::Assistant,
                })
                .collect()
        }),
        priority: a.priority.map(f64::from),
        last_modified: a.last_modified.clone(),
    }
}

/// App 声明的 `outputSchema` 在 MCP 出口的形式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OutputShape {
    /// 未声明：对象结果放入 `structuredContent`（第 19 项之前的行为）。
    #[default]
    Undeclared,
    /// 根类型为 object：schema 与对象结果原样。
    Object,
    /// 其他根类型：schema 包装为 `{ result: <schema> }`，结果包装为 `{ result: <data> }`。
    Wrapped,
}

impl OutputShape {
    pub fn of(schema: Option<&Value>) -> Self {
        match schema {
            None => OutputShape::Undeclared,
            Some(s) if s.get("type").and_then(Value::as_str) == Some("object") => OutputShape::Object,
            Some(_) => OutputShape::Wrapped,
        }
    }

    /// `structuredContent`：无返回值（`null`）时不填；声明为 object 但结果不是对象时不填（已按校验策略记录）。
    pub fn structured(self, data: &Value) -> Option<Value> {
        match (self, data) {
            (_, Value::Null) => None,
            (OutputShape::Wrapped, v) => Some(json!({ WRAPPED_RESULT_KEY: v })),
            (OutputShape::Undeclared | OutputShape::Object, v) if v.is_object() => Some(v.clone()),
            _ => None,
        }
    }
}

/// MCP `outputSchema`：根类型为 object 时原样，否则包装（见 [`OutputShape::Wrapped`]）。
pub(crate) fn mcp_output_schema(schema: &Value) -> JsonObject {
    let wrapped = match (OutputShape::of(Some(schema)), schema) {
        (OutputShape::Object, Value::Object(m)) => return m.clone(),
        _ => json!({
            "type": "object",
            "properties": { WRAPPED_RESULT_KEY: schema },
            "required": [WRAPPED_RESULT_KEY],
        }),
    };
    match wrapped {
        Value::Object(m) => m,
        _ => JsonObject::new(),
    }
}

/// 无返回值、无摘要、状态为 `done` 时的结果文本（第 19 项 R3）。
pub const DONE_TEXT: &str = "已完成";

/// `status` 不是 `done` 时附在结果最前面的一句说明（第 19 项 R1）。`state_resource` 为资源 URI。
pub(crate) fn status_note(status: ResultStatus, state_resource: Option<&str>) -> Option<String> {
    let note = match status {
        ResultStatus::Done => return None,
        ResultStatus::Pending => {
            let mut s = "[app-mcp] 已受理，尚未完成：操作在等待用户在 App 内确认或异步处理。不要当作已完成，也不要重复提交。"
                .to_owned();
            if let Some(uri) = state_resource {
                s.push_str(&format!("可读取资源 {uri} 查看后续状态。"));
            }
            s
        }
        ResultStatus::Partial => "[app-mcp] 只完成了一部分，未完成的部分没有执行；请根据结果确认后再决定下一步。".to_owned(),
        ResultStatus::Noop => "[app-mcp] 没有做任何改动（目标状态已满足或无事可做）。".to_owned(),
    };
    Some(note)
}

/// 结果状态在 MCP `_meta` 中的键（`status` 不是 `done` 时写入）。
///
/// @compat 键名前缀 `app-mcp/` 暂定；第 19 项 R4 核实 MCP `_meta` 键命名约定（U3）后可能调整。
pub const META_STATUS: &str = "app-mcp/status";
/// `pending` 时可读取后续状态的资源 URI 在 `_meta` 中的键。
pub const META_STATE_RESOURCE: &str = "app-mcp/stateResource";
/// 改调了后台替代时实际调用的工具全名在 `_meta` 中的键（spec/hub-api.md 3.14）。
pub const META_ROUTED_TO: &str = "app-mcp/routedTo";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_annotations_roundtrip() {
        let a = ToolAnnotations {
            title: Some("t".into()),
            read_only_hint: Some(false),
            destructive_hint: Some(true),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
        };
        let m = tool_annotations(&a);
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            json!({"title": "t", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false})
        );
        assert_eq!(from_mcp_tool_annotations(&m), a);
    }

    #[test]
    fn content_annotations_verbatim() {
        let a = ContentAnnotations {
            audience: Some(vec![Audience::User]),
            priority: Some(0.25),
            last_modified: Some("2026-10-02T00:00:00Z".into()),
        };
        assert_eq!(
            serde_json::to_value(content_annotations(&a)).unwrap(),
            json!({"audience": ["user"], "priority": 0.25, "lastModified": "2026-10-02T00:00:00Z"})
        );
        assert_eq!(serde_json::to_value(content_annotations(&ContentAnnotations::default())).unwrap(), json!({}));
        assert_eq!(from_mcp_content_annotations(&content_annotations(&a)), a);
        // 超出范围的 priority 原样转发，不 panic
        let odd = ContentAnnotations { priority: Some(3.0), ..Default::default() };
        assert_eq!(serde_json::to_value(content_annotations(&odd)).unwrap()["priority"], json!(3.0));
    }

    #[test]
    fn output_shapes() {
        assert_eq!(OutputShape::of(None), OutputShape::Undeclared);
        let obj = json!({"type": "object", "properties": {"n": {"type": "number"}}});
        let arr = json!({"type": "array", "items": {"type": "string"}});
        assert_eq!(OutputShape::of(Some(&obj)), OutputShape::Object);
        assert_eq!(OutputShape::of(Some(&arr)), OutputShape::Wrapped);
        assert_eq!(OutputShape::of(Some(&json!({"anyOf": []}))), OutputShape::Wrapped);
        assert_eq!(Value::Object(mcp_output_schema(&obj)), obj);
        assert_eq!(
            Value::Object(mcp_output_schema(&arr)),
            json!({"type": "object", "properties": {"result": arr}, "required": ["result"]})
        );
        // structuredContent
        assert_eq!(OutputShape::Undeclared.structured(&json!({"a": 1})), Some(json!({"a": 1})));
        assert_eq!(OutputShape::Undeclared.structured(&json!([1])), None);
        assert_eq!(OutputShape::Object.structured(&json!(5)), None);
        assert_eq!(OutputShape::Wrapped.structured(&json!(["x"])), Some(json!({"result": ["x"]})));
        assert_eq!(OutputShape::Wrapped.structured(&Value::Null), None, "无返回值不填");
    }

    #[test]
    fn status_notes() {
        assert_eq!(status_note(ResultStatus::Done, None), None);
        let p = status_note(ResultStatus::Pending, Some("app-mcp://shop/order.state")).unwrap();
        assert!(p.contains("尚未完成") && p.contains("app-mcp://shop/order.state"), "{p}");
        assert!(!status_note(ResultStatus::Pending, None).unwrap().contains("app-mcp://"));
        assert!(status_note(ResultStatus::Partial, None).unwrap().contains("一部分"));
        assert!(status_note(ResultStatus::Noop, None).unwrap().contains("没有做任何改动"));
    }
}
