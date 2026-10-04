//! 资源：描述、同步、读取、订阅。

use super::*;

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    /// `[a-zA-Z0-9_.-]{1,64}`，如 `cart.state`。
    pub name: String,
    pub description: String,
    /// 缺省为 `application/json`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// 需实时推送：被订阅时 SDK 保持连接（spec/lifecycle.md 第 13 节 B3）。缺省 `false`，只在 `true` 时序列化
    /// （未声明的资源 `toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub realtime: bool,
    /// 资源内容的标注，Hub 原样放到 MCP `resources/list` 的资源注解上。未声明时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
    /// 读取结果缓存声明（spec/protocol.md 3.6）。未声明时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePolicy>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesSyncParams {
    pub resources: Vec<ResourceInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesChangedParams {
    #[serde(default)]
    pub upserted: Vec<ResourceInfo>,
    #[serde(default)]
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUpdatedParams {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesReadParams {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesReadResult {
    /// 资源内容（JSON）。
    pub contents: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSubscribeParams {
    pub name: String,
}
