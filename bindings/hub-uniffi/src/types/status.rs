//! 运行状态（spec/hub-api.md 3.9；与 `GET /status` 的 JSON 同构）。

use app_mcp_hub as hub;

use super::{AppKind, InstanceInfo, LimitsConfig, OutputValidation, PolicyStatus, Risk, ToolAnnotations};

/// App 的整体状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AppState {
    /// 至少一个实例在线（上游：子进程已连接）。
    Connected,
    /// 正在唤醒。
    Waking,
    /// 没有在线实例，但有休眠实例。
    Dormant,
    /// 未连接（只有静态清单、握手被拒，或上游未连接）。
    Disconnected,
}

impl From<hub::AppState> for AppState {
    fn from(v: hub::AppState) -> Self {
        match v {
            hub::AppState::Connected => AppState::Connected,
            hub::AppState::Waking => AppState::Waking,
            hub::AppState::Dormant => AppState::Dormant,
            hub::AppState::Disconnected => AppState::Disconnected,
        }
    }
}

/// 实例状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum InstanceState {
    Connected,
    Dormant,
    /// 休眠实例正在被唤醒。
    Waking,
}

impl From<hub::InstanceState> for InstanceState {
    fn from(v: hub::InstanceState) -> Self {
        match v {
            hub::InstanceState::Connected => InstanceState::Connected,
            hub::InstanceState::Dormant => InstanceState::Dormant,
            hub::InstanceState::Waking => InstanceState::Waking,
        }
    }
}

/// 主 HTTP 服务的令牌策略。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AuthStatus {
    /// 是否配置了访问令牌。
    pub token_configured: bool,
    /// 不带 `Origin` 的本地客户端是否也必须携带令牌（`--auth all`）。
    pub token_required_without_origin: bool,
}

/// 最近一次错误（握手 / 配对被拒、唤醒失败 / 超时；上游为进程错误）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LastError {
    /// 连接级错误码（spec/protocol.md 10.1）或工具错误类别（如 `APP_NOT_RESPONDING`）；未知时为空。
    pub code: Option<String>,
    pub message: String,
    /// 发生时刻（Unix 毫秒）；上游错误为 0。
    pub at_ms: u64,
}

/// 实例及其状态。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct InstanceStatus {
    pub info: InstanceInfo,
    pub state: InstanceState,
    /// 功耗观测（spec/lifecycle.md 第 12 节）；Hub 尚无该实例的计数时为空。
    #[uniffi(default = None)]
    pub power: Option<InstancePower>,
}

/// SDK 声明的生命周期模式（`app/hello.lifecycleMode`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LifecycleMode {
    Persistent,
    Idle,
    OnDemand,
}

impl From<hub::LifecycleMode> for LifecycleMode {
    fn from(v: hub::LifecycleMode) -> Self {
        match v {
            hub::LifecycleMode::Persistent => LifecycleMode::Persistent,
            hub::LifecycleMode::Idle => LifecycleMode::Idle,
            hub::LifecycleMode::OnDemand => LifecycleMode::OnDemand,
        }
    }
}

/// 已连接实例当前不能休眠的原因中 Hub 可见的部分（App 的 `hold()` 只有 SDK 知道）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AwakeReason {
    /// SDK 声明为 `persistent` 模式。
    Persistent,
    /// 有进行中的调用 / 资源读取。
    Call,
    /// 有未到期的租约。
    Lease,
    /// Host 订阅了该实例声明 `realtime` 的资源。
    Subscription,
    /// 有待派发给该实例的唤醒。
    WakePending,
}

impl From<hub::AwakeReason> for AwakeReason {
    fn from(v: hub::AwakeReason) -> Self {
        match v {
            hub::AwakeReason::Persistent => AwakeReason::Persistent,
            hub::AwakeReason::Call => AwakeReason::Call,
            hub::AwakeReason::Lease => AwakeReason::Lease,
            hub::AwakeReason::Subscription => AwakeReason::Subscription,
            hub::AwakeReason::WakePending => AwakeReason::WakePending,
        }
    }
}

/// 每实例功耗观测（跨重连与休眠保留，Hub 重启清零）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct InstancePower {
    /// 回连次数：Hub 启动以来该实例完成握手的次数减 1。
    pub reconnects: u64,
    /// 以该休眠实例为目标实际发出的唤醒激活次数。
    pub wakes: u64,
    /// 累计在线秒数（含当前连接）。
    pub online_secs: u64,
    /// Hub 发出的 `ping` 与收到 SDK 的 `ping` 之和。
    pub heartbeats: u64,
    /// SDK 声明的心跳间隔：`0` = 不发心跳（本地传输）；为空 = 旧 SDK（双向心跳）。
    pub heartbeat_ms: Option<u64>,
    /// SDK 声明的生命周期模式；为空 = 未声明。
    pub lifecycle_mode: Option<LifecycleMode>,
    /// 已连接实例当前不能休眠的原因；休眠实例与可以休眠时为空。
    pub awake_reasons: Vec<AwakeReason>,
}

impl From<hub::InstancePower> for InstancePower {
    fn from(p: hub::InstancePower) -> Self {
        InstancePower {
            reconnects: p.reconnects,
            wakes: p.wakes,
            online_secs: p.online_secs,
            heartbeats: p.heartbeats,
            heartbeat_ms: p.heartbeat_ms,
            lifecycle_mode: p.lifecycle_mode.map(Into::into),
            awake_reasons: p.awake_reasons.into_iter().map(Into::into).collect(),
        }
    }
}

/// 租约策略与统计（spec/lifecycle.md 第 13 节 B2）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LeaseStatus {
    /// `adaptive` / `fixed`（固定 `lease_ttl_ms`）/ `off`（`lease_ttl_ms = 0`）。
    pub mode: String,
    /// 默认（无历史 / 固定）租约毫秒数。
    pub default_ms: u64,
    pub min_ms: u64,
    pub max_ms: u64,
    pub margin_ms: u64,
    pub window: u32,
    /// 请求流空闲收回阈值；0 = 不因空闲收回。
    pub idle_revoke_ms: u64,
    pub adaptive_grants: u64,
    pub default_grants: u64,
    pub revoked_session_end: u64,
    pub revoked_idle: u64,
    /// 当前跟踪的（会话, App）。
    pub pairs: Vec<LeasePairStatus>,
}

/// 一个（会话, App）的租约统计。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LeasePairStatus {
    /// 会话键（MCP `mcp:<n>`，API `api` / `api:<session>`）。
    pub session: String,
    pub app_id: String,
    /// 窗口内的间隔样本数。
    pub samples: u32,
    /// 下一次调用完成后将发出的租约毫秒数。
    pub next_ttl_ms: u64,
    /// `next_ttl_ms` 是否来自统计。
    pub adaptive: bool,
}

impl From<hub::LeaseStatus> for LeaseStatus {
    fn from(l: hub::LeaseStatus) -> Self {
        LeaseStatus {
            mode: l.mode,
            default_ms: l.default_ms,
            min_ms: l.min_ms,
            max_ms: l.max_ms,
            margin_ms: l.margin_ms,
            window: l.window,
            idle_revoke_ms: l.idle_revoke_ms,
            adaptive_grants: l.adaptive_grants,
            default_grants: l.default_grants,
            revoked_session_end: l.revoked_session_end,
            revoked_idle: l.revoked_idle,
            pairs: l
                .pairs
                .into_iter()
                .map(|p| LeasePairStatus {
                    session: p.session,
                    app_id: p.app_id,
                    samples: p.samples,
                    next_ttl_ms: p.next_ttl_ms,
                    adaptive: p.adaptive,
                })
                .collect(),
        }
    }
}

/// 一个 App（或上游）的运行状态。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppStatus {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    pub state: AppState,
    /// 在线实例在前，其后为休眠实例；上游为空。
    pub instances: Vec<InstanceStatus>,
    pub last_error: Option<LastError>,
    /// Hub 启动以来为该 App 实际发出的唤醒激活次数（含冷启动；上游为 0）。
    #[uniffi(default = 0)]
    pub wakes: u64,
    /// Hub 启动以来因限流被拒绝的调用次数。
    #[uniffi(default = 0)]
    pub rate_limited: u64,
    /// Hub 启动以来因大小上限被拒绝的参数 / 结果 / 资源次数。
    #[uniffi(default = 0)]
    pub too_large: u64,
    /// 各工具的声明（`risk` 与 MCP 注解）；上游与旧 Host 为空。
    #[uniffi(default = [])]
    pub tools: Vec<ToolDeclaration>,
}

/// 一个工具的声明（[`AppStatus::tools`]）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ToolDeclaration {
    /// 局部名（不含 appId）。
    pub name: String,
    /// 旧写法 `risk`（未声明时为 `Write`）。
    pub risk: Risk,
    /// App 声明的注解（原样）；为空 = 未声明。
    pub annotations: Option<ToolAnnotations>,
    /// Agent 实际看到的注解（声明优先，缺少的按 `risk` 推导）。
    pub effective: ToolAnnotations,
    /// 是否声明了 `outputSchema`。
    pub output_schema: bool,
}

impl From<hub::ToolDeclaration> for ToolDeclaration {
    fn from(d: hub::ToolDeclaration) -> Self {
        ToolDeclaration {
            name: d.name,
            risk: d.risk.into(),
            annotations: d.annotations.map(Into::into),
            effective: d.effective.into(),
            output_schema: d.output_schema,
        }
    }
}

/// 一条 SDK 诊断上报（`app/diagnostic`，spec/protocol.md 10.2）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
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

/// `AppMcpHub::status()` 的结果（spec/hub-api.md 3.9）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HubStatus {
    /// 固定为 `app-mcp`。
    pub service: String,
    pub version: String,
    /// 进程的操作系统用户；取不到时为空。
    pub user: Option<String>,
    pub pid: u32,
    /// HTTP 服务实际监听的地址；未开启时为空。
    pub listen: Option<String>,
    /// 本地 IPC 端点；未开启时为空。
    pub ipc_endpoint: Option<String>,
    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
    /// 是否提供 MCP Streamable HTTP（`/mcp`）。
    pub mcp_http: bool,
    pub auth: AuthStatus,
    /// 已初始化的 legacy MCP 会话数。
    pub mcp_sessions: u64,
    /// 进行中的 `subscriptions/listen` 流数（spec/hub-api.md 3.6「通知」）；旧 Host 为空。
    #[uniffi(default = None)]
    pub mcp_listen_streams: Option<u64>,
    /// App（含上游），按 appId 排序。
    pub apps: Vec<AppStatus>,
    /// 最近的 SDK 诊断上报，旧的在前（最多 32 条）。
    pub reports: Vec<DiagnosticReport>,
    /// 租约策略与统计。
    #[uniffi(default = None)]
    pub lease: Option<LeaseStatus>,
    /// 资源保护策略（全部字段给出）；旧 Host 为空。
    #[uniffi(default = None)]
    pub limits: Option<LimitsConfig>,
    /// 结果与 `outputSchema` 不符时的处理；旧 Host 为空。
    #[uniffi(default = None)]
    pub output_validation: Option<OutputValidation>,
    /// 策略规则、命中次数与最近的加载错误；旧 Host 为空。
    #[uniffi(default = None)]
    pub policy: Option<PolicyStatus>,
    /// 休眠记录持久化状态；未配置 `state_dir` 或旧 Host 时为空。
    #[uniffi(default = None)]
    pub dormant_store: Option<DormantStoreStatus>,
    /// Agent 任务（调用方的跨请求状态，spec/hub-api.md 3.6），按调用方键排序；旧 Host 为空。
    #[uniffi(default = None)]
    pub tasks: Option<Vec<AgentTaskStatus>>,
    /// 已登记的 Agent 名（第 16 项 N5，不含令牌）；旧 Host 为空。
    #[uniffi(default = None)]
    pub agents: Option<Vec<String>>,
    /// 按调用方记账（第 16 项 P3），按主体排序；旧 Host 为空。
    #[uniffi(default = None)]
    pub usage: Option<Vec<super::UsageStatus>>,
    /// 未到期的对象锁（第 16 项 N6），按 appId、key 排序；旧 Host 为空。
    #[uniffi(default = None)]
    pub locks: Option<Vec<super::LockStatus>>,
    /// 进行中的调用对象（第 16 项 P5），按开始时刻排序；旧 Host 为空。
    #[uniffi(default = None)]
    pub calls: Option<Vec<super::CallStatus>>,
    /// 事件订阅与丢弃统计（第 16 项 N3，spec/hub-api.md 3.17）；旧 Host 为空。
    #[uniffi(default = None)]
    pub events: Option<super::EventsStatus>,
    /// 标准意图的机主默认表与最近的替换错误（spec/intents.md 第 4 节）；旧 Host 为空。
    #[uniffi(default = None)]
    pub intents: Option<super::IntentsStatus>,
    /// 只读结果缓存的条目与命中统计（第 16 项 O3，spec/hub-api.md 3.20）；旧 Host 为空。
    #[uniffi(default = None)]
    pub cache: Option<super::CacheStatus>,
}

/// 调用方的种类（spec/hub-api.md 3.6）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CallerKind {
    /// legacy MCP 会话（`mcp:<n>`）。
    McpSession,
    /// 无会话 MCP 请求的主体（`principal:<主体>`）。
    Principal,
    /// Hub API 会话（`api` / `api:<session>`）。
    Api,
}

impl From<hub::CallerKind> for CallerKind {
    fn from(k: hub::CallerKind) -> Self {
        match k {
            hub::CallerKind::McpSession => CallerKind::McpSession,
            hub::CallerKind::Principal => CallerKind::Principal,
            hub::CallerKind::Api => CallerKind::Api,
        }
    }
}

/// 一个 Agent 任务（`HubStatus.tasks`）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AgentTaskStatus {
    /// Hub 签发的任务 ID（`task-<128 位十六进制>`）。
    pub id: String,
    /// 调用方键：`mcp:<n>` / `principal:<主体>` / `api` / `api:<session>`。
    pub caller: String,
    pub kind: CallerKind,
    /// 未过期的 `apps.select` 选择，按 appId 排序。
    pub selections: Vec<TaskSelectionStatus>,
    /// 本任务发出、尚未到期且实例仍连接的租约，按连接 ID 排序。
    pub leases: Vec<TaskLeaseStatus>,
    /// 进行中的请求数。
    pub inflight: u32,
    /// 距最近一次请求活动的毫秒数；没有活动记录时为空。
    #[uniffi(default = None)]
    pub idle_ms: Option<u64>,
    /// 发起方 Agent 名（第 16 项 N5）；本机主体与 Hub API 为空。
    #[uniffi(default = None)]
    pub agent: Option<String>,
}

/// `AgentTaskStatus.selections` 的一项。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskSelectionStatus {
    pub app_id: String,
    pub instance_id: String,
    /// 距失效的毫秒数（主体级选择，`HubConfig.principal_select_ttl_ms`）；不单独过期时为空。
    #[uniffi(default = None)]
    pub expires_in_ms: Option<u64>,
}

/// `AgentTaskStatus.leases` 的一项。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskLeaseStatus {
    /// 实例的连接 ID（与 `InstanceInfo.connection_id` 相同）。
    pub connection_id: String,
    /// 距到期的毫秒数。
    pub expires_in_ms: u64,
}

impl From<hub::AgentTaskStatus> for AgentTaskStatus {
    fn from(t: hub::AgentTaskStatus) -> Self {
        AgentTaskStatus {
            id: t.id,
            caller: t.caller,
            kind: t.kind.into(),
            selections: t
                .selections
                .into_iter()
                .map(|s| TaskSelectionStatus { app_id: s.app_id, instance_id: s.instance_id, expires_in_ms: s.expires_in_ms })
                .collect(),
            leases: t
                .leases
                .into_iter()
                .map(|l| TaskLeaseStatus { connection_id: l.connection_id, expires_in_ms: l.expires_in_ms })
                .collect(),
            inflight: t.inflight,
            idle_ms: t.idle_ms,
            agent: t.agent,
        }
    }
}

/// 休眠记录持久化状态（`HubStatus.dormant_store`，配置了 `HubConfig.state_dir` 时）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DormantStoreStatus {
    /// 休眠记录目录（`<state_dir>/dormant`）。
    pub dir: String,
    /// 启动时读回的实例数。
    pub loaded_instances: u64,
    /// 启动时因过期丢弃的实例数。
    pub expired_instances: u64,
    /// 启动以来成功写入 / 删除文件的次数。
    pub writes: u64,
    /// 启动时跳过的文件（损坏、版本未知、超出上限）。
    pub issues: Vec<StoreIssue>,
    /// 最近一次写入失败。
    #[uniffi(default = None)]
    pub last_error: Option<String>,
}

/// 读取 / 写入中被跳过的文件或失败。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StoreIssue {
    /// 文件名（相对于休眠记录目录）。
    pub file: String,
    /// 中文说明。
    pub reason: String,
}

impl From<hub::DormantStoreStatus> for DormantStoreStatus {
    fn from(s: hub::DormantStoreStatus) -> Self {
        DormantStoreStatus {
            dir: s.dir,
            loaded_instances: s.loaded_instances,
            expired_instances: s.expired_instances,
            writes: s.writes,
            issues: s.issues.into_iter().map(|i| StoreIssue { file: i.file, reason: i.reason }).collect(),
            last_error: s.last_error,
        }
    }
}

impl From<hub::LastError> for LastError {
    fn from(e: hub::LastError) -> Self {
        LastError {
            code: e.code,
            message: e.message,
            at_ms: e.at_ms,
        }
    }
}

impl From<hub::InstanceStatus> for InstanceStatus {
    fn from(i: hub::InstanceStatus) -> Self {
        InstanceStatus {
            info: i.info.into(),
            state: i.state.into(),
            power: i.power.map(Into::into),
        }
    }
}

impl From<hub::AppStatus> for AppStatus {
    fn from(a: hub::AppStatus) -> Self {
        AppStatus {
            app_id: a.app_id,
            name: a.name,
            kind: a.kind.into(),
            state: a.state.into(),
            instances: a.instances.into_iter().map(Into::into).collect(),
            last_error: a.last_error.map(Into::into),
            wakes: a.wakes,
            rate_limited: a.rate_limited,
            too_large: a.too_large,
            tools: a.tools.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<hub::DiagnosticReport> for DiagnosticReport {
    fn from(r: hub::DiagnosticReport) -> Self {
        DiagnosticReport {
            app_id: r.app_id,
            instance_id: r.instance_id,
            connection_id: r.connection_id,
            code: r.code,
            message: r.message,
            count: r.count,
            received_at_ms: r.received_at_ms,
        }
    }
}

impl From<hub::HubStatus> for HubStatus {
    fn from(s: hub::HubStatus) -> Self {
        HubStatus {
            service: s.identity.service,
            version: s.identity.version,
            user: s.identity.user,
            pid: s.identity.pid,
            listen: s.listen,
            ipc_endpoint: s.ipc_endpoint,
            started_at_ms: s.started_at_ms,
            mcp_http: s.mcp_http,
            auth: AuthStatus {
                token_configured: s.auth.token_configured,
                token_required_without_origin: s.auth.token_required_without_origin,
            },
            mcp_sessions: u64::try_from(s.mcp_sessions).unwrap_or(u64::MAX),
            mcp_listen_streams: s.mcp_listen_streams.map(|n| u64::try_from(n).unwrap_or(u64::MAX)),
            apps: s.apps.into_iter().map(Into::into).collect(),
            reports: s.reports.into_iter().map(Into::into).collect(),
            lease: s.lease.map(Into::into),
            limits: s.limits.map(Into::into),
            output_validation: s.output_validation.map(Into::into),
            policy: s.policy.map(Into::into),
            dormant_store: s.dormant_store.map(Into::into),
            tasks: s.tasks.map(|t| t.into_iter().map(Into::into).collect()),
            agents: s.agents,
            usage: s.usage.map(|u| u.into_iter().map(Into::into).collect()),
            locks: s.locks.map(|l| l.into_iter().map(Into::into).collect()),
            calls: s.calls.map(|c| c.into_iter().map(Into::into).collect()),
            events: s.events.map(Into::into),
            intents: s.intents.map(Into::into),
            cache: s.cache.map(Into::into),
        }
    }
}
