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
    /// 本工具的并发上限（spec/protocol.md 5.3）：缺省 / 0 = 不单独限制。
    pub concurrency: Option<u32>,
    /// 互斥组（spec/protocol.md 5.3）。
    pub exclusive: Option<String>,
    /// 实现的标准意图（spec/intents.md），如 `["message.send@1"]`。
    pub implements: Option<Vec<String>>,
    /// 结果缓存声明（spec/protocol.md 3.6）；`ttlMs` 范围由核心校验。
    pub cache: Option<CachePolicy>,
    /// 弃用声明（spec/protocol.md 3.7）；格式由核心校验。
    pub deprecated: Option<Deprecation>,
    /// 成功结果可能带 `undo`（spec/protocol.md 3.8），只用于展示；缺省 false。
    pub undoable: Option<bool>,
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
            concurrency: f.u32("concurrency"),
            exclusive: f.string("exclusive"),
            implements: f.strings("implements"),
            cache: f.object("cache"),
            deprecated: f.object("deprecated"),
            undoable: f.bool("undoable"),
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
            concurrency: self.concurrency.unwrap_or(0),
            exclusive: self.exclusive,
            implements: self.implements.unwrap_or_default(),
            cache: self.cache,
            deprecated: self.deprecated,
            undoable: self.undoable.unwrap_or(false),
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

/// 结果缓存声明（spec/protocol.md 3.6）：`ttlMs` 必填（非负整数，范围由核心校验），`scope` 缺省 `private`。
///
/// @why 与 [`ToolAnnotations`] 相同，逐字段读取而不用 serde 派生（WASM 体积）。
impl FromJson for CachePolicy {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let ttl_ms = f.u64("ttlMs").unwrap_or_else(|| {
            f.fail("缺少字段 ttlMs".to_owned());
            0
        });
        let scope = f.keyword("scope", "无效的缓存范围（应为 private / shared）", parse_cache_scope);
        f.finish(CachePolicy { ttl_ms, scope: scope.unwrap_or_default() })
    }
}

/// 弃用声明（spec/protocol.md 3.7）：`message` 必填；长度、局部名与日期格式由核心校验。
///
/// @why 与 [`ToolAnnotations`] 相同，逐字段读取而不用 serde 派生（WASM 体积）。
impl FromJson for Deprecation {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let d = Deprecation { message: f.required_string("message"), replacement: f.string("replacement"), until: f.string("until") };
        f.finish(d)
    }
}

fn parse_cache_scope(s: &str) -> Option<CacheScope> {
    match s {
        "private" => Some(CacheScope::Private),
        "shared" => Some(CacheScope::Shared),
        _ => None,
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

/// 部分更新：缺省字段不变；`activation` / `title` / `cache` / `deprecated` 等区分缺省（外层 `None`）与 `null`（`Some(None)`，表示清除）。
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
    pub concurrency: Option<u32>,
    /// `null` 清除互斥组。
    pub exclusive: Option<Option<String>>,
    /// 替换实现的标准意图（空数组 = 清空）。
    pub implements: Option<Vec<String>>,
    /// 整体替换结果缓存声明；`null` 清除。
    pub cache: Option<Option<CachePolicy>>,
    /// 整体替换弃用声明；`null` 清除。
    pub deprecated: Option<Option<Deprecation>>,
    /// `false` 取消声明（`null` 与缺省等同：不变）。
    pub undoable: Option<bool>,
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
        let annotations = f.nullable_object("annotations");
        let page = f.nullable_string("page");
        let background_tool = f.nullable_string("backgroundTool");
        let exclusive = f.nullable_string("exclusive");
        let cache = f.nullable_object("cache");
        let deprecated = f.nullable_object("deprecated");
        let u = JsToolUpdate {
            cache,
            deprecated,
            undoable: f.bool("undoable"),
            concurrency: f.u32("concurrency"),
            exclusive,
            implements: f.strings("implements"),
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
            concurrency: self.concurrency,
            exclusive: self.exclusive,
            implements: self.implements,
            cache: self.cache,
            deprecated: self.deprecated,
            undoable: self.undoable,
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
    pub realtime: bool,
    /// 资源内容的标注（MCP 内容注解）；缺省未声明。
    pub annotations: Option<ContentAnnotations>,
    /// 读取结果缓存声明（spec/protocol.md 3.6）；`ttlMs` 范围由核心校验。
    pub cache: Option<CachePolicy>,
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
            cache: f.object("cache"),
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
            cache: self.cache,
        })
    }
}
