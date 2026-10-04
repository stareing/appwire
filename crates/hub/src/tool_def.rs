//! Hub 保存的工具定义（第 4f 项 d 测量结果）：schema 以 JSON 文本保存、按需解析；同一份定义经 [`SharedTool`]（`Arc`）
//! 在已连接实例、休眠快照、页面目录与各种列表之间共享，列出 / 计数 / 路由都不深拷贝。
//!
//! 与 [`ToolInfo`]（协议类型）字段一一对应；互转只在 [`ToolDef::from_info`] / [`ToolDef::to_info`] 两处
//! （@invariant 两处都用穷举解构 / 构造，`ToolInfo` 增加字段时编译失败，不会静默丢字段）。

use std::sync::Arc;

use app_mcp_manifest::{Manifest, Page};
use app_mcp_protocol::{Activation, CachePolicy, Risk, ToolAnnotations, ToolInfo, ToolSurface};
use serde_json::{Map, Value};

/// 共享的工具定义。
pub type SharedTool = Arc<ToolDef>;

/// 工具定义的紧凑形式：`inputSchema` / `outputSchema` 为紧凑 JSON 文本（解析后的 `serde_json::Value` 约为文本的 6–7 倍）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub risk: Risk,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    pub annotations: Option<ToolAnnotations>,
    pub surface: ToolSurface,
    pub page: Option<String>,
    pub background_tool: Option<String>,
    /// 实现的标准意图（spec/intents.md）；空 = 未声明。
    pub implements: Vec<String>,
    /// 结果缓存声明（spec/protocol.md 3.6）；只在生效注解 `readOnlyHint` 为真时执行（spec/hub-api.md 3.20）。
    pub cache: Option<CachePolicy>,
    /// @invariant 由 `serde_json` 序列化一个 `Value` 得到，总能解析回同一个值。
    input_schema: Box<str>,
    output_schema: Option<Box<str>>,
}

/// 序列化 schema 为紧凑文本。`Value` 序列化不会失败（键都是字符串）；万一失败按 `null` 保存并记错误日志。
fn schema_text(v: &Value) -> Box<str> {
    serde_json::to_string(v)
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "工具 schema 序列化失败，按 null 保存");
            "null".to_owned()
        })
        .into_boxed_str()
}

/// 解析 [`schema_text`] 的结果。
fn parse_schema<T: serde::de::DeserializeOwned + Default>(text: &str) -> T {
    serde_json::from_str(text).unwrap_or_else(|e| {
        tracing::error!(error = %e, "工具 schema 文本无法解析（不应发生），按空值处理");
        T::default()
    })
}

impl ToolDef {
    /// 从协议类型转换（schema 序列化为文本，原 `Value` 随之释放；其余字段移入，不另分配）。
    pub fn from_info(info: ToolInfo) -> Self {
        let ToolInfo {
            name,
            description,
            input_schema,
            risk,
            activation,
            title,
            annotations,
            output_schema,
            surface,
            page,
            background_tool,
            implements,
            cache,
        } = info;
        Self {
            name,
            description,
            risk,
            activation,
            title,
            annotations,
            surface,
            page,
            background_tool,
            implements,
            cache,
            input_schema: schema_text(&input_schema),
            output_schema: output_schema.as_ref().map(schema_text),
        }
    }

    /// 从协议类型复制（全部字段新分配，原值不受影响）；`page` 为 `Some` 时覆盖所在页面。
    pub fn copy_of(info: &ToolInfo, page: Option<&str>) -> Self {
        let ToolInfo {
            name,
            description,
            input_schema,
            risk,
            activation,
            title,
            annotations,
            output_schema,
            surface,
            page: declared_page,
            background_tool,
            implements,
            cache,
        } = info;
        Self {
            name: name.clone(),
            description: description.clone(),
            risk: *risk,
            activation: *activation,
            title: title.clone(),
            annotations: annotations.clone(),
            surface: *surface,
            page: page.map(str::to_owned).or_else(|| declared_page.clone()),
            background_tool: background_tool.clone(),
            implements: implements.clone(),
            cache: *cache,
            input_schema: schema_text(input_schema),
            output_schema: output_schema.as_ref().map(schema_text),
        }
    }

    /// 转回协议类型（解析 schema；用于需要完整定义的少数路径，如 `toolsHash`）。
    pub fn to_info(&self) -> ToolInfo {
        ToolInfo {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema(),
            risk: self.risk,
            activation: self.activation,
            title: self.title.clone(),
            annotations: self.annotations.clone(),
            output_schema: self.output_schema(),
            surface: self.surface,
            page: self.page.clone(),
            background_tool: self.background_tool.clone(),
            implements: self.implements.clone(),
            cache: self.cache,
        }
    }

    /// 同一定义、所在页面改为 `page`（清单 `pages[].tools` 的工具记入其页面）。
    pub fn with_page(&self, page: &str) -> Self {
        Self { page: Some(page.to_owned()), ..self.clone() }
    }

    /// 解析后的 `inputSchema`。
    pub fn input_schema(&self) -> Value {
        parse_schema(&self.input_schema)
    }

    /// `inputSchema` 的 JSON 文本（紧凑形式）。
    pub fn input_schema_json(&self) -> &str {
        &self.input_schema
    }

    /// `inputSchema` 解析为 JSON 对象；根不是对象时为 `None`（注册时已过滤，正常不出现）。
    pub fn input_schema_object(&self) -> Option<Map<String, Value>> {
        serde_json::from_str(&self.input_schema).ok()
    }

    /// 解析后的 `outputSchema`。
    pub fn output_schema(&self) -> Option<Value> {
        self.output_schema.as_deref().map(parse_schema)
    }

    pub fn has_output_schema(&self) -> bool {
        self.output_schema.is_some()
    }

    /// Agent 看到的注解（[`ToolInfo::effective_annotations`]）。
    pub fn effective_annotations(&self) -> ToolAnnotations {
        ToolAnnotations::effective(self.risk, self.annotations.as_ref())
    }

    /// 生效的结果缓存声明（spec/hub-api.md 3.20）：只在生效注解 `readOnlyHint` 为真时返回 `cache`；写工具上的声明被忽略。
    pub fn effective_cache(&self) -> Option<CachePolicy> {
        self.cache.filter(|_| self.is_read_only())
    }

    /// 生效注解 `readOnlyHint` 是否为真（缓存的存入条件与写调用失效的判据）。
    pub fn is_read_only(&self) -> bool {
        self.effective_annotations().read_only_hint == Some(true)
    }

    /// 同 [`ToolInfo`]：`activation` 未声明时取缺省。
    pub fn activation_or_default(&self) -> Activation {
        self.activation.unwrap_or_default()
    }
}

impl From<ToolInfo> for ToolDef {
    fn from(info: ToolInfo) -> Self {
        Self::from_info(info)
    }
}

/// 注册表保存的静态清单：工具（顶层与 `pages[].tools`）转为共享的紧凑定义，其余字段原样。
///
/// @invariant [`StaticManifest::meta`] 的 `tools` 与各页面的 `tools` 为空——工具只在本结构中保存一份，
/// 经 [`StaticManifest::tools`] / [`StaticManifest::tool`] / [`StaticManifest::pages`] 读取。
#[derive(Debug)]
pub struct StaticManifest {
    meta: Manifest,
    tools: Vec<SharedTool>,
    /// 与 `meta.pages` 一一对应；工具的 `page` 已设为所在页面名。
    page_tools: Vec<Vec<SharedTool>>,
}

impl StaticManifest {
    pub fn new(manifest: Manifest) -> Self {
        Self::copy_of(&manifest)
    }

    /// 复制为紧凑形式（全部新分配，不引用 `manifest` 的任何内存）。
    ///
    /// @why 批量登记时先全部复制、再一起释放解析形式，新分配集中而解析形式留下成片的空闲内存，可以归还操作系统
    /// （逐个移入时紧凑数据散落在解析形式的空隙中，整页无法归还；第 4f 项 d 测量）。
    pub fn copy_of(manifest: &Manifest) -> Self {
        let Manifest {
            manifest_version,
            app_id,
            name,
            version,
            description,
            overview,
            launch,
            wake,
            tools,
            resources,
            pages,
            events,
        } = manifest;
        let page_meta = |p: &Page| Page {
            name: p.name.clone(),
            title: p.title.clone(),
            description: p.description.clone(),
            route: p.route.clone(),
            params: p.params.clone(),
            tools: Vec::new(),
            navigable: p.navigable,
            activation: p.activation,
        };
        Self {
            meta: Manifest {
                manifest_version: *manifest_version,
                app_id: app_id.clone(),
                name: name.clone(),
                version: version.clone(),
                description: description.clone(),
                overview: overview.clone(),
                launch: launch.clone(),
                wake: wake.clone(),
                tools: Vec::new(),
                resources: resources.clone(),
                pages: pages.iter().map(page_meta).collect(),
                events: events.clone(),
            },
            tools: tools.iter().map(|t| Arc::new(ToolDef::copy_of(t, None))).collect(),
            page_tools: pages
                .iter()
                .map(|p| p.tools.iter().map(|t| Arc::new(ToolDef::copy_of(t, Some(&p.name)))).collect())
                .collect(),
        }
    }

    /// 清单的非工具部分（名称、总览、启动 / 唤醒描述、页面说明等）；`tools` 与 `pages[].tools` 为空。
    pub fn meta(&self) -> &Manifest {
        &self.meta
    }

    /// 顶层静态工具。
    pub fn tools(&self) -> &[SharedTool] {
        &self.tools
    }

    pub fn tool(&self, name: &str) -> Option<&SharedTool> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// 页面与其工具（工具的 `page` 为该页面名）。
    pub fn pages(&self) -> impl Iterator<Item = (&Page, &[SharedTool])> {
        self.meta.pages.iter().zip(self.page_tools.iter().map(Vec::as_slice))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::ToolSurface;
    use serde_json::json;

    fn full() -> ToolInfo {
        ToolInfo {
            name: "orders.search".into(),
            description: "查订单".into(),
            input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
            risk: Risk::Destructive,
            activation: Some(Activation::Background),
            title: Some("搜索".into()),
            annotations: Some(ToolAnnotations { read_only_hint: Some(true), ..Default::default() }),
            output_schema: Some(json!({"type": "array"})),
            surface: ToolSurface::View,
            page: Some("orders".into()),
            background_tool: Some("orders.searchBg".into()),
            implements: vec!["message.send@1".into()],
            cache: Some(CachePolicy { ttl_ms: 5000, scope: app_mcp_protocol::CacheScope::Shared }),
        }
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let info = full();
        let def = ToolDef::from_info(info.clone());
        assert_eq!(def.to_info(), info);
        assert_eq!(ToolDef::copy_of(&info, None), def);
        assert_eq!(ToolDef::copy_of(&info, Some("p")).page.as_deref(), Some("p"));
        assert_eq!(def.input_schema_json(), r#"{"properties":{"q":{"type":"string"}},"required":["q"],"type":"object"}"#);
        assert_eq!(def.input_schema_object().unwrap()["type"], "object");
        assert!(def.has_output_schema());
        assert_eq!(def.effective_annotations(), info.effective_annotations());
    }

    #[test]
    fn minimal_tool_and_page_override() {
        let info = ToolInfo {
            name: "a".into(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            risk: Risk::default(),
            activation: None,
            title: None,
            annotations: None,
            output_schema: None,
            surface: ToolSurface::App,
            page: None,
            background_tool: None,
            implements: Vec::new(),
            cache: None,
        };
        let def = ToolDef::from(info.clone());
        assert_eq!(def.to_info(), info);
        assert!(!def.has_output_schema());
        assert_eq!(def.output_schema(), None);
        assert_eq!(def.with_page("p").page.as_deref(), Some("p"));
        assert_eq!(def.activation_or_default(), Activation::default());
    }

    #[test]
    fn static_manifest_keeps_tools_once() {
        let m: Manifest = serde_json::from_value(json!({
            "manifestVersion": 1, "appId": "shop", "name": "Shop",
            "tools": [{"name": "cart.add", "description": "加", "inputSchema": {"type": "object"}}],
            "pages": [{"name": "orders", "tools": [{"name": "orders.open", "description": "开", "inputSchema": {"type": "object"}}]}]
        }))
        .unwrap();
        let s = StaticManifest::new(m);
        assert!(s.meta().tools.is_empty());
        assert!(s.meta().pages.iter().all(|p| p.tools.is_empty()));
        assert_eq!(s.meta().name, "Shop");
        assert_eq!(s.tools().len(), 1);
        assert!(s.tool("cart.add").is_some() && s.tool("orders.open").is_none());
        let pages: Vec<(String, Vec<Option<String>>)> =
            s.pages().map(|(p, t)| (p.name.clone(), t.iter().map(|t| t.page.clone()).collect())).collect();
        assert_eq!(pages, vec![("orders".to_owned(), vec![Some("orders".to_owned())])]);
    }
}
