//! 工具 / 资源定义与部分更新，以及 MCP 工具注解、内容注解。

use super::*;

// ---------------------------------------------------------------------------
// 定义
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct JsToolDef {
    pub name: String,
    /// 缺省为空字符串。
    pub description: String,
    pub input_schema: Value,
    pub risk: Option<Risk>,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    pub annotations: Option<ToolAnnotations>,
    pub output_schema: Option<Value>,
    /// `"app"`（缺省）/ `"view"`（spec/protocol.md 3.4）。
    pub surface: Option<ToolSurface>,
    pub page: Option<String>,
    /// 后台替代：同一 App 中一个 `app` 工具的局部名（spec/protocol.md 3.4）。
    pub background_tool: Option<String>,
    /// 缺省 true。
    pub enabled: Option<bool>,
    pub scope: Option<f64>,
}

impl FromJson for JsToolDef {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let name = f.required_string("name");
        let description = f.string("description").unwrap_or_default();
        let input_schema = f.value("inputSchema").unwrap_or_else(|| {
            f.fail("缺少字段 inputSchema".to_owned());
            Value::Null
        });
        let d = JsToolDef {
            name,
            description,
            input_schema,
            risk: f.protocol("risk"),
            activation: f.protocol("activation"),
            title: f.string("title"),
            annotations: f.object("annotations"),
            output_schema: f.value("outputSchema"),
            surface: f.protocol("surface"),
            page: f.string("page"),
            background_tool: f.string("backgroundTool"),
            enabled: f.bool("enabled"),
            scope: f.f64("scope"),
        };
        f.finish(d)
    }
}

impl JsToolDef {
    pub fn into_core(self) -> Result<ToolDef, String> {
        Ok(ToolDef {
            name: self.name,
            description: self.description,
            input_schema: self.input_schema,
            risk: self.risk.unwrap_or_default(),
            activation: self.activation,
            title: self.title,
            enabled: self.enabled.unwrap_or(true),
            scope: scope_handle(self.scope)?,
            annotations: self.annotations,
            output_schema: self.output_schema,
            surface: self.surface.unwrap_or_default(),
            page: self.page,
            background_tool: self.background_tool,
        })
    }
}

/// 标准 MCP 工具注解（spec/protocol.md 第 3 节）。
///
/// @why 用 [`Fields`] 逐字段读取而不是 serde 派生：派生的反序列化代码让 WASM gzip 增加约 1.7 KB。
impl FromJson for ToolAnnotations {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let a = ToolAnnotations {
            title: f.string("title"),
            read_only_hint: f.bool("readOnlyHint"),
            destructive_hint: f.bool("destructiveHint"),
            idempotent_hint: f.bool("idempotentHint"),
            open_world_hint: f.bool("openWorldHint"),
        };
        f.finish(a)
    }
}

pub(super) fn parse_result_status(s: &str) -> Option<ResultStatus> {
    Some(match s {
        "done" => ResultStatus::Done,
        "pending" => ResultStatus::Pending,
        "partial" => ResultStatus::Partial,
        "noop" => ResultStatus::Noop,
        _ => return None,
    })
}

/// 标准 MCP 内容注解（结果的 `annotations`）。
impl FromJson for ContentAnnotations {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let audience = f.strings("audience").map(|roles| {
            roles
                .iter()
                .filter_map(|r| match r.as_str() {
                    "user" => Some(Audience::User),
                    "assistant" => Some(Audience::Assistant),
                    other => {
                        f.fail(format!("字段 audience 的取值应为 user / assistant：\"{other}\""));
                        None
                    }
                })
                .collect()
        });
        let a = ContentAnnotations {
            audience,
            priority: f.f64("priority"),
            last_modified: f.string("lastModified"),
        };
        f.finish(a)
    }
}

/// 部分更新：缺省字段不变；`activation` / `title` 区分缺省（外层 `None`）与 `null`（`Some(None)`，表示清除）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsToolUpdate {
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub risk: Option<Risk>,
    pub activation: Option<Option<Activation>>,
    pub title: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// `null` 清除声明的注解。
    pub annotations: Option<Option<ToolAnnotations>>,
    /// `null` 清除声明的输出 schema。
    pub output_schema: Option<Option<Value>>,
    pub surface: Option<ToolSurface>,
    /// `null` 清除声明的页面。
    pub page: Option<Option<String>>,
    /// `null` 清除声明的后台替代。
    pub background_tool: Option<Option<String>>,
}

impl FromJson for JsToolUpdate {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let activation = match f.nullable("activation") {
            None => None,
            Some(None) => Some(None),
            Some(Some(v)) => f.protocol_value("activation", v).map(Some),
        };
        let title = f.nullable_string("title");
        let annotations = match f.nullable("annotations") {
            None => None,
            Some(None) => Some(None),
            Some(Some(v)) => match ToolAnnotations::from_json(v) {
                Ok(a) => Some(Some(a)),
                Err(e) => {
                    f.fail(format!("annotations.{e}"));
                    None
                }
            },
        };
        let page = f.nullable_string("page");
        let background_tool = f.nullable_string("backgroundTool");
        let u = JsToolUpdate {
            surface: f.protocol("surface"),
            page,
            background_tool,
            annotations,
            output_schema: f.nullable("outputSchema"),
            description: f.string("description"),
            input_schema: f.value("inputSchema"),
            risk: f.protocol("risk"),
            activation,
            title,
            enabled: f.bool("enabled"),
        };
        f.finish(u)
    }
}

impl JsToolUpdate {
    pub fn into_core(self) -> ToolUpdate {
        ToolUpdate {
            description: self.description,
            input_schema: self.input_schema,
            risk: self.risk,
            activation: self.activation,
            title: self.title,
            enabled: self.enabled,
            annotations: self.annotations,
            output_schema: self.output_schema,
            surface: self.surface,
            page: self.page,
            background_tool: self.background_tool,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JsResourceDef {
    pub name: String,
    /// 缺省为空字符串。
    pub description: String,
    pub mime_type: Option<String>,
    pub scope: Option<f64>,
    /// 缺省 `false`（spec/lifecycle.md 第 13 节 B3）。
    pub realtime: bool,    /// 资源内容的标注（MCP 内容注解）；缺省未声明。
    pub annotations: Option<ContentAnnotations>,
}

impl FromJson for JsResourceDef {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let d = JsResourceDef {
            name: f.required_string("name"),
            description: f.string("description").unwrap_or_default(),
            mime_type: f.string("mimeType"),
            scope: f.f64("scope"),
            realtime: f.bool("realtime").unwrap_or(false),
            annotations: f.object("annotations"),
        };
        f.finish(d)
    }
}

impl JsResourceDef {
    pub fn into_core(self) -> Result<ResourceDef, String> {
        Ok(ResourceDef {
            name: self.name,
            description: self.description,
            mime_type: self.mime_type,
            scope: scope_handle(self.scope)?,
            realtime: self.realtime,
            annotations: self.annotations,
        })
    }
}
