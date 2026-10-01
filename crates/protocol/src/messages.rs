//! 方法名与参数 / 结果类型。所有字段在 JSON 中为 camelCase。
//!
//! 方向约定：SDK 指 App 内的客户端核心，Host 指本地 MCP Host。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 方法名常量。
pub mod method {
    // ---- SDK → Host：请求 ----
    /// 握手与配对。参数 [`super::HelloParams`]，结果 [`super::HelloResult`]。
    pub const HELLO: &str = "app/hello";

    // ---- SDK → Host：通知 ----
    /// 唤醒 / 连接后初始化完成，动态工具已同步。参数 [`super::ReadyParams`]。
    pub const READY: &str = "app/ready";
    /// 实例可见性与焦点变化。参数 [`super::VisibilityParams`]。
    pub const VISIBILITY: &str = "app/visibility";
    /// 全量同步工具列表（握手成功后、重连后）。参数 [`super::ToolsSyncParams`]。
    pub const TOOLS_SYNC: &str = "tools/sync";
    /// 增量变更。参数 [`super::ToolsChangedParams`]。
    pub const TOOLS_CHANGED: &str = "tools/changed";
    /// 全量同步资源列表。参数 [`super::ResourcesSyncParams`]。
    pub const RESOURCES_SYNC: &str = "resources/sync";
    /// 资源列表增量变更。参数 [`super::ResourcesChangedParams`]。
    pub const RESOURCES_CHANGED: &str = "resources/changed";
    /// 已订阅资源的内容变化。参数 [`super::ResourceUpdatedParams`]。
    pub const RESOURCES_UPDATED: &str = "resources/updated";

    // ---- SDK → Host：请求（生命周期，spec/lifecycle.md）----
    /// 请求休眠。参数 [`super::SleepParams`]，结果 [`super::SleepResult`]。
    pub const SLEEP: &str = "app/sleep";

    // ---- Host → SDK：请求 ----
    /// 调用工具。参数 [`super::ToolsInvokeParams`]，结果 [`super::ToolsInvokeResult`]。
    pub const TOOLS_INVOKE: &str = "tools/invoke";
    /// 读取资源。参数 [`super::ResourcesReadParams`]，结果 [`super::ResourcesReadResult`]。
    pub const RESOURCES_READ: &str = "resources/read";
    /// 订阅资源变化。参数 [`super::ResourceSubscribeParams`]，结果 `{}`。
    pub const RESOURCES_SUBSCRIBE: &str = "resources/subscribe";
    /// 取消订阅。参数 [`super::ResourceSubscribeParams`]，结果 `{}`。
    pub const RESOURCES_UNSUBSCRIBE: &str = "resources/unsubscribe";
    /// 请求实例切换前台 / 后台。参数 [`super::ActivateParams`]，结果 `{}`。
    pub const ACTIVATE: &str = "app/activate";

    // ---- Host → SDK：通知 ----
    /// 取消进行中的调用。参数 [`super::ToolsCancelParams`]。
    pub const TOOLS_CANCEL: &str = "tools/cancel";
    /// 配对结果（握手返回 `pending` 之后）。参数 [`super::PairingResultParams`]。
    pub const PAIRING_RESULT: &str = "app/pairingResult";
    /// 租约：Host 预计还会调用本实例，ttl 内不要休眠。参数 [`super::LeaseParams`]。
    pub const LEASE: &str = "app/lease";

    // ---- SDK → Host：连接级请求（多路复用，spec/protocol.md 第 9 节）----
    /// 把本连接切换为多路复用模式，只能作为连接上的第一条消息。参数 [`super::MuxParams`]，结果 [`super::MuxResult`]。
    pub const MUX: &str = "app/mux";

    // ---- 双向：请求 ----
    /// 心跳。参数为空，结果 `{}`。
    pub const PING: &str = "ping";
}

/// 客户端类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    /// 浏览器中的网页。
    Web,
    /// 原生 App。
    Native,
    /// 混合应用（Electron、Tauri、WebView）。
    Hybrid,
}

/// 风险等级。决定 Host 的确认策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Read,
    #[default]
    Write,
    Destructive,
    Payment,
    OsSensitive,
}

/// 调用时 App 需要的激活方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activation {
    Headless,
    Background,
    #[default]
    Foreground,
}

/// 实例可见性。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
    Frozen,
}

// ---------------------------------------------------------------------------
// 握手与配对
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloParams {
    /// App 标识，`[a-z][a-z0-9-]{0,62}`。
    pub app_id: String,
    pub app_name: String,
    pub protocol_version: String,
    pub sdk_version: String,
    pub client_kind: ClientKind,
    /// 实例 ID：每个标签页或进程唯一，页面刷新后保持不变。
    pub instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// 网页来源，如 `http://localhost:5173`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 实例标题（网页为 `document.title`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_title: Option<String>,
    /// 实例地址（网页为 `location.href`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_url: Option<String>,
    /// 之前配对得到的 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// 由 Host 唤醒时携带的一次性 token。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_token: Option<String>,
    /// App 总览。优先于静态清单中的总览（见 spec/protocol.md 第 7 节）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<AppOverview>,
    /// 上次 `app/sleep` 被接受时 Host 返回的恢复令牌（spec/lifecycle.md 4.3）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_token: Option<String>,
    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节），与 `resume_token` 一起发送。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_hash: Option<String>,
    /// 本次连接的原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_reason: Option<WakeReason>,
}

impl Default for HelloParams {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_name: String::new(),
            protocol_version: crate::PROTOCOL_VERSION.to_owned(),
            sdk_version: String::new(),
            client_kind: ClientKind::Native,
            instance_id: String::new(),
            app_version: None,
            origin: None,
            instance_title: None,
            instance_url: None,
            token: None,
            launch_token: None,
            overview: None,
            resume_token: None,
            tools_hash: None,
            wake_reason: None,
        }
    }
}

/// 一句话简介的长度上限（字符数），超出部分由 Host 截断。
pub const OVERVIEW_SUMMARY_MAX_CHARS: usize = 100;
/// 总览正文的长度上限（字符数），超出部分由 Host 截断。
pub const OVERVIEW_BODY_MAX_CHARS: usize = 2000;

/// App 总览：模型在一个会话中首次接触该 App 时，由 Host 附带给模型（spec/protocol.md 第 7 节）。
///
/// 总览只描述能力，不授权：实际可调用的范围只看已注册的工具和 Host 的权限策略。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppOverview {
    /// 一句话简介（≤ 100 字符），出现在 MCP `instructions` 与 `apps.list` 中。
    pub summary: String,
    /// 总览正文（Markdown，≤ 2000 字符）。建议小节：适用场景、能力范围、典型流程、
    /// 前置条件、不支持的操作、风险说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// 语言，如 `zh-CN`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairingStatus {
    /// 已配对，可以同步工具。
    Paired,
    /// 等待用户在 Host 端确认，结果通过 `app/pairingResult` 通知。
    Pending,
    /// 被拒绝，SDK 不应自动重连。
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    pub status: PairingStatus,
    /// `paired` 时返回，SDK 应持久化并在下次握手时携带。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub protocol_version: String,
    pub host_version: String,
    /// `rejected` 时的原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 为 true 时 Host 沿用休眠前的工具快照，SDK 跳过 `tools/sync` / `resources/sync`
    /// （spec/lifecycle.md 4.3）。缺省 false。
    #[serde(default, skip_serializing_if = "is_false")]
    pub tools_current: bool,
}

/// `Default` 仅为方便构造（配合 `..Default::default()`）：`status` 缺省为 `rejected`，
/// `protocol_version` 为当前协议版本。
impl Default for HelloResult {
    fn default() -> Self {
        Self {
            status: PairingStatus::Rejected,
            token: None,
            protocol_version: crate::PROTOCOL_VERSION.to_owned(),
            host_version: String::new(),
            reason: None,
            tools_current: false,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingResultParams {
    /// `paired` 或 `rejected`。
    pub status: PairingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyParams {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibilityParams {
    pub visibility: Visibility,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateParams {
    pub mode: Activation,
}

// ---------------------------------------------------------------------------
// 生命周期（spec/lifecycle.md）
// ---------------------------------------------------------------------------

/// 连接（回连）原因，随 `app/hello` 发送。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeReason {
    /// 由 Host 通过操作系统激活机制唤醒（携带 `launchToken`）。
    OsActivation,
    /// App 主动回连（`wake()` / `connectNow()`）。
    App,
    /// 页面 / 窗口重新可见。
    Visible,
    /// 进程启动后的首次连接。
    ColdStart,
}

/// 休眠原因。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SleepReason {
    /// `idle` 模式下空闲计时到期（隐藏 / 冻结时计时更短，但原因不变）。
    #[default]
    Idle,
    /// `on-demand` 模式下任务完成后的保留时间已过。
    Grace,
    /// 进入后台时立即休眠（bfcache、移动端进后台），由 App / 封装层显式请求；不用于空闲计时到期。
    Background,
    /// App 主动请求。
    App,
}

/// 唤醒方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeKind {
    /// 自定义 URL scheme：`<scheme>://app-mcp/wake?token=`。
    Uri,
    /// Windows 打包应用：`IApplicationActivationManager::ActivateApplication`。
    Aumid,
    /// macOS Apple Event。
    AppleEvent,
    /// Linux D-Bus `org.freedesktop.Application.ActivateAction`。
    Dbus,
    /// Android 显式广播。
    AndroidIntent,
    /// 网页：打开 / 聚焦 URL（带 `#app-mcp-wake=<token>`）。
    WebUrl,
    /// 不可唤醒（Host 回退到清单 `launch`）。
    #[default]
    None,
}

/// 本实例的唤醒描述（spec/lifecycle.md 第 5 节）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeDescriptor {
    pub kind: WakeKind,
    /// 各 kind 的定位信息（scheme、AUMID、bundle id、D-Bus 名、组件名、URL）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 能否不把窗口带到前台就唤醒。
    #[serde(default)]
    pub background: bool,
}

/// `app/sleep` 参数。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepParams {
    pub reason: SleepReason,
    /// 未上报时 Host 回退到清单 `launch`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<WakeDescriptor>,
    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
    pub tools_hash: String,
}

/// `app/sleep` 结果：`{ accepted: true, resumeToken }` 或 `{ accepted: false, retryAfterMs }`。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepResult {
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// `app/lease` 参数。`ttl_ms` 为 0 表示取消租约。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseParams {
    pub ttl_ms: u64,
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// SDK 登记给 Host 的工具描述。`name` 在 App 内唯一（不含 appId 前缀）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    /// `[a-zA-Z0-9_.-]{1,64}`，如 `cart.checkout`。
    pub name: String,
    pub description: String,
    /// JSON Schema（`type` 必须为 `object`）。
    pub input_schema: Value,
    #[serde(default)]
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsSyncParams {
    pub tools: Vec<ToolInfo>,
}

/// 增量变更。同一名称在一条消息中只会出现在 `upserted` 或 `removed` 之一。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsChangedParams {
    #[serde(default)]
    pub upserted: Vec<ToolInfo>,
    #[serde(default)]
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsInvokeParams {
    /// 调用 ID，由 Host 生成，全局唯一。
    pub call_id: String,
    pub name: String,
    /// 已由 Host 按 inputSchema 校验过的参数。
    #[serde(default)]
    pub arguments: Value,
    /// SDK 侧超时（毫秒）。超时后 SDK 取消 handler 并返回 `TIMEOUT`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// 调用成功的结果。失败通过 JSON-RPC 错误返回（见 [`crate::ErrorKind`]）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsInvokeResult {
    /// handler 返回值；无返回值时为 `null`。
    #[serde(default)]
    pub data: Value,
    /// 调用后内容可能已变化的资源名，提示模型重新读取。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_hints: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCancelParams {
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

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

/// 空结果 `{}`。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Empty {}

/// 名称校验：工具与资源名 `[a-zA-Z0-9_.-]{1,64}`。
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// 局部名（工具 / 资源名）是否以 `<appId>.` 开头（spec/protocol.md 3.1）。
///
/// 协议上仍合法，但通常是误把全名 `<appId>.<局部名>` 写成了局部名；核心、Host、清单校验据此给出警告。
pub fn has_app_id_prefix(local_name: &str, app_id: &str) -> bool {
    !app_id.is_empty()
        && local_name.len() > app_id.len() + 1
        && local_name.starts_with(app_id)
        && local_name.as_bytes()[app_id.len()] == b'.'
}

/// [`has_app_id_prefix`] 成立时的统一警告文案。
pub fn app_id_prefix_warning(local_name: &str, app_id: &str) -> String {
    let suggested = local_name.get(app_id.len() + 1..).unwrap_or(local_name);
    format!(
        "名称 \"{local_name}\" 以 appId 前缀 \"{app_id}.\" 开头：名称是 App 内的局部名，Host 对外暴露为 \"{app_id}.{local_name}\"；\
         如果本意是全名，请改为 \"{suggested}\"（spec/protocol.md 3.1）"
    )
}

/// App ID 校验：`[a-z][a-z0-9-]{0,62}`。
pub fn is_valid_app_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_prefix_detection() {
        assert!(has_app_id_prefix("shop.info", "shop"));
        assert!(!has_app_id_prefix("shopping.info", "shop"));
        assert!(!has_app_id_prefix("shop", "shop"));
        assert!(!has_app_id_prefix("shop.", "shop"));
        assert!(!has_app_id_prefix("cart.checkout", "shop"));
        assert!(!has_app_id_prefix("info", ""));
        let w = app_id_prefix_warning("shop.info", "shop");
        assert!(w.contains("shop.shop.info") && w.contains("\"info\""), "{w}");
    }
    use serde_json::json;

    #[test]
    fn hello_is_camel_case_and_skips_none() {
        let p = HelloParams {
            app_id: "shop".into(),
            app_name: "示例商城".into(),
            protocol_version: crate::PROTOCOL_VERSION.into(),
            sdk_version: "0.1.0".into(),
            client_kind: ClientKind::Web,
            instance_id: "i-1".into(),
            app_version: None,
            origin: Some("http://localhost:5173".into()),
            instance_title: None,
            instance_url: None,
            token: None,
            launch_token: None,
            overview: None,
            resume_token: None,
            tools_hash: None,
            wake_reason: None,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v,
            json!({
                "appId": "shop", "appName": "示例商城", "protocolVersion": "1",
                "sdkVersion": "0.1.0", "clientKind": "web", "instanceId": "i-1",
                "origin": "http://localhost:5173"
            })
        );
        assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), p);
    }

    #[test]
    fn hello_lifecycle_fields() {
        let p = HelloParams {
            app_id: "shop".into(),
            resume_token: Some("r1".into()),
            tools_hash: Some("0123456789abcdef".into()),
            wake_reason: Some(WakeReason::OsActivation),
            ..Default::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["resumeToken"], "r1");
        assert_eq!(v["toolsHash"], "0123456789abcdef");
        assert_eq!(v["wakeReason"], "os-activation");
        assert_eq!(v["protocolVersion"], "1");
        assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), p);

        // toolsCurrent 缺省 false，false 时不序列化
        let r: HelloResult = serde_json::from_value(json!({
            "status": "paired", "protocolVersion": "1", "hostVersion": "h"
        }))
        .unwrap();
        assert!(!r.tools_current);
        assert!(serde_json::to_value(&r).unwrap().get("toolsCurrent").is_none());
        let r = HelloResult { status: PairingStatus::Paired, tools_current: true, ..Default::default() };
        assert_eq!(serde_json::to_value(&r).unwrap()["toolsCurrent"], true);
    }

    #[test]
    fn lifecycle_messages() {
        let p = SleepParams {
            reason: SleepReason::Grace,
            wake: Some(WakeDescriptor {
                kind: WakeKind::AndroidIntent,
                target: Some("dev.example/.WakeReceiver".into()),
                background: true,
            }),
            tools_hash: "abc".into(),
        };
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            json!({
                "reason": "grace",
                "wake": {"kind": "android-intent", "target": "dev.example/.WakeReceiver", "background": true},
                "toolsHash": "abc"
            })
        );
        let w: WakeDescriptor = serde_json::from_value(json!({"kind": "none"})).unwrap();
        assert_eq!(w, WakeDescriptor::default());
        for (k, s) in [
            (WakeKind::Uri, "uri"),
            (WakeKind::Aumid, "aumid"),
            (WakeKind::AppleEvent, "apple-event"),
            (WakeKind::Dbus, "dbus"),
            (WakeKind::WebUrl, "web-url"),
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(s));
        }
        assert_eq!(serde_json::to_value(WakeReason::ColdStart).unwrap(), json!("cold-start"));

        let ok: SleepResult = serde_json::from_value(json!({"accepted": true, "resumeToken": "r"})).unwrap();
        assert_eq!(ok.resume_token.as_deref(), Some("r"));
        let no: SleepResult = serde_json::from_value(json!({"accepted": false, "retryAfterMs": 5000})).unwrap();
        assert_eq!(no.retry_after_ms, Some(5000));
        assert_eq!(serde_json::to_value(LeaseParams { ttl_ms: 0 }).unwrap(), json!({"ttlMs": 0}));
    }

    #[test]
    fn overview_roundtrip() {
        let v = json!({"summary": "示例商城", "body": "## 能力范围\n- 购物车"});
        let o: AppOverview = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(o.locale, None);
        assert_eq!(serde_json::to_value(&o).unwrap(), v);
    }

    #[test]
    fn tool_info_defaults() {
        let t: ToolInfo = serde_json::from_value(json!({
            "name": "cart.checkout", "description": "结算", "inputSchema": {"type": "object"}
        }))
        .unwrap();
        assert_eq!(t.risk, Risk::Write);
        assert_eq!(t.activation, None);
        assert_eq!(serde_json::to_value(Risk::OsSensitive).unwrap(), json!("os-sensitive"));
    }

    #[test]
    fn name_validation() {
        assert!(is_valid_name("cart.checkout"));
        assert!(is_valid_name("a-b_c.D9"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("has space"));
        assert!(!is_valid_name(&"x".repeat(65)));
        assert!(is_valid_app_id("shop"));
        assert!(is_valid_app_id("my-app2"));
        assert!(!is_valid_app_id("Shop"));
        assert!(!is_valid_app_id("2shop"));
        assert!(!is_valid_app_id(""));
    }
}
