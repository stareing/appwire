//! App 工具与上游工具的定义转换：Hub API 形式、MCP 形式、`/status` 声明、风险与注解。

#[cfg(feature = "mcp-server")]
use std::sync::Arc;

use app_mcp_protocol::{Activation, Risk};
use rmcp::model::Tool;
use serde_json::Value;
#[cfg(feature = "mcp-server")]
use serde_json::{Map, json};

use crate::mcp_convert;
use crate::tool_def::ToolDef;
use crate::types::{Availability, HubTool, ToolDeclaration};

#[cfg(feature = "mcp-server")]
use crate::schema_evolution;

/// App 工具的 Hub API 形式（解析 schema 文本，只在需要完整定义时构造）。
pub(crate) fn app_hub_tool(app_id: &str, info: &ToolDef, availability: Availability) -> HubTool {
    HubTool {
        annotations: info.effective_annotations(),
        name: format!("{app_id}.{}", info.name),
        app_id: app_id.to_owned(),
        tool: info.name.clone(),
        title: info.title.clone(),
        description: info.description.clone(),
        input_schema: info.input_schema(),
        risk: info.risk,
        activation: info.activation_or_default(),
        availability,
        output_schema: info.output_schema(),
        surface: Some(info.surface),
        page: info.page.clone(),
        implements: info.implements.clone(),
        schema_hash: Some(info.schema_hash().to_owned()),
        deprecated: info.deprecated.clone(),
    }
}

/// App 工具的声明（`/status` 的 `tools`，docs/plans/14-safety.md S5）。
pub(crate) fn tool_declaration(info: &ToolDef) -> ToolDeclaration {
    ToolDeclaration {
        name: info.name.clone(),
        risk: info.risk,
        annotations: info.annotations.clone(),
        effective: info.effective_annotations(),
        output_schema: info.has_output_schema(),
    }
}

/// 上游工具的声明：注解原样（上游没有 `risk`，按注解推导，见 [`upstream_risk`]）。
pub(crate) fn upstream_tool_declaration(t: &Tool) -> ToolDeclaration {
    let annotations = t.annotations.as_ref().map(mcp_convert::from_mcp_tool_annotations);
    ToolDeclaration {
        name: t.name.to_string(),
        risk: upstream_risk(t),
        effective: annotations.clone().unwrap_or_default(),
        annotations,
        output_schema: t.output_schema.is_some(),
    }
}

/// 上游工具的风险：`readOnlyHint` → read；`destructiveHint` → destructive；否则 write。
pub(crate) fn upstream_risk(t: &Tool) -> Risk {
    match &t.annotations {
        Some(a) if a.read_only_hint == Some(true) => Risk::Read,
        Some(a) if a.destructive_hint == Some(true) => Risk::Destructive,
        _ => Risk::Write,
    }
}

/// 上游工具的注解（原样转换，缺省为空）。
pub(crate) fn upstream_annotations(t: &Tool) -> crate::ToolAnnotations {
    t.annotations.as_ref().map(mcp_convert::from_mcp_tool_annotations).unwrap_or_default()
}

pub(crate) fn upstream_hub_tool(name: &str, t: &Tool) -> HubTool {
    HubTool {
        name: format!("{name}.{}", t.name),
        app_id: name.to_owned(),
        tool: t.name.to_string(),
        title: t
            .title
            .clone()
            .or_else(|| t.annotations.as_ref().and_then(|a| a.title.clone())),
        description: t.description.as_deref().unwrap_or_default().to_owned(),
        input_schema: Value::Object((*t.input_schema).clone()),
        risk: upstream_risk(t),
        activation: Activation::Headless,
        availability: Availability::Available,
        annotations: upstream_annotations(t),
        output_schema: t.output_schema.as_ref().map(|s| Value::Object((**s).clone())),
        surface: None,
        page: None,
        implements: Vec::new(),
        schema_hash: None,
        deprecated: None,
    }
}

/// App 工具的 MCP 形式。
#[cfg(feature = "mcp-server")]
pub(crate) fn to_mcp_tool(app_id: &str, info: &ToolDef, availability: Availability) -> Tool {
    let schema = info.input_schema_object().unwrap_or_else(|| {
        let mut m = Map::new();
        m.insert("type".into(), json!("object"));
        m
    });
    let description = schema_evolution::mcp_description(app_id, info, availability);
    let mut tool = Tool::new(format!("{app_id}.{}", info.name), description, schema)
        .with_annotations(mcp_convert::tool_annotations(&info.effective_annotations()))
        .with_meta(schema_evolution::mcp_tool_meta(info));
    if let Some(title) = &info.title {
        tool = tool.with_title(title.clone());
    }
    if let Some(output) = info.output_schema() {
        tool = tool.with_raw_output_schema(Arc::new(mcp_convert::mcp_output_schema(&output)));
    }
    tool
}
