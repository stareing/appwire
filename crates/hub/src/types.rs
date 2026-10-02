//! Hub 公开数据类型（spec/hub-api.md 3.1、3.3）。
//!
//! 所有类型都实现 `Serialize` / `Deserialize`（camelCase），方便绑定层以 JSON 传递。

use std::time::Duration;

use app_mcp_protocol::{
    Activation, ContentAnnotations, ErrorKind, LifecycleMode, ResultStatus, Risk, ToolAnnotations, ToolError, ToolSurface,
    Visibility,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// App 与实例
// ---------------------------------------------------------------------------

/// App 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    /// 通过 App 端 SDK 连接（或只有静态清单）的 App。
    App,
    /// Hub 以子进程启动的上游 MCP 服务器。
    Upstream,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    /// 总览中的一句话简介。
    pub summary: Option<String>,
    pub connected: bool,
    /// 按连接顺序排列；上游为空。
    pub instances: Vec<InstanceInfo>,
    /// [`crate::Hub::select_instance`] 选定且仍连接的实例。
    pub selected_instance: Option<String>,
    /// 休眠中的实例（spec/lifecycle.md §9）：已与 Hub 完成 `app/sleep` 握手后断开，保留工具快照，
    /// 调用其工具时按需唤醒。按休眠时间排列。
    #[serde(default)]
    pub dormant_instances: Vec<InstanceInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub instance_id: String,
    /// `web` / `native` / `hybrid`。
    pub client_kind: String,
    /// 尚未上报时为 `visible`。
    pub visibility: Visibility,
    pub focused: bool,
    /// 最近活跃时间（Unix 毫秒）；从未活跃时为连接时间。
    pub last_active_ms: u64,
    /// 实例标题（网页为 `document.title`）。spec 之外的补充字段。
    pub title: Option<String>,
    /// 实例进程号：经本地 IPC（Unix 域套接字 / 命名管道）连接时由操作系统提供；
    /// 回环 TCP 连接与休眠实例为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// 连接 ID（spec/protocol.md 10.3），与 Hub 日志的 `cid` 字段、SDK 日志中的连接 ID 相同；休眠实例为 `None`。
    /// spec 之外的补充字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
}

// ---------------------------------------------------------------------------
// 工具与资源
// ---------------------------------------------------------------------------

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
    /// 厂商会话 ID（与 [`CallRequest::session`] 相同；`None` = 默认会话）。渐进暴露生效且 `apps` 为 `None` 时，
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

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CallRequest {
    /// 全名 `<appId>.<tool>`（内置工具为 `apps.list` 等）。
    pub name: String,
    /// 参数对象；`null` 视为 `{}`。
    pub arguments: Value,
    /// 指定实例：该实例必须已连接且注册了此工具，否则返回 `TOOL_NOT_FOUND`。`None` 按路由规则。
    pub instance_id: Option<String>,
    /// 本次调用的等待上限；`None` 用 [`crate::HubConfig::response_timeout`]。
    #[serde(with = "opt_millis")]
    pub timeout: Option<Duration>,
    /// 供 [`crate::Hub::cancel_call`] 使用；`None` 自动生成。
    pub call_id: Option<String>,
    /// 厂商会话 ID：总览的首次附带、`apps.select` 按会话计算；`None` = 默认会话。
    pub session: Option<String>,
    /// Agent 的幂等键：原样转交 App（`ToolsInvokeParams.idempotencyKey`，spec/protocol.md 3.3），1..=256 个字符；
    /// MCP 出口取自请求 `_meta` 的 `dev.appwire/idempotencyKey`（spec/hub-api.md 3.15）。
    pub idempotency_key: Option<String>,
}

impl CallRequest {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOutcome {
    pub call_id: String,
    #[serde(with = "result_json")]
    pub result: Result<Value, ToolError>,
    /// App 声明可能已变化的资源名（不含 appId 前缀）。
    pub state_hints: Vec<String>,
    /// 实际处理调用的实例（App 工具）。
    pub instance_id: Option<String>,
    /// 该会话首次接触此 App（或总览版本变化）时附带。
    pub overview: Option<AppOverviewInfo>,
    /// App 声明的业务状态（spec/protocol.md 3.2；缺省 `done`）。
    #[serde(default)]
    pub status: ResultStatus,
    /// `pending` 时可读取后续状态的资源 URI（`app-mcp://<appId>/<资源名>`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_resource: Option<String>,
    /// App 给出的一句结论。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// App 对结果内容的标注（MCP 内容注解），原样。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
    /// App 在后台、改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    /// Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App）。
    #[serde(default)]
    pub duration_ms: u64,
    /// 本次 App 工具调用是否经历了唤醒（调用时目标未连接，唤醒回连后才送达）。内置工具与上游工具恒为 `false`
    /// （`apps.activate` / `apps.navigate` 的结果自带 `woke`）。
    #[serde(default)]
    pub woke: bool,
}

/// Hub 操作失败：[`ToolError`] 的包装（同一套错误码，spec/protocol.md §4）。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct HubError(pub ToolError);

impl HubError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self(ToolError::new(kind, message))
    }
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }
    pub fn message(&self) -> &str {
        &self.0.message
    }
}

impl From<ToolError> for HubError {
    fn from(e: ToolError) -> Self {
        Self(e)
    }
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HubEvent {
    AppConnected {
        app_id: String,
        instance_id: String,
    },
    AppDisconnected {
        app_id: String,
        instance_id: String,
    },
    /// 已按 `list_changed_debounce` 合并。
    ToolsChanged,
    /// 已按 `list_changed_debounce` 合并。
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
    /// 实例与 Hub 完成 `app/sleep` 握手，进入休眠（工具仍列出，可唤醒；不另发 `ToolsChanged`）。
    AppDormant {
        app_id: String,
        instance_id: String,
    },
    /// Hub 正在唤醒 App：`instance_id` 为被唤醒的休眠实例；`None` 表示 App 未运行，按清单冷启动。
    AppWaking {
        app_id: String,
        instance_id: Option<String>,
    },
    /// SDK 上报了此前遇到的连接问题（`app/diagnostic`，spec/protocol.md 10.2），如浏览器拦截。
    AppDiagnostic {
        app_id: String,
        instance_id: String,
        /// 错误码（spec/protocol.md 10.1；可能是本 Hub 不认识的新码）。
        code: String,
        message: String,
        count: u32,
    },
}

// ---------------------------------------------------------------------------
// 运行状态（`/status`、`app-mcp-host doctor`）
// ---------------------------------------------------------------------------

/// [`crate::Hub::status`] 的结果，也是 `GET /status` 的响应体（spec/hub-api.md 3.8）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubStatus {
    /// Host 身份（`service` / `version` / `user` / `pid`）。
    #[serde(flatten)]
    pub identity: app_mcp_protocol::identity::HostIdentity,
    /// HTTP 服务实际监听的地址；未开启时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// 本地 IPC 端点；未开启时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_endpoint: Option<String>,
    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
    /// 是否提供 MCP Streamable HTTP（`/mcp`，TCP 与 IPC）。
    pub mcp_http: bool,
    pub auth: AuthStatus,
    /// 已初始化的 MCP 会话数。
    pub mcp_sessions: usize,
    /// App（含上游），按 appId 排序。
    pub apps: Vec<AppStatus>,
    /// 最近的 SDK 诊断上报（`app/diagnostic`），旧的在前，最多 [`crate::hub::MAX_REPORTS`] 条。
    pub reports: Vec<DiagnosticReport>,
    /// 租约策略与统计（spec/lifecycle.md 第 13 节 B2）；旧 Host 的 `/status` 没有该字段时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<crate::lease::LeaseStatus>,
    /// 资源保护策略（spec/hub-api.md 3.11，全部字段给出）；旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<crate::limits::LimitOverrides>,
    /// 结果与 `outputSchema` 不符时的处理；旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_validation: Option<crate::limits::OutputValidation>,
    /// 策略规则与命中次数、最近的加载错误（spec/hub-api.md 3.13）；旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<crate::policy::PolicyStatus>,
    /// 休眠记录持久化（spec/hub-api.md 3.5「持久化」）；未配置 `state_dir` 或旧 Host 时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dormant_store: Option<crate::dormant_store::DormantStoreStatus>,
}

/// 主 HTTP 服务的令牌策略。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// 是否配置了访问令牌（`/mcp` 浏览器来源必须携带；TCP 上的 `/status` 必须携带）。
    pub token_configured: bool,
    /// 不带 `Origin` 的本地客户端是否也必须携带令牌（`--auth all`）。
    pub token_required_without_origin: bool,
}

/// App 的整体状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppState {
    /// 至少一个实例在线（上游：子进程已连接）。
    Connected,
    /// 正在唤醒（有进行中的唤醒）。
    Waking,
    /// 没有在线实例，但有休眠实例。
    Dormant,
    /// 未连接（只有静态清单，或上游未连接）。
    Disconnected,
}

/// 实例状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    Connected,
    Dormant,
    /// 休眠实例正在被唤醒。
    Waking,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceStatus {
    #[serde(flatten)]
    pub info: InstanceInfo,
    pub state: InstanceState,
    /// 功耗观测（spec/lifecycle.md 第 12 节）；Hub 尚无该实例的计数时为 `None`。spec 之外的补充字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<InstancePower>,
}

/// 已连接实例当前不能休眠的原因中 Hub 可见的部分（App 的 `hold()` 只有 SDK 知道，不在其中）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AwakeReason {
    /// SDK 声明为 `persistent` 模式（不休眠）。
    Persistent,
    /// 有已路由到该实例、尚未完成的调用 / 资源读取。
    Call,
    /// 有未到期的租约（`app/lease`）。
    Lease,
    /// Host 订阅了该实例声明 `realtime` 的资源（spec/lifecycle.md 第 13 节 B3；普通资源的订阅不阻止休眠）。
    Subscription,
    /// 有待派发给该实例的唤醒。
    WakePending,
}

/// 每实例功耗观测（按 `(appId, instanceId)` 计数，跨重连与休眠保留；Hub 重启清零）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstancePower {
    /// 回连次数：Hub 启动以来该实例完成握手的次数减 1。
    pub reconnects: u64,
    /// 以该休眠实例为目标实际发出的唤醒激活次数（冷启动唤醒只计入 [`AppStatus::wakes`]）。
    pub wakes: u64,
    /// 累计在线秒数（含当前连接）。
    pub online_secs: u64,
    /// 心跳次数：Hub 发出的 `ping` 与收到 SDK 的 `ping` 之和。
    pub heartbeats: u64,
    /// SDK 在 `app/hello` 中声明的心跳间隔：`0` = 不发心跳（本地传输）；`None` = 旧 SDK（双向心跳）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_ms: Option<u64>,
    /// SDK 声明的生命周期模式；`None` = 未声明（旧 SDK）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_mode: Option<LifecycleMode>,
    /// 已连接实例当前不能休眠的原因（Hub 可见部分）；休眠实例与可以休眠时为空。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub awake_reasons: Vec<AwakeReason>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    pub state: AppState,
    /// 在线实例在前，其后为休眠实例。上游为空。
    pub instances: Vec<InstanceStatus>,
    /// 最近一次错误（握手被拒、唤醒失败 / 超时；上游为进程错误）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<LastError>,
    /// Hub 启动以来为该 App 实际发出的唤醒激活次数（含冷启动；上游为 0）。spec 之外的补充字段。
    #[serde(default)]
    pub wakes: u64,
    /// Hub 启动以来因限流被拒绝的调用次数（spec/hub-api.md 3.11）。
    #[serde(default)]
    pub rate_limited: u64,
    /// Hub 启动以来因大小上限被拒绝的参数 / 结果 / 资源次数（spec/hub-api.md 3.11）。
    #[serde(default)]
    pub too_large: u64,
    /// 各工具的声明（`risk` 与 MCP 注解），供 `app-mcp-host doctor` 展示（docs/plans/14-safety.md S5）。旧 Host 没有时为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDeclaration>,
}

/// 一个工具的声明（[`AppStatus::tools`]）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolDeclaration {
    /// 局部名（不含 appId）。
    pub name: String,
    /// 旧写法 `risk`（未声明时为缺省 `write`）。
    pub risk: Risk,
    /// App 声明的 MCP 注解（原样）；`None` = 未声明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    /// Agent 实际看到的注解（声明优先，缺少的按 `risk` 推导）。
    pub effective: ToolAnnotations,
    /// 是否声明了 `outputSchema`。
    pub output_schema: bool,
}

/// 最近一次错误。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastError {
    /// 连接级错误码（spec/protocol.md 10.1）或工具错误类别（第 4 节，如 `APP_NOT_RESPONDING`）；未知时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    /// 发生时刻（Unix 毫秒）；上游错误为 0（未记录时刻）。
    pub at_ms: u64,
}

/// 一条 SDK 诊断上报（`app/diagnostic`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub app_id: String,
    pub instance_id: String,
    /// 上报所在连接的连接 ID。
    pub connection_id: String,
    pub code: String,
    pub message: String,
    pub count: u32,
    /// Hub 收到的时刻（Unix 毫秒）。
    pub received_at_ms: u64,
}

// ---------------------------------------------------------------------------
// 策略回调
// ---------------------------------------------------------------------------

/// 审批策略。默认不审批（与原 Host 一致）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApprovalPolicy {
    /// 风险不低于此等级的工具调用前询问 [`ApprovalHandler`]；`None` = 不审批。
    pub require_at_or_above: Option<Risk>,
    /// 等待审批的上限；`None` 用 `response_timeout`。超时视为拒绝。spec 之外的补充字段。
    #[serde(with = "opt_millis")]
    pub timeout: Option<Duration>,
}

impl ApprovalPolicy {
    pub fn requires(&self, risk: Risk) -> bool {
        self.require_at_or_above
            .is_some_and(|min| risk_rank(risk) >= risk_rank(min))
    }
}

/// 厂商 UI 接管调用确认。
#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// 返回 `false` → 调用以 `USER_REJECTED` 结束。超时视为拒绝。
    async fn approve(&self, req: ApprovalRequest) -> bool;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub call_id: String,
    pub app_id: String,
    pub app_name: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    pub risk: Risk,
    pub arguments: Value,
    pub session: Option<String>,
    /// 工具的 MCP 注解（与 [`HubTool::annotations`] 相同），供厂商按声明决定是否确认。
    #[serde(default)]
    pub annotations: ToolAnnotations,
}

/// 厂商 UI 接管 App 配对。
#[async_trait::async_trait]
pub trait PairingHandler: Send + Sync {
    /// 未知 App（无静态清单，或 Origin 不在白名单）首次连接时询问。
    /// 返回 `false` → 握手结果为 `rejected`。超时（`pairing_timeout`）视为拒绝。
    async fn pair(&self, req: PairingRequest) -> bool;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRequest {
    pub app_id: String,
    pub app_name: String,
    pub origin: Option<String>,
    pub client_kind: String,
    /// spec 之外的补充字段。
    pub instance_id: String,
}

// ---------------------------------------------------------------------------
// serde 辅助
// ---------------------------------------------------------------------------

/// `Option<Duration>` ↔ 毫秒数。
mod opt_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => s.serialize_some(&(d.as_millis() as u64)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(Duration::from_millis))
    }
}

/// `Result<Value, ToolError>` ↔ `{"ok": value}` / `{"error": {kind, message, details}}`。
mod result_json {
    use app_mcp_protocol::{ErrorKind, ToolError};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    #[derive(Serialize, Deserialize)]
    struct ErrorJson {
        kind: ErrorKind,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Repr {
        Ok(Value),
        Error(ErrorJson),
    }

    pub fn serialize<S: Serializer>(v: &Result<Value, ToolError>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Ok(v) => Repr::Ok(v.clone()),
            Err(e) => Repr::Error(ErrorJson {
                kind: e.kind,
                message: e.message.clone(),
                details: e.details.clone(),
            }),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Result<Value, ToolError>, D::Error> {
        Ok(match Repr::deserialize(d)? {
            Repr::Ok(v) => Ok(v),
            Repr::Error(e) => Err(ToolError {
                kind: e.kind,
                message: e.message,
                details: e.details,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn approval_threshold() {
        let p = ApprovalPolicy {
            require_at_or_above: Some(Risk::Destructive),
            timeout: None,
        };
        assert!(!p.requires(Risk::Read));
        assert!(!p.requires(Risk::Write));
        assert!(p.requires(Risk::Destructive));
        assert!(p.requires(Risk::Payment));
        assert!(p.requires(Risk::OsSensitive));
        assert!(!ApprovalPolicy::default().requires(Risk::OsSensitive));
    }

    #[test]
    fn json_shapes() {
        let e = HubEvent::AppConnected {
            app_id: "shop".into(),
            instance_id: "i1".into(),
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"type": "appConnected", "appId": "shop", "instanceId": "i1"})
        );
        assert_eq!(
            serde_json::to_value(HubEvent::ToolsChanged).unwrap(),
            json!({"type": "toolsChanged"})
        );
        assert_eq!(
            serde_json::to_value(HubEvent::AppDormant { app_id: "a".into(), instance_id: "i".into() }).unwrap(),
            json!({"type": "appDormant", "appId": "a", "instanceId": "i"})
        );
        assert_eq!(
            serde_json::to_value(HubEvent::AppWaking { app_id: "a".into(), instance_id: None }).unwrap(),
            json!({"type": "appWaking", "appId": "a", "instanceId": null})
        );
        assert_eq!(serde_json::to_value(Availability::Dormant).unwrap(), json!("dormant"));
        let r: CallRequest =
            serde_json::from_value(json!({"name": "a.b", "timeout": 1500, "session": "s"}))
                .unwrap();
        assert_eq!(r.timeout, Some(Duration::from_millis(1500)));
        assert_eq!(r.arguments, Value::Null);
        let f: ToolFilter = serde_json::from_value(json!({"maxRisk": "write"})).unwrap();
        assert!(f.include_builtin);
        assert_eq!(f.max_risk, Some(Risk::Write));
        assert_eq!(f.session, None);
        let f: ToolFilter = serde_json::from_value(json!({"session": "s"})).unwrap();
        assert_eq!(f.session.as_deref(), Some("s"));
        assert_eq!(serde_json::to_value(ToolExposure::Progressive).unwrap(), json!("progressive"));
        assert_eq!(serde_json::from_value::<ToolExposure>(json!("all")).unwrap(), ToolExposure::All);
        assert_eq!(ToolExposure::default(), ToolExposure::Auto);
        let o = CallOutcome {
            call_id: "c".into(),
            result: Err(ToolError::new(ErrorKind::UserRejected, "不")),
            state_hints: vec![],
            instance_id: None,
            overview: None,
            annotations: None,
            state_resource: None,
            status: Default::default(),
            summary: None,
            routed_to: None,
            duration_ms: 5,
            woke: true,
        };
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!((v["durationMs"].clone(), v["woke"].clone()), (json!(5), json!(true)));
        assert_eq!(v["result"]["error"]["kind"], "USER_REJECTED");
        assert!(v.get("routedTo").is_none(), "未改调时不序列化");
        let back: CallOutcome = serde_json::from_value(v).unwrap();
        assert_eq!(back, o);
    }
}
