//! FFI 数据类型（`uniffi::Record` / `uniffi::Enum` / `uniffi::Error`）及与 `app_mcp_hub` 类型的转换。
//!
//! 约定：任意 JSON（参数、结果、schema、details）在 FFI 上以 JSON 文本（`String`）传递；
//! 时长以毫秒数（`u64`）传递；错误类别用协议字符串（如 `"USER_REJECTED"`）。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use app_mcp_hub as hub;
use serde_json::Value;

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

/// 风险等级。顺序：read < write < destructive < payment < os-sensitive。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Risk {
    Read,
    Write,
    Destructive,
    Payment,
    OsSensitive,
}

/// 工具暴露方式（spec/hub-api.md 3.7）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ToolExposure {
    /// 列出全部工具。
    All,
    /// 工具列表只含 `apps.*` 与本会话展开过（`apps.tools`）、调用过或选定了实例的 App 的工具。
    Progressive,
    /// App 与上游工具总数超过 `tool_exposure_threshold` 时按 `Progressive`，否则按 `All`（默认）。
    Auto,
}

impl From<ToolExposure> for hub::ToolExposure {
    fn from(v: ToolExposure) -> Self {
        match v {
            ToolExposure::All => hub::ToolExposure::All,
            ToolExposure::Progressive => hub::ToolExposure::Progressive,
            ToolExposure::Auto => hub::ToolExposure::Auto,
        }
    }
}

/// 唤醒器配置（spec/hub-api.md 3.5；对应 JSON 的 `"system"` / `"none"` / `{"exec": [...]}`）。
/// `set_waker` 设置的实现优先。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WakerConfig {
    /// 按平台执行系统激活（默认）。
    System,
    /// 不唤醒：休眠实例 / 未运行 App 的调用直接返回 `APP_DISCONNECTED`（JSON 中为 `"none"`）。
    Disabled,
    /// 执行 `argv[0] argv[1..]`（不经 shell），唤醒请求以一行 JSON 写入其 stdin。
    Exec { argv: Vec<String> },
}

impl From<WakerConfig> for hub::WakerConfig {
    fn from(v: WakerConfig) -> Self {
        match v {
            WakerConfig::System => hub::WakerConfig::System,
            WakerConfig::Disabled => hub::WakerConfig::None,
            WakerConfig::Exec { argv } => hub::WakerConfig::Exec(argv),
        }
    }
}

/// 调用时 App 需要的激活方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Activation {
    Headless,
    Background,
    Foreground,
}

/// 实例可见性。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Visibility {
    Visible,
    Hidden,
    Frozen,
}

/// App 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AppKind {
    /// 通过 App 端 SDK 连接（或只有静态清单）的 App。
    App,
    /// Hub 以子进程启动的上游 MCP 服务器。
    Upstream,
}

/// 工具当前是否可调用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Availability {
    /// 至少一个已连接实例注册了该工具。
    Available,
    /// App 未连接，工具来自静态清单。
    Disconnected,
    /// App 已连接，但没有实例注册该静态工具。
    NotRegistered,
    /// 只由休眠实例提供（来自休眠前的快照）；调用时 Hub 先唤醒实例再派发。
    Dormant,
    /// 其他不可直接调用的状态（兜底：Hub 未来新增、本绑定尚未单独映射的可用性）。
    Unavailable,
}

/// 唤醒方式（spec/lifecycle.md 第 5 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum WakeKind {
    /// 自定义 URL scheme：`<scheme>://app-mcp/wake?token=`。
    Uri,
    /// Windows 打包应用（AUMID）。
    Aumid,
    /// macOS Apple Event（bundle id）。
    AppleEvent,
    /// Linux D-Bus `org.freedesktop.Application.ActivateAction`。
    Dbus,
    /// Android 显式广播（`target` 为组件名，如 `com.example/.WakeReceiver`）。
    AndroidIntent,
    /// 网页：打开 / 聚焦 URL。
    WebUrl,
    /// 不可唤醒。
    None,
}

impl From<hub::WakeKind> for WakeKind {
    fn from(k: hub::WakeKind) -> Self {
        #[allow(unreachable_patterns)]
        match k {
            hub::WakeKind::Uri => WakeKind::Uri,
            hub::WakeKind::Aumid => WakeKind::Aumid,
            hub::WakeKind::AppleEvent => WakeKind::AppleEvent,
            hub::WakeKind::Dbus => WakeKind::Dbus,
            hub::WakeKind::AndroidIntent => WakeKind::AndroidIntent,
            hub::WakeKind::WebUrl => WakeKind::WebUrl,
            _ => WakeKind::None,
        }
    }
}

/// 工具定义 / 调用格式（spec/hub-api.md 第 5 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ToolFormat {
    Mcp,
    OpenAiChat,
    OpenAiResponses,
    Anthropic,
    Gemini,
}

macro_rules! enum_map {
    ($ffi:ident <=> $hub:path { $($v:ident),* $(,)? }) => {
        impl From<$ffi> for $hub {
            fn from(v: $ffi) -> Self {
                match v { $($ffi::$v => <$hub>::$v,)* }
            }
        }
        impl From<$hub> for $ffi {
            fn from(v: $hub) -> Self {
                match v { $(<$hub>::$v => $ffi::$v,)* }
            }
        }
    };
}

enum_map!(Risk <=> hub::Risk { Read, Write, Destructive, Payment, OsSensitive });
enum_map!(Activation <=> hub::Activation { Headless, Background, Foreground });
enum_map!(Visibility <=> hub::Visibility { Visible, Hidden, Frozen });
enum_map!(AppKind <=> hub::AppKind { App, Upstream });

impl From<hub::Availability> for Availability {
    fn from(v: hub::Availability) -> Self {
        #[allow(unreachable_patterns)]
        match v {
            hub::Availability::Available => Availability::Available,
            hub::Availability::Disconnected => Availability::Disconnected,
            hub::Availability::NotRegistered => Availability::NotRegistered,
            hub::Availability::Dormant => Availability::Dormant,
            // 兜底：Hub 新增的可用性变体。
            _ => Availability::Unavailable,
        }
    }
}
enum_map!(ToolFormat <=> hub::ToolFormat { Mcp, OpenAiChat, OpenAiResponses, Anthropic, Gemini });

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// Hub 操作的错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error, uniffi::Error)]
pub enum HubError {
    /// 协议错误（spec/protocol.md §4）：`kind` 如 `"TOOL_NOT_FOUND"`、`"RESOURCE_NOT_FOUND"`。
    #[error("{kind}: {reason}")]
    Tool {
        kind: String,
        reason: String,
        details_json: Option<String>,
    },
    #[error("JSON 不合法：{detail}")]
    InvalidJson { detail: String },
    #[error("配置不合法：{detail}")]
    InvalidConfig { detail: String },
    #[error("I/O 错误：{detail}")]
    Io { detail: String },
    #[error("Hub 已停止")]
    Shutdown,
}

impl From<hub::HubError> for HubError {
    fn from(e: hub::HubError) -> Self {
        let e = e.0;
        HubError::Tool {
            kind: e.kind.as_str().to_owned(),
            reason: e.message,
            details_json: e.details.map(|d| d.to_string()),
        }
    }
}

impl From<std::io::Error> for HubError {
    fn from(e: std::io::Error) -> Self {
        HubError::Io {
            detail: e.to_string(),
        }
    }
}

/// `WakeResponder::fail` 的错误：`kind` 为协议错误类别（如 `"LAUNCH_FAILED"`、`"APP_NOT_INSTALLED"`），
/// 不认识的类别按 `LAUNCH_FAILED` 处理。
pub(crate) fn wake_error(kind: &str, reason: String) -> hub::HubError {
    let kind = serde_json::from_value::<hub::ErrorKind>(Value::String(kind.to_owned()))
        .unwrap_or(hub::ErrorKind::LaunchFailed);
    let message = if reason.is_empty() { "唤醒失败。".to_owned() } else { reason };
    hub::HubError::new(kind, message)
}

pub(crate) fn parse_json(text: &str) -> Result<Value, HubError> {
    serde_json::from_str(text).map_err(|e| HubError::InvalidJson {
        detail: e.to_string(),
    })
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 上游 MCP 服务器（Hub 以子进程启动，stdio 传输）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct UpstreamSpec {
    /// 名称，须满足 appId 规则，且不能与清单 appId 或保留名冲突。
    pub name: String,
    pub command: String,
    #[uniffi(default = [])]
    pub args: Vec<String>,
    /// 额外环境变量（Kotlin / Swift / Python 中需显式传空表）。
    pub env: HashMap<String, String>,
}

/// Hub 配置。可选字段为空时使用 `app_mcp_hub::HubConfig` 的默认值。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HubConfig {
    /// App 连接服务（WebSocket）监听地址，端口 0 = 随机。为空时为 `127.0.0.1:7717`。
    #[uniffi(default = None)]
    pub ws_addr: Option<String>,
    /// `false` = 不开 WebSocket 服务（仅上游）。
    #[uniffi(default = true)]
    pub enable_ws: bool,
    /// 静态清单文件路径。加载失败的清单记录日志后跳过。
    #[uniffi(default = [])]
    pub manifest_files: Vec<String>,
    /// 静态清单目录（按文件名排序加载；不存在时忽略）。
    #[uniffi(default = None)]
    pub manifest_dir: Option<String>,
    /// 静态清单 JSON 文本（如 Android assets 中读出的内容）。不合法时 `start` 返回 `InvalidConfig`。
    #[uniffi(default = [])]
    pub manifests_json: Vec<String>,
    /// 额外允许的 Origin 模式（默认已允许 localhost / 127.0.0.1 任意端口）。
    #[uniffi(default = [])]
    pub allow_origins: Vec<String>,
    #[uniffi(default = [])]
    pub upstreams: Vec<UpstreamSpec>,
    /// 风险不低于此等级的调用先询问 `ApprovalHandler`；为空 = 不审批。
    #[uniffi(default = None)]
    pub approval_min_risk: Option<Risk>,
    /// 等待审批的上限；为空时用 `response_timeout_ms`。超时视为拒绝。
    #[uniffi(default = None)]
    pub approval_timeout_ms: Option<u64>,
    /// 等待 `PairingHandler` 的上限（默认 120 s），超时视为拒绝。
    #[uniffi(default = None)]
    pub pairing_timeout_ms: Option<u64>,
    #[uniffi(default = None)]
    pub ping_interval_ms: Option<u64>,
    #[uniffi(default = None)]
    pub idle_timeout_ms: Option<u64>,
    #[uniffi(default = None)]
    pub hidden_idle_timeout_ms: Option<u64>,
    /// SDK 侧超时（`tools/invoke` 的 `timeoutMs`）。
    #[uniffi(default = None)]
    pub invoke_timeout_ms: Option<u64>,
    /// Hub 侧等待 SDK 响应的时间。
    #[uniffi(default = None)]
    pub response_timeout_ms: Option<u64>,
    /// 列表变化通知的合并窗口。
    #[uniffi(default = None)]
    pub list_changed_debounce_ms: Option<u64>,
    // ---- 生命周期（spec/hub-api.md 3.5）----
    /// 调用实例完成后发送的租约时长（默认 60 s）；`0` 关闭租约。
    #[uniffi(default = None)]
    pub lease_ttl_ms: Option<u64>,
    /// 唤醒后等待 App 回连的上限（默认 15 s），超时 → `APP_NOT_RESPONDING`。
    #[uniffi(default = None)]
    pub wake_timeout_ms: Option<u64>,
    /// 唤醒令牌有效期（默认 60 s）。
    #[uniffi(default = None)]
    pub wake_token_ttl_ms: Option<u64>,
    /// 休眠记录保留时长（默认 24 小时）。
    #[uniffi(default = None)]
    pub dormant_ttl_ms: Option<u64>,
    /// 同一 appId 以新实例 ID 连接时移除其休眠记录（默认 `true`）。
    #[uniffi(default = None)]
    pub dormant_replaced_by_new_instance: Option<bool>,
    /// App 未运行且清单无显式 `wake` 时，是否由清单 `launch` 推导唤醒方式（默认 `false`）。
    #[uniffi(default = None)]
    pub wake_from_launch: Option<bool>,
    /// 唤醒器（默认 `System`）；`set_waker` 设置的实现优先，清除后恢复为此配置。
    #[uniffi(default = None)]
    pub waker: Option<WakerConfig>,
    // ---- 渐进暴露（spec/hub-api.md 3.7）----
    /// 工具暴露方式（默认 `Auto`）。
    #[uniffi(default = None)]
    pub tool_exposure: Option<ToolExposure>,
    /// `Auto` 的阈值：App 与上游工具总数超过此值时渐进暴露（默认 40）。
    #[uniffi(default = None)]
    pub tool_exposure_threshold: Option<u32>,
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig {
            ws_addr: None,
            enable_ws: true,
            manifest_files: Vec::new(),
            manifest_dir: None,
            manifests_json: Vec::new(),
            allow_origins: Vec::new(),
            upstreams: Vec::new(),
            approval_min_risk: None,
            approval_timeout_ms: None,
            pairing_timeout_ms: None,
            ping_interval_ms: None,
            idle_timeout_ms: None,
            hidden_idle_timeout_ms: None,
            invoke_timeout_ms: None,
            response_timeout_ms: None,
            list_changed_debounce_ms: None,
            lease_ttl_ms: None,
            wake_timeout_ms: None,
            wake_token_ttl_ms: None,
            dormant_ttl_ms: None,
            dormant_replaced_by_new_instance: None,
            wake_from_launch: None,
            waker: None,
            tool_exposure: None,
            tool_exposure_threshold: None,
        }
    }
}

impl HubConfig {
    pub(crate) fn into_hub(self) -> Result<hub::HubConfig, HubError> {
        let mut c = hub::HubConfig::default();
        if !self.enable_ws {
            c.ws_addr = None;
        } else if let Some(addr) = self.ws_addr {
            c.ws_addr = Some(addr);
        }
        let files: Vec<PathBuf> = self.manifest_files.into_iter().map(PathBuf::from).collect();
        let dir = self.manifest_dir.map(PathBuf::from);
        c.manifests = hub::load_manifests(&files, dir.as_deref(), false);
        for (i, text) in self.manifests_json.iter().enumerate() {
            let loaded = app_mcp_manifest::load_str(text).map_err(|e| HubError::InvalidConfig {
                detail: format!("manifests_json[{i}]：{e}"),
            })?;
            c.manifests.push(loaded.manifest);
        }
        c.allow_origins = self.allow_origins;
        let mut upstreams = BTreeMap::new();
        for u in self.upstreams {
            if upstreams.contains_key(&u.name) {
                return Err(HubError::InvalidConfig {
                    detail: format!("上游名称重复：{}", u.name),
                });
            }
            upstreams.insert(
                u.name,
                hub::UpstreamConfig {
                    command: u.command,
                    args: u.args,
                    env: u.env.into_iter().collect(),
                },
            );
        }
        c.upstreams = upstreams;
        c.approval = hub::ApprovalPolicy {
            require_at_or_above: self.approval_min_risk.map(Into::into),
            timeout: self.approval_timeout_ms.map(Duration::from_millis),
        };
        let set = |slot: &mut Duration, v: Option<u64>| {
            if let Some(ms) = v {
                *slot = Duration::from_millis(ms);
            }
        };
        set(&mut c.pairing_timeout, self.pairing_timeout_ms);
        set(&mut c.ping_interval, self.ping_interval_ms);
        set(&mut c.idle_timeout, self.idle_timeout_ms);
        set(&mut c.hidden_idle_timeout, self.hidden_idle_timeout_ms);
        set(&mut c.invoke_timeout, self.invoke_timeout_ms);
        set(&mut c.response_timeout, self.response_timeout_ms);
        set(&mut c.list_changed_debounce, self.list_changed_debounce_ms);
        set(&mut c.lease_ttl, self.lease_ttl_ms);
        set(&mut c.wake_timeout, self.wake_timeout_ms);
        set(&mut c.wake_token_ttl, self.wake_token_ttl_ms);
        set(&mut c.dormant_ttl, self.dormant_ttl_ms);
        if let Some(v) = self.dormant_replaced_by_new_instance {
            c.dormant_replaced_by_new_instance = v;
        }
        if let Some(v) = self.wake_from_launch {
            c.wake_from_launch = v;
        }
        if let Some(w) = self.waker {
            c.waker = w.into();
        }
        if let Some(v) = self.tool_exposure {
            c.tool_exposure = v.into();
        }
        if let Some(v) = self.tool_exposure_threshold {
            c.tool_exposure_threshold = v as usize;
        }
        Ok(c)
    }
}

// ---------------------------------------------------------------------------
// App 与实例
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct InstanceInfo {
    pub instance_id: String,
    /// `web` / `native` / `hybrid`。
    pub client_kind: String,
    pub visibility: Visibility,
    pub focused: bool,
    /// 最近活跃时间（Unix 毫秒）。
    pub last_active_ms: u64,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppInfo {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    pub summary: Option<String>,
    pub connected: bool,
    pub instances: Vec<InstanceInfo>,
    pub selected_instance: Option<String>,
    /// 休眠中的实例（按休眠时间排列）；调用其工具时 Hub 先唤醒。`connected` 只看已连接实例。
    pub dormant_instances: Vec<InstanceInfo>,
}

impl From<hub::InstanceInfo> for InstanceInfo {
    fn from(i: hub::InstanceInfo) -> Self {
        InstanceInfo {
            instance_id: i.instance_id,
            client_kind: i.client_kind,
            visibility: i.visibility.into(),
            focused: i.focused,
            last_active_ms: i.last_active_ms,
            title: i.title,
        }
    }
}

impl From<hub::AppInfo> for AppInfo {
    fn from(a: hub::AppInfo) -> Self {
        AppInfo {
            app_id: a.app_id,
            name: a.name,
            kind: a.kind.into(),
            summary: a.summary,
            connected: a.connected,
            instances: a.instances.into_iter().map(Into::into).collect(),
            selected_instance: a.selected_instance,
            dormant_instances: a.dormant_instances.into_iter().map(Into::into).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// 工具与资源
// ---------------------------------------------------------------------------

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
    pub risk: Risk,
    pub activation: Activation,
    pub availability: Availability,
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

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CallRequest {
    /// 全名 `<appId>.<tool>`。
    pub name: String,
    /// 参数 JSON 对象文本；为空视为 `{}`。
    #[uniffi(default = None)]
    pub arguments_json: Option<String>,
    /// 严格指定实例；为空按路由规则。
    #[uniffi(default = None)]
    pub instance_id: Option<String>,
    /// 等待上限；为空用配置的 `response_timeout_ms`。
    #[uniffi(default = None)]
    pub timeout_ms: Option<u64>,
    /// 供 `cancel_call`；为空自动生成（结果中的 `call_id`）。
    #[uniffi(default = None)]
    pub call_id: Option<String>,
    /// 厂商会话 ID；为空 = 默认会话。
    #[uniffi(default = None)]
    pub session: Option<String>,
}

impl CallRequest {
    pub(crate) fn into_hub(self) -> Result<hub::CallRequest, HubError> {
        let arguments = match self.arguments_json.as_deref().map(str::trim) {
            None | Some("") => Value::Object(Default::default()),
            Some(text) => parse_json(text)?,
        };
        Ok(hub::CallRequest {
            name: self.name,
            arguments,
            instance_id: self.instance_id,
            timeout: self.timeout_ms.map(Duration::from_millis),
            call_id: self.call_id,
            session: self.session,
        })
    }
}

/// 工具层面的失败。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ToolErrorInfo {
    /// 错误类别，如 `"USER_REJECTED"`、`"TIMEOUT"`、`"APP_DISCONNECTED"`。
    pub kind: String,
    pub message: String,
    pub details_json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CallOutcome {
    pub call_id: String,
    /// 成功时的结果 JSON 文本（`error` 为空）。
    pub data_json: Option<String>,
    /// 失败时的错误（`data_json` 为空）。
    pub error: Option<ToolErrorInfo>,
    /// App 声明可能已变化的资源名。
    pub state_hints: Vec<String>,
    pub instance_id: Option<String>,
    /// 该会话首次接触此 App 时附带。
    pub overview: Option<AppOverviewInfo>,
}

impl From<hub::CallOutcome> for CallOutcome {
    fn from(o: hub::CallOutcome) -> Self {
        let (data_json, error) = match o.result {
            Ok(v) => (Some(v.to_string()), None),
            Err(e) => (
                None,
                Some(ToolErrorInfo {
                    kind: e.kind.as_str().to_owned(),
                    message: e.message,
                    details_json: e.details.map(|d| d.to_string()),
                }),
            ),
        };
        CallOutcome {
            call_id: o.call_id,
            data_json,
            error,
            state_hints: o.state_hints,
            instance_id: o.instance_id,
            overview: o.overview.map(Into::into),
        }
    }
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum HubEvent {
    AppConnected {
        app_id: String,
        instance_id: String,
    },
    AppDisconnected {
        app_id: String,
        instance_id: String,
    },
    /// 已合并；收到后应重新列工具 / 导出。
    ToolsChanged,
    ResourcesChanged,
    ResourceUpdated {
        uri: String,
    },
    VisibilityChanged {
        app_id: String,
        instance_id: String,
        visibility: Visibility,
    },
    UpstreamState {
        name: String,
        connected: bool,
        error: Option<String>,
    },
    /// 实例进入休眠：工具仍列出（`Availability::Dormant`），不另发 `ToolsChanged`。
    AppDormant {
        app_id: String,
        instance_id: String,
    },
    /// Hub 正在唤醒 App；`instance_id` 为空表示 App 未运行、按清单冷启动。
    AppWaking {
        app_id: String,
        instance_id: Option<String>,
    },
    /// 本绑定尚未单独映射的 Hub 事件（兜底，兼容未来新增）。`kind` 为事件类型名，
    /// `json` 为事件的完整 JSON 文本。
    Other {
        kind: String,
        json: String,
    },
}

impl From<hub::HubEvent> for HubEvent {
    fn from(e: hub::HubEvent) -> Self {
        use hub::HubEvent as H;
        match e {
            H::AppConnected {
                app_id,
                instance_id,
            } => HubEvent::AppConnected {
                app_id,
                instance_id,
            },
            H::AppDisconnected {
                app_id,
                instance_id,
            } => HubEvent::AppDisconnected {
                app_id,
                instance_id,
            },
            H::ToolsChanged => HubEvent::ToolsChanged,
            H::ResourcesChanged => HubEvent::ResourcesChanged,
            H::ResourceUpdated { uri } => HubEvent::ResourceUpdated { uri },
            H::VisibilityChanged {
                app_id,
                instance_id,
                visibility,
            } => HubEvent::VisibilityChanged {
                app_id,
                instance_id,
                visibility: visibility.into(),
            },
            H::UpstreamState {
                name,
                connected,
                error,
            } => HubEvent::UpstreamState {
                name,
                connected,
                error,
            },
            H::AppDormant {
                app_id,
                instance_id,
            } => HubEvent::AppDormant {
                app_id,
                instance_id,
            },
            H::AppWaking {
                app_id,
                instance_id,
            } => HubEvent::AppWaking {
                app_id,
                instance_id,
            },
            #[allow(unreachable_patterns)]
            other => other_event(&other),
        }
    }
}

/// 兜底映射：取 serde 标签 `type` 作为类别，整段 JSON 原样透传。
#[allow(dead_code)]
fn other_event(e: &hub::HubEvent) -> HubEvent {
    let value = serde_json::to_value(e).unwrap_or(Value::Null);
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    HubEvent::Other {
        kind,
        json: value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 审批 / 配对请求
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub app_id: String,
    pub app_name: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    pub risk: Risk,
    /// 参数 JSON 文本（已按 schema 校验）。
    pub arguments_json: String,
    pub session: Option<String>,
}

impl From<hub::ApprovalRequest> for ApprovalRequest {
    fn from(r: hub::ApprovalRequest) -> Self {
        ApprovalRequest {
            call_id: r.call_id,
            app_id: r.app_id,
            app_name: r.app_name,
            tool: r.tool,
            title: r.title,
            description: r.description,
            risk: r.risk.into(),
            arguments_json: r.arguments.to_string(),
            session: r.session,
        }
    }
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PairingRequest {
    pub app_id: String,
    pub app_name: String,
    pub origin: Option<String>,
    pub client_kind: String,
    pub instance_id: String,
}

impl From<hub::PairingRequest> for PairingRequest {
    fn from(r: hub::PairingRequest) -> Self {
        PairingRequest {
            app_id: r.app_id,
            app_name: r.app_name,
            origin: r.origin,
            client_kind: r.client_kind,
            instance_id: r.instance_id,
        }
    }
}

// ---------------------------------------------------------------------------
// 唤醒请求
// ---------------------------------------------------------------------------

/// 唤醒描述。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WakeDescriptor {
    pub kind: WakeKind,
    /// scheme、AUMID、bundle id、D-Bus 名称、组件名或 URL。
    pub target: Option<String>,
    /// 能否不把窗口带到前台就唤醒。
    pub background: bool,
}

/// 交给 `HubWaker` 的唤醒请求（spec/hub-api.md 3.5）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WakeRequest {
    pub app_id: String,
    /// 被唤醒的休眠实例；为空表示 App 未运行，按清单冷启动。
    pub instance_id: Option<String>,
    pub descriptor: WakeDescriptor,
    /// 一次性唤醒令牌（32 位十六进制）。
    pub token: String,
    /// 通用激活参数 `app-mcp-wake:<token>`，App 端 SDK 的 `handleWake` 可识别。
    pub activation_arg: String,
}

impl From<hub::WakeRequest> for WakeRequest {
    fn from(r: hub::WakeRequest) -> Self {
        WakeRequest {
            app_id: r.app_id,
            instance_id: r.instance_id,
            descriptor: WakeDescriptor {
                kind: r.descriptor.kind.into(),
                target: r.descriptor.target,
                background: r.descriptor.background,
            },
            token: r.token,
            activation_arg: r.activation_arg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_defaults_follow_hub() {
        let c = HubConfig::default().into_hub().unwrap();
        let d = hub::HubConfig::default();
        assert_eq!(c.ws_addr, d.ws_addr);
        assert_eq!(c.response_timeout, d.response_timeout);
        assert_eq!(c.approval, d.approval);
        assert!(c.manifests.is_empty() && c.upstreams.is_empty());
    }

    #[test]
    fn config_overrides() {
        let c = HubConfig {
            ws_addr: Some("127.0.0.1:0".into()),
            approval_min_risk: Some(Risk::Destructive),
            approval_timeout_ms: Some(500),
            response_timeout_ms: Some(1234),
            upstreams: vec![UpstreamSpec {
                name: "fs".into(),
                command: "npx".into(),
                args: vec!["x".into()],
                env: HashMap::from([("A".into(), "1".into())]),
            }],
            manifests_json: vec![r#"{"manifestVersion":1,"appId":"shop","name":"Shop"}"#.into()],
            ..Default::default()
        }
        .into_hub()
        .unwrap();
        assert_eq!(c.ws_addr.as_deref(), Some("127.0.0.1:0"));
        assert_eq!(c.approval.require_at_or_above, Some(hub::Risk::Destructive));
        assert_eq!(c.approval.timeout, Some(Duration::from_millis(500)));
        assert_eq!(c.response_timeout, Duration::from_millis(1234));
        assert_eq!(c.upstreams["fs"].env["A"], "1");
        assert_eq!(c.manifests[0].app_id, "shop");

        let off = HubConfig {
            enable_ws: false,
            ws_addr: Some("127.0.0.1:1".into()),
            ..Default::default()
        }
        .into_hub()
        .unwrap();
        assert_eq!(off.ws_addr, None);

        let bad = HubConfig {
            manifests_json: vec!["{".into()],
            ..Default::default()
        }
        .into_hub();
        assert!(matches!(bad, Err(HubError::InvalidConfig { .. })));
    }

    #[test]
    fn call_request_arguments() {
        let r = CallRequest {
            name: "a.b".into(),
            arguments_json: None,
            instance_id: None,
            timeout_ms: Some(10),
            call_id: None,
            session: Some("s".into()),
        };
        let h = r.clone().into_hub().unwrap();
        assert_eq!(h.arguments, json!({}));
        assert_eq!(h.timeout, Some(Duration::from_millis(10)));
        let h = CallRequest {
            arguments_json: Some(r#"{"x":1}"#.into()),
            ..r.clone()
        }
        .into_hub()
        .unwrap();
        assert_eq!(h.arguments, json!({"x": 1}));
        assert!(matches!(
            CallRequest {
                arguments_json: Some("nope".into()),
                ..r
            }
            .into_hub(),
            Err(HubError::InvalidJson { .. })
        ));
    }

    #[test]
    fn outcome_and_error_conversion() {
        let o: CallOutcome = hub::CallOutcome {
            call_id: "c".into(),
            result: Err(hub::ToolError::new(hub::ErrorKind::UserRejected, "不")
                .with_details(json!({"a": 1}))),
            state_hints: vec![],
            instance_id: None,
            overview: None,
        }
        .into();
        let e = o.error.unwrap();
        assert_eq!(e.kind, "USER_REJECTED");
        assert_eq!(e.details_json.as_deref(), Some(r#"{"a":1}"#));
        assert!(o.data_json.is_none());

        let err: HubError = hub::HubError::new(hub::ErrorKind::ToolNotFound, "x").into();
        assert_eq!(
            err,
            HubError::Tool {
                kind: "TOOL_NOT_FOUND".into(),
                reason: "x".into(),
                details_json: None
            }
        );
    }

    #[test]
    fn filter_and_event_conversion() {
        let f: hub::ToolFilter = ToolFilter {
            max_risk: Some(Risk::Write),
            ..Default::default()
        }
        .into();
        assert_eq!(f.max_risk, Some(hub::Risk::Write));
        assert!(f.include_builtin);
        assert_eq!(f.session, None);
        let f: hub::ToolFilter = ToolFilter { session: Some("s".into()), ..Default::default() }.into();
        assert_eq!(f.session.as_deref(), Some("s"));
        let e: HubEvent = hub::HubEvent::VisibilityChanged {
            app_id: "a".into(),
            instance_id: "i".into(),
            visibility: hub::Visibility::Hidden,
        }
        .into();
        assert_eq!(
            e,
            HubEvent::VisibilityChanged {
                app_id: "a".into(),
                instance_id: "i".into(),
                visibility: Visibility::Hidden
            }
        );
        assert_eq!(ToolFormat::from(hub::ToolFormat::Gemini), ToolFormat::Gemini);
    }

    #[test]
    fn lifecycle_mapping() {
        let c = HubConfig {
            lease_ttl_ms: Some(0),
            wake_timeout_ms: Some(2000),
            wake_token_ttl_ms: Some(3000),
            dormant_ttl_ms: Some(4000),
            dormant_replaced_by_new_instance: Some(false),
            wake_from_launch: Some(true),
            ..Default::default()
        }
        .into_hub()
        .unwrap();
        assert_eq!(c.lease_ttl, Duration::ZERO);
        assert_eq!(c.wake_timeout, Duration::from_millis(2000));
        assert_eq!(c.wake_token_ttl, Duration::from_millis(3000));
        assert_eq!(c.dormant_ttl, Duration::from_millis(4000));
        assert!(!c.dormant_replaced_by_new_instance && c.wake_from_launch);
        let d = HubConfig::default().into_hub().unwrap();
        assert_eq!(d.lease_ttl, hub::HubConfig::default().lease_ttl);
        assert!(d.dormant_replaced_by_new_instance && !d.wake_from_launch);
        assert_eq!(d.waker, hub::WakerConfig::System);
        assert_eq!(d.tool_exposure, hub::ToolExposure::Auto);
        assert_eq!(d.tool_exposure_threshold, hub::DEFAULT_TOOL_EXPOSURE_THRESHOLD);
        let c = HubConfig {
            waker: Some(WakerConfig::Disabled),
            tool_exposure: Some(ToolExposure::Progressive),
            tool_exposure_threshold: Some(5),
            ..Default::default()
        }
        .into_hub()
        .unwrap();
        assert_eq!(c.waker, hub::WakerConfig::None);
        assert_eq!(c.tool_exposure, hub::ToolExposure::Progressive);
        assert_eq!(c.tool_exposure_threshold, 5);
        let c = HubConfig {
            waker: Some(WakerConfig::Exec { argv: vec!["node".into(), "w.mjs".into()] }),
            ..Default::default()
        }
        .into_hub()
        .unwrap();
        assert_eq!(c.waker, hub::WakerConfig::Exec(vec!["node".into(), "w.mjs".into()]));

        assert_eq!(Availability::from(hub::Availability::Dormant), Availability::Dormant);
        assert_eq!(
            HubEvent::from(hub::HubEvent::AppDormant { app_id: "a".into(), instance_id: "i".into() }),
            HubEvent::AppDormant { app_id: "a".into(), instance_id: "i".into() }
        );
        assert_eq!(
            HubEvent::from(hub::HubEvent::AppWaking { app_id: "a".into(), instance_id: None }),
            HubEvent::AppWaking { app_id: "a".into(), instance_id: None }
        );
        let e = wake_error("APP_NOT_INSTALLED", "没装".into());
        assert_eq!((e.kind(), e.message()), (hub::ErrorKind::AppNotInstalled, "没装"));
        let e = wake_error("NOPE", String::new());
        assert_eq!(e.kind(), hub::ErrorKind::LaunchFailed);
        let r = WakeRequest::from(hub::WakeRequest {
            app_id: "a".into(),
            instance_id: Some("i".into()),
            descriptor: hub::WakeDescriptor {
                kind: hub::WakeKind::AndroidIntent,
                target: Some("p/.R".into()),
                background: true,
            },
            token: "t".into(),
            activation_arg: "app-mcp-wake:t".into(),
        });
        assert_eq!(r.descriptor.kind, WakeKind::AndroidIntent);
        assert_eq!(r.descriptor.target.as_deref(), Some("p/.R"));
    }

    #[test]
    fn other_event_fallback_keeps_type_and_json() {
        let e = other_event(&hub::HubEvent::ResourceUpdated { uri: "u://x".into() });
        let HubEvent::Other { kind, json } = e else {
            panic!("应为 Other");
        };
        assert_eq!(kind, "resourceUpdated");
        let v: Value = serde_json::from_str(&json).expect("json");
        assert_eq!(v["uri"], "u://x");
    }
}
