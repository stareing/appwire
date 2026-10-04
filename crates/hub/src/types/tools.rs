//! 工具与资源：可用性、工具定义与过滤、暴露方式、协议版本模式、风险顺序、资源与总览。

use app_mcp_protocol::{Activation, ContentAnnotations, Risk, ToolAnnotations, ToolSurface};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具当前是否可调用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    /// 至少一个已连接实例注册了该工具。
    Available,
    /// App 未连接，工具来自静态清单。
    Disconnected,
    /// App 已连接，但没有实例注册该静态工具。
    NotRegistered,
    /// 只有休眠实例注册了该工具（工具来自休眠前的快照）；调用时 Hub 先唤醒实例再派发（spec/lifecycle.md §9）。
    Dormant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubTool {
    /// 全名 `<appId>.<tool>`，与 MCP 出口一致。
    pub name: String,
    pub app_id: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub risk: Risk,
    pub activation: Activation,
    pub availability: Availability,
    /// Agent 看到的 MCP 工具注解：App 声明的字段原样保留，缺少的按 `risk` 推导（spec/protocol.md 第 3 节）；
    /// 上游工具为其原样注解。
    #[serde(default)]
    pub annotations: ToolAnnotations,
    /// App 声明的结果 JSON Schema（原样；MCP 出口按需包装，spec/hub-api.md 3.2）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// App 工具声明的界面依赖（spec/protocol.md 3.4；未声明即 `app`）；内置与上游工具为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<ToolSurface>,
    /// App 工具所在页面（声明的 `page`，或页面目录中的页面，spec/hub-api.md 3.14）；不属于页面时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// App 工具声明实现的标准意图（spec/intents.md，如 `message.send@1`）；未声明、内置与上游工具为空（空时不序列化）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<String>,
    /// App 工具定义的 `schemaHash`（spec/hub-api.md 3.21）：`inputSchema` / `outputSchema` 变化时随之变化；内置与上游工具为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_hash: Option<String>,
    /// App 工具的弃用声明（原样，spec/protocol.md 3.7）；未弃用、内置与上游工具为 `None`。弃用工具照常列出与调用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<app_mcp_protocol::Deprecation>,
    /// 工具声明了 `undoable`（spec/protocol.md 3.8，只用于展示）；内置 / 上游工具为 `false`。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undoable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolFilter {
    /// `None` = 全部 App。
    pub apps: Option<Vec<String>>,
    /// 只要风险不高于此等级的工具（等级顺序见 [`risk_rank`]）。
    pub max_risk: Option<Risk>,
    /// 只列出 [`Availability::Available`] 的工具。默认 `false`。
    pub only_available: bool,
    /// 是否包含内置工具 `apps.list` / `apps.select` / `apps.overview`（渐进暴露生效时另有 `apps.tools`）。默认 `true`。
    pub include_builtin: bool,
    /// 厂商会话 ID（与 [`CallRequest::session`](super::CallRequest::session) 相同；`None` = 默认会话）。渐进暴露生效且 `apps` 为 `None` 时，
    /// 只保留该会话已展开 / 选定的 App 的工具（spec/hub-api.md 3.7）。
    pub session: Option<String>,
}

impl Default for ToolFilter {
    fn default() -> Self {
        Self {
            apps: None,
            max_risk: None,
            only_available: false,
            include_builtin: true,
            session: None,
        }
    }
}

impl ToolFilter {
    pub(crate) fn accepts(&self, tool: &HubTool, builtin: bool) -> bool {
        if builtin {
            return self.include_builtin;
        }
        if let Some(apps) = &self.apps
            && !apps.iter().any(|a| a == &tool.app_id)
        {
            return false;
        }
        if let Some(max) = self.max_risk
            && risk_rank(tool.risk) > risk_rank(max)
        {
            return false;
        }
        !self.only_available || tool.availability == Availability::Available
    }
}

/// 工具暴露方式（spec/hub-api.md 3.7）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolExposure {
    /// 列出全部工具（旧行为）。
    All,
    /// 渐进暴露：工具列表只含 `apps.*` 内置工具，以及本会话展开过（`apps.tools`）、调用过或选定了实例的 App 的工具。
    Progressive,
    /// 默认：App 与上游工具总数超过 [`crate::HubConfig::tool_exposure_threshold`] 时按 `Progressive`，否则按 `All`。
    #[default]
    Auto,
}

/// MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」，docs/plans/12-mcp-stateless.md S7）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpProtocolMode {
    /// 默认：双版本——`initialize` 客户端走 legacy 会话（至多 2025-11-25），每请求自带 `_meta` 的客户端可协商 2026-07-28
    /// （无会话语义、`subscriptions/listen`）。
    #[default]
    Auto,
    /// 回退开关：只声明到 2025-11-25（S7 之前的行为）。声明 2026-07-28 的请求得 `-32022 UnsupportedProtocolVersion`，
    /// 能回退的客户端改用 `initialize`；`subscriptions/listen` 不可用。
    LegacyOnly,
}

/// 风险等级的顺序：read < write < destructive < payment < os-sensitive（与协议中的列举顺序一致）。
pub fn risk_rank(r: Risk) -> u8 {
    match r {
        Risk::Read => 0,
        Risk::Write => 1,
        Risk::Destructive => 2,
        Risk::Payment => 3,
        Risk::OsSensitive => 4,
    }
}

/// 风险的字符串形式（`read` / `write` / `destructive` / `payment` / `os-sensitive`）。
pub fn risk_str(r: Risk) -> &'static str {
    match r {
        Risk::Read => "read",
        Risk::Write => "write",
        Risk::Destructive => "destructive",
        Risk::Payment => "payment",
        Risk::OsSensitive => "os-sensitive",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubResource {
    /// `app-mcp://<appId>/<name>`（上游为 `app-mcp://<name>/<编码后的上游 URI>`）。
    pub uri: String,
    /// `<appId>.<name>`。
    pub name: String,
    pub app_id: String,
    pub description: String,
    pub mime_type: Option<String>,
    /// App 未连接（来自静态清单）时为 `false`。
    pub available: bool,
    /// 资源内容的标注（MCP 内容注解），原样。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
}

/// 资源内容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    /// 文本内容（JSON 资源为 JSON 文本）。
    pub text: Option<String>,
    /// 二进制内容（base64，仅上游 MCP 服务器可能返回）。
    pub blob: Option<String>,
}

/// App 总览（spec/protocol.md 第 7 节），已截断并计算版本。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppOverviewInfo {
    pub app_id: String,
    pub name: String,
    pub summary: String,
    pub body: Option<String>,
    pub locale: Option<String>,
    /// 内容哈希（12 位十六进制）。
    pub version: String,
    /// `runtime` / `manifest` / `upstream`。
    pub source: String,
    /// 注入给模型的文本（7.3 节格式，含 `<app-overview>` 包裹）。
    pub text: String,
}
