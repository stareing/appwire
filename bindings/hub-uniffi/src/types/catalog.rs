//! 工具与资源：工具列表、过滤条件、资源与 App 总览。

use app_mcp_hub as hub;

use super::{Activation, Availability, ContentAnnotations, Deprecation, Risk, ToolAnnotations, ToolSurface};

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HubTool {
    /// 全名 `<appId>.<tool>`。
    pub name: String,
    pub app_id: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    /// JSON Schema 文本。
    pub input_schema_json: String,
    /// 旧写法；优先看 `annotations`。
    pub risk: Risk,
    pub activation: Activation,
    pub availability: Availability,
    /// Agent 看到的 MCP 工具注解：App 声明的字段原样保留，缺少的按 `risk` 推导；上游工具为其原样注解。
    pub annotations: ToolAnnotations,
    /// App 声明的结果 JSON Schema 文本（原样）；未声明时为空。
    #[uniffi(default = None)]
    pub output_schema_json: Option<String>,
    /// App 工具的界面依赖（未声明即 `App`）；内置与上游工具为空。
    #[uniffi(default = None)]
    pub surface: Option<ToolSurface>,
    /// App 工具所在页面（spec/hub-api.md 3.14）；不属于页面时为空。
    #[uniffi(default = None)]
    pub page: Option<String>,
    /// App 工具实现的标准意图（spec/intents.md，如 `["message.send@1"]`）；未声明时为空。
    #[uniffi(default = [])]
    pub implements: Vec<String>,
    /// App 工具定义的 `schemaHash`（spec/hub-api.md 3.21）：`inputSchema` / `outputSchema` 变化时随之变化；内置与上游工具为空。
    #[uniffi(default = None)]
    pub schema_hash: Option<String>,
    /// App 工具的弃用声明（原样，spec/protocol.md 3.7）；未弃用、内置与上游工具为空。弃用工具照常列出与调用。
    #[uniffi(default = None)]
    pub deprecated: Option<Deprecation>,
}

impl From<hub::HubTool> for HubTool {
    fn from(t: hub::HubTool) -> Self {
        HubTool {
            name: t.name,
            app_id: t.app_id,
            tool: t.tool,
            title: t.title,
            description: t.description,
            input_schema_json: t.input_schema.to_string(),
            risk: t.risk.into(),
            activation: t.activation.into(),
            availability: t.availability.into(),
            annotations: t.annotations.into(),
            output_schema_json: t.output_schema.map(|v| v.to_string()),
            surface: t.surface.map(Into::into),
            page: t.page,
            implements: t.implements,
            schema_hash: t.schema_hash,
            deprecated: t.deprecated.map(Into::into),
        }
    }
}

/// 工具过滤条件。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ToolFilter {
    /// 为空 = 全部 App。
    #[uniffi(default = None)]
    pub apps: Option<Vec<String>>,
    /// 只要风险不高于此等级的工具。
    #[uniffi(default = None)]
    pub max_risk: Option<Risk>,
    /// 只列出当前可调用的工具。
    #[uniffi(default = false)]
    pub only_available: bool,
    /// 是否包含内置工具 `apps.list` / `apps.select` / `apps.overview`（渐进暴露生效时另有 `apps.tools`）。
    #[uniffi(default = true)]
    pub include_builtin: bool,
    /// 厂商会话 ID（`None` = 默认会话）。渐进暴露生效且 `apps` 为空时，只保留该会话已展开 / 调用过 /
    /// 选定了实例的 App 的工具（spec/hub-api.md 3.7）。
    #[uniffi(default = None)]
    pub session: Option<String>,
}

impl Default for ToolFilter {
    fn default() -> Self {
        ToolFilter {
            apps: None,
            max_risk: None,
            only_available: false,
            include_builtin: true,
            session: None,
        }
    }
}

impl From<ToolFilter> for hub::ToolFilter {
    fn from(f: ToolFilter) -> Self {
        hub::ToolFilter {
            apps: f.apps,
            max_risk: f.max_risk.map(Into::into),
            only_available: f.only_available,
            include_builtin: f.include_builtin,
            session: f.session,
        }
    }
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HubResource {
    /// `app-mcp://<appId>/<name>`。
    pub uri: String,
    pub name: String,
    pub app_id: String,
    pub description: String,
    pub mime_type: Option<String>,
    pub available: bool,
    /// 资源内容的标注（MCP 内容注解），原样。
    #[uniffi(default = None)]
    pub annotations: Option<ContentAnnotations>,
}

impl From<hub::HubResource> for HubResource {
    fn from(r: hub::HubResource) -> Self {
        HubResource {
            uri: r.uri,
            name: r.name,
            app_id: r.app_id,
            description: r.description,
            mime_type: r.mime_type,
            available: r.available,
            annotations: r.annotations.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    /// 文本内容（JSON 资源为 JSON 文本）。
    pub text: Option<String>,
    /// 二进制内容（base64）。
    pub blob: Option<String>,
}

impl From<hub::ResourceContent> for ResourceContent {
    fn from(r: hub::ResourceContent) -> Self {
        ResourceContent {
            uri: r.uri,
            mime_type: r.mime_type,
            text: r.text,
            blob: r.blob,
        }
    }
}

/// App 总览（spec/protocol.md 第 7 节）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppOverviewInfo {
    pub app_id: String,
    pub name: String,
    pub summary: String,
    pub body: Option<String>,
    pub locale: Option<String>,
    pub version: String,
    /// `runtime` / `manifest` / `upstream`。
    pub source: String,
    /// 注入给模型的文本。
    pub text: String,
}

impl From<hub::AppOverviewInfo> for AppOverviewInfo {
    fn from(o: hub::AppOverviewInfo) -> Self {
        AppOverviewInfo {
            app_id: o.app_id,
            name: o.name,
            summary: o.summary,
            body: o.body,
            locale: o.locale,
            version: o.version,
            source: o.source,
            text: o.text,
        }
    }
}
