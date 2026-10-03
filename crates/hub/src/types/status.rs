//! 运行状态（`/status`、`app-mcp-host doctor`）：App、实例、Agent 任务、功耗、诊断上报。

use app_mcp_protocol::{LifecycleMode, Risk, ToolAnnotations};
use serde::{Deserialize, Serialize};

use super::apps::{AppKind, InstanceInfo};

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
    /// 进行中的 `subscriptions/listen` 流数（无会话请求的通知订阅，spec/hub-api.md 3.6「通知」）；旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_listen_streams: Option<usize>,
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
    /// Agent 任务（调用方的跨请求状态，spec/hub-api.md 3.6），按调用方键排序；只读。旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<AgentTaskStatus>>,
    /// 已登记的 Agent 名（第 16 项 N5，不含令牌），按名字排序。旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
    /// 按调用方记账（第 16 项 P3）：各主体的调用 / 唤醒 / 被限流次数与字节数，按 App 细分，按主体排序。旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Vec<crate::usage::UsageStatus>>,
    /// 未到期的对象锁（第 16 项 N6，spec/hub-api.md 3.6「对象锁」），按 appId、key 排序。旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locks: Option<Vec<crate::object_lock::LockStatus>>,
    /// 进行中的调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」），按开始时刻排序。旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<Vec<crate::call_objects::CallStatus>>,
    /// 事件订阅（各订阅的投递 / 丢弃数与订阅方积压）与丢弃的不合法事件数（第 16 项 N3，spec/hub-api.md 3.17）。旧 Host 没有时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<crate::events::EventsStatus>,
}

/// 一个 Agent 任务（[`HubStatus::tasks`]）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskStatus {
    /// Hub 签发的任务 ID（`task-<128 位十六进制>`）。
    pub id: String,
    /// 调用方键：`mcp:<n>` / `principal:<主体>` / `api` / `api:<session>`。
    pub caller: String,
    pub kind: crate::task::CallerKind,
    /// 发起方 Agent 名（第 16 项 N5）：Agent 主体及其任务句柄、以 Agent 令牌建立的 legacy 会话；本机用户与 Hub API 为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// 未过期的 `apps.select` 选择，按 appId 排序。
    pub selections: Vec<TaskSelectionStatus>,
    /// 本任务发出、尚未到期且实例仍连接的租约，按连接 ID 排序。
    pub leases: Vec<TaskLeaseStatus>,
    /// 进行中的请求数。
    pub inflight: u32,
    /// 距最近一次请求活动（开始或结束）的毫秒数；没有活动记录时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
}

/// [`AgentTaskStatus::selections`] 的一项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSelectionStatus {
    pub app_id: String,
    pub instance_id: String,
    /// 距失效的毫秒数（主体级选择，`HubConfig.principal_select_ttl`）；不单独过期时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_ms: Option<u64>,
}

/// [`AgentTaskStatus::leases`] 的一项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLeaseStatus {
    /// 实例的连接 ID（与 [`InstanceInfo::connection_id`] 相同）。
    pub connection_id: String,
    /// 距到期的毫秒数。
    pub expires_in_ms: u64,
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
