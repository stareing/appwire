//! Hub 主体：共享状态、公开 API（spec/hub-api.md 第 3 节）、列表变化通知、资源订阅、事件。
//!
//! 工具调用的唯一实现见 [`crate::call`]；MCP 出口（[`McpSession`](crate::McpSession)）、[`Hub::call_tool`]、
//! [`Hub::dispatch`] 都经由它。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use app_mcp_protocol::identity::HostIdentity;
use app_mcp_protocol::registry::EndpointRegistry;
use app_mcp_protocol::{ErrorKind, ToolError};
use serde_json::json;
use tokio::sync::{Notify, broadcast};

use crate::connection::RequestError;
use crate::instance::Instance;
use crate::lease::LeaseBook;
use crate::limits::RateBook;
use crate::origin::OriginPolicy;
use crate::policy::PolicyState;
use crate::overview::Overview;
use crate::tool_def::StaticManifest;
use crate::registry::Registry;
use crate::subscribers::SubscriberTable;
use crate::task::TaskTable;
use crate::types::{AppOverviewInfo, ApprovalHandler, DiagnosticReport, HubEvent, LastError, PairingHandler};
use crate::upstream::UpstreamState;
use crate::wake::Waker;

mod accounting;
mod api;
mod catalog;
mod config;
mod exposure;
mod manifests;
mod notify;
mod policy_hooks;
mod resources;
mod start;
mod status;
mod tasks;
#[cfg(test)]
mod tests;
mod upstreams;

pub use config::{
    DEFAULT_CHANNEL_GRACE, DEFAULT_MAX_LISTEN_RESOURCES, DEFAULT_MAX_LISTEN_STREAMS, DEFAULT_MAX_LOCKS, DEFAULT_MAX_TASK_HANDLES,
    DEFAULT_NAVIGATE_TIMEOUT, DEFAULT_PRINCIPAL_SELECT_TTL, DEFAULT_PROGRESS_INTERVAL, DEFAULT_STATELESS_LIST_TTL,
    DEFAULT_TASK_IDLE_TTL, DEFAULT_TOOL_EXPOSURE_THRESHOLD, DEFAULT_WAKE_RATE_LIMIT, HubConfig,
};
pub use manifests::load_manifests;

/// 资源 URI 前缀：`app-mcp://<appId>/<resourceName>`。
pub const RESOURCE_URI_SCHEME: &str = "app-mcp://";

/// 默认资源 MIME 类型。
pub(crate) const DEFAULT_MIME: &str = "application/json";

/// Hub API 自身（[`Hub::subscribe`]）使用的订阅会话 ID；MCP 会话 ID 从 1 开始。
pub(crate) const API_SUBSCRIBER: u64 = 0;

/// 事件通道容量；接收方落后超过此数时会收到 `Lagged`。
const EVENT_CAPACITY: usize = 256;

/// [`HubStatus::reports`](crate::types::HubStatus::reports) 保留的 SDK 诊断上报条数。
pub const MAX_REPORTS: usize = 32;

/// 诊断记录（`/status`）：每个 App 最近的错误与最近的 SDK 上报。
#[derive(Debug, Default)]
struct Diagnostics {
    last_errors: HashMap<String, LastError>,
    reports: VecDeque<DiagnosticReport>,
}

pub fn resource_uri(app_id: &str, name: &str) -> String {
    format!("{RESOURCE_URI_SCHEME}{app_id}/{name}")
}

/// 解析资源 URI，返回 `(appId, resourceName)`。
pub fn parse_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix(RESOURCE_URI_SCHEME)?;
    let (app, name) = rest.split_once('/')?;
    (!app.is_empty() && !name.is_empty()).then_some((app, name))
}

/// Hub 各部分共享的状态。
pub struct HubShared {
    pub(crate) config: HubConfig,
    /// 本进程的 Host 身份（握手结果、`/healthz`、登记文件）。
    pub(crate) identity: HostIdentity,
    /// 本次启动的随机标记（6 位十六进制），连接 ID 的前缀（spec/protocol.md 10.3）。
    run_tag: String,
    /// 实际监听位置与启动时刻（[`Hub::start`] 绑定完成后设置），供 `/status` 使用。
    endpoints: OnceLock<EndpointRegistry>,
    diagnostics: Mutex<Diagnostics>,
    pub(crate) origins: OriginPolicy,
    registry: Mutex<Registry>,
    /// 通知订阅方：已完成初始化的 legacy MCP 会话与 `subscriptions/listen` 流（[`crate::subscribers`]）。
    subscribers: Mutex<SubscriberTable>,
    /// 资源订阅：URI → 订阅方 ID（[`Self::subscribers`] 的 ID；[`API_SUBSCRIBER`] 为 Hub API）。
    resource_subs: Mutex<HashMap<String, HashSet<u64>>>,
    /// Hub 停止中（[`Hub::shutdown`] / Drop 置位）：listen 流据此正常结束（发最终结果）。
    closing: tokio::sync::watch::Sender<bool>,
    tools_dirty: AtomicBool,
    resources_dirty: AtomicBool,
    dirty: Notify,
    next_id: AtomicU64,
    /// 上游 MCP 服务器状态。
    upstreams: Mutex<BTreeMap<String, UpstreamState>>,
    events: broadcast::Sender<HubEvent>,
    /// 调用方键 → Agent 任务（调用方的跨请求状态，[`crate::task`]）。
    agent_tasks: Mutex<TaskTable>,
    /// [`Hub::select_instance`] 的选择（所有调用方共用，调用方自己的 `apps.select` 优先）。
    global_selected: Mutex<HashMap<String, String>>,
    /// 进行中的调用对象：callId → 登记（[`crate::call_objects`]）。
    pub(crate) calls: Mutex<HashMap<String, crate::call_objects::CallEntry>>,
    /// 等待进度的调用：callId → 进度路由（只接受被路由到的那条 App 连接发来的 `tools/progress`）。
    pub(crate) progress_routes: Mutex<HashMap<String, ProgressRoute>>,
    approval_handler: Mutex<Option<Arc<dyn ApprovalHandler>>>,
    pairing_handler: Mutex<Option<Arc<dyn PairingHandler>>>,
    /// 配对过的 token → appId（设置了 PairingHandler 时用于免询问重连）。
    paired_tokens: Mutex<HashMap<String, String>>,
    /// 经 PairingHandler 同意的 (appId, Origin)。
    paired_origins: Mutex<HashSet<(String, Option<String>)>>,
    /// 导出名 → 全名（历次导出的并集，后导出的覆盖）。
    export_names: Mutex<HashMap<String, String>>,
    /// 当前唤醒实现（由 `HubConfig::waker` 构造，可被 [`Hub::set_waker`] 替换）；`None` = 不唤醒。
    pub(crate) waker: Mutex<Option<Arc<dyn Waker>>>,
    /// 进行中的唤醒。
    pub(crate) wakes: Mutex<Vec<crate::lifecycle::PendingWake>>,
    /// 功耗观测与唤醒速率（spec/lifecycle.md 第 12 节）。
    pub(crate) power: Mutex<crate::power::PowerBook>,
    /// 自适应租约统计与会话请求活动（spec/lifecycle.md 第 13 节 B2）。
    pub(crate) leases: Mutex<LeaseBook>,
    /// 休眠记录持久化（[`HubConfig::state_dir`]）；未配置时为 `None`。
    pub(crate) persist: Option<crate::lifecycle::Persist>,
    /// 请求活动 / 默认租约变化时唤醒空闲收回任务。
    pub(crate) lease_changed: Notify,
    /// 调用限流状态与每 App 的拒绝计数（spec/hub-api.md 3.11）。
    pub(crate) rates: Mutex<RateBook>,
    /// 按调用方记账（第 16 项 P3，[`crate::usage`]）。
    pub(crate) usage: Mutex<crate::usage::UsageBook>,
    /// 生效的策略规则、命中计数与最近的加载错误（spec/hub-api.md 3.13）。
    pub(crate) policy: Mutex<PolicyState>,
    /// 已登记 Agent 的令牌（[`HubConfig::agents`]，运行中由 [`Hub::set_agents`] 整体替换）。
    pub(crate) agents: Mutex<crate::agents::AgentRegistry>,
    /// 工具注册的变化序号（每次 [`Self::mark_tools_changed`] 加一）；导航后等待目标工具注册时订阅（spec/hub-api.md 3.14）。
    pub(crate) tools_rev: tokio::sync::watch::Sender<u64>,
    /// 接受闸门与连接计数（按需启动的空闲退出，[`crate::activity`]）。
    pub(crate) activity: crate::activity::Activity,
    /// App 事件：目录、订阅与信箱、厂商回调（第 16 项 N3 + P4，[`crate::events`]）。
    pub(crate) app_events: crate::events::AppEvents,
    /// 按（记账主体, 工具全名）的使用统计（第 16 项 O1，`apps.search` 的排序加成，[`crate::search`]）。
    pub(crate) search_stats: Mutex<crate::search::SearchStats>,
    /// 标准意图的机主默认表与最近的替换错误（[`HubConfig::intent_defaults`]，运行中由 [`Hub::set_intent_defaults`] 替换）。
    pub(crate) intents: Mutex<crate::intents::defaults::IntentsState>,
    /// 只读结果缓存（第 16 项 O3，[`crate::result_cache`]）。
    pub(crate) result_cache: Mutex<crate::result_cache::ResultCache<crate::result_cache::CachedValue>>,
}

/// 一次调用的进度路由（[`HubShared::progress_routes`]）。
#[derive(Debug)]
pub(crate) struct ProgressRoute {
    /// 登记序号：同名 callId 被新调用覆盖后，旧调用结束时不删除新登记。
    pub token: u64,
    /// 调用被路由到的 App 连接。
    pub conn_id: u64,
    pub tx: tokio::sync::mpsc::UnboundedSender<app_mcp_protocol::ToolsProgressParams>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl HubShared {
    fn new(mut config: HubConfig, waker: Option<Arc<dyn Waker>>) -> Self {
        let mut registry = Registry::new();
        // @invariant 清单只在注册表中保存一份：移出配置，`config.manifests` 之后为空（启动后不再读取）。
        let manifests = std::mem::take(&mut config.manifests);
        if !manifests.is_empty() {
            // 先全部复制为紧凑形式、再一起释放解析形式，然后把空闲内存还给操作系统（见 StaticManifest::copy_of）。
            let compact: Vec<StaticManifest> = manifests.iter().map(StaticManifest::copy_of).collect();
            drop(manifests);
            for m in compact {
                let app_id = m.meta().app_id.clone();
                if registry.set_static_manifest(m) {
                    tracing::warn!(%app_id, "同一 appId 的清单出现多次，后加载的覆盖先加载的");
                }
            }
            crate::heap::release_free_memory();
        }
        let mut upstreams = BTreeMap::new();
        for (name, up) in &config.upstreams {
            if !app_mcp_protocol::is_valid_app_id(name)
                || app_mcp_manifest::is_reserved_app_id(name)
            {
                tracing::error!(upstream = %name, "上游名称不合法（须满足 [a-z][a-z0-9-]{{0,62}} 且不是保留名），已跳过");
            } else if registry.has_app(name) {
                tracing::error!(upstream = %name, "上游名称与静态清单的 appId 冲突，已跳过");
            } else {
                upstreams.insert(name.clone(), UpstreamState::new(up.clone()));
            }
        }
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let policy = PolicyState::new(config.policy.clone(), unix_millis());
        let agents = crate::agents::AgentRegistry::new(&config.agents);
        let intents = crate::intents::defaults::IntentsState::new(&config.intent_defaults);
        let result_cache = crate::result_cache::ResultCache::new(config.result_cache);
        let persist = config.state_dir.as_deref().map(crate::lifecycle::Persist::new);
        let app_events = crate::events::AppEvents::new(config.state_dir.as_deref());
        Self {
            app_events,
            identity: HostIdentity::current(env!("CARGO_PKG_VERSION")),
            run_tag: format!("{:06x}", rand::random::<u32>() & 0x00ff_ffff),
            endpoints: OnceLock::new(),
            diagnostics: Mutex::new(Diagnostics::default()),
            upstreams: Mutex::new(upstreams),
            origins: OriginPolicy::new(config.allow_origins.iter().cloned()),
            config,
            registry: Mutex::new(registry),
            subscribers: Mutex::new(SubscriberTable::default()),
            resource_subs: Mutex::new(HashMap::new()),
            closing: tokio::sync::watch::Sender::new(false),
            tools_dirty: AtomicBool::new(false),
            resources_dirty: AtomicBool::new(false),
            dirty: Notify::new(),
            next_id: AtomicU64::new(1),
            events,
            agent_tasks: Mutex::new(TaskTable::default()),
            global_selected: Mutex::new(HashMap::new()),
            calls: Mutex::new(HashMap::new()),
            progress_routes: Mutex::new(HashMap::new()),
            approval_handler: Mutex::new(None),
            pairing_handler: Mutex::new(None),
            paired_tokens: Mutex::new(HashMap::new()),
            paired_origins: Mutex::new(HashSet::new()),
            export_names: Mutex::new(HashMap::new()),
            waker: Mutex::new(waker),
            wakes: Mutex::new(Vec::new()),
            power: Mutex::new(crate::power::PowerBook::default()),
            leases: Mutex::new(LeaseBook::default()),
            persist,
            lease_changed: Notify::new(),
            rates: Mutex::new(RateBook::default()),
            usage: Mutex::new(crate::usage::UsageBook::default()),
            policy: Mutex::new(policy),
            agents: Mutex::new(agents),
            tools_rev: tokio::sync::watch::Sender::new(0),
            activity: crate::activity::Activity::default(),
            search_stats: Mutex::new(crate::search::SearchStats::default()),
            intents: Mutex::new(intents),
            result_cache: Mutex::new(result_cache),
        }
    }

    /// 测试用：不启动监听与后台任务的共享状态。
    #[cfg(test)]
    pub(crate) fn new_for_test(config: HubConfig) -> Arc<Self> {
        Arc::new(Self::new(config, None))
    }

    /// 测试用：订阅 [`HubEvent`] 广播。
    #[cfg(test)]
    pub(crate) fn hub_events_for_test(&self) -> broadcast::Receiver<HubEvent> {
        self.events.subscribe()
    }

    pub(crate) fn registry(&self) -> MutexGuard<'_, Registry> {
        lock(&self.registry)
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 新的 App 连接（或多路复用通道），连接 ID 为 `<启动标记>-<序号>`。
    pub(crate) fn new_connection(
        &self,
    ) -> (Arc<crate::connection::Connection>, tokio::sync::mpsc::UnboundedReceiver<crate::connection::Outgoing>) {
        let id = self.next_id();
        crate::connection::Connection::new(id, format!("{}-{id}", self.run_tag))
    }
}

pub(crate) fn request_error(e: RequestError, app_id: &str) -> ToolError {
    match e {
        RequestError::Rpc(rpc) => rpc.to_tool_error(),
        RequestError::Timeout => {
            ToolError::new(ErrorKind::Timeout, "App 没有及时响应，请稍后重试。")
        }
        RequestError::Disconnected => ToolError::new(
            ErrorKind::AppDisconnected,
            format!("App「{app_id}」的实例在请求过程中断开了连接。"),
        )
        .with_details(json!({ "appId": app_id })),
    }
}

pub(crate) fn overview_info(ov: &Overview) -> AppOverviewInfo {
    AppOverviewInfo {
        app_id: ov.app_id.clone(),
        name: ov.app_name.clone(),
        summary: ov.summary.clone(),
        body: ov.body.clone(),
        locale: ov.locale.clone(),
        version: ov.version.clone(),
        source: ov.to_json()["source"].as_str().unwrap_or_default().to_owned(),
        text: ov.render(),
    }
}

/// 运行中的 Hub。
///
/// 所有方法都可在任意线程调用；`async` 方法与 [`Hub::start`] 必须在 tokio 运行时（多线程）中执行。
/// Drop 时中止后台任务（与 [`Hub::shutdown`] 相比不等待连接关闭）。
pub struct Hub {
    shared: Arc<HubShared>,
    listen_addr: Option<SocketAddr>,
    ipc_endpoint: Option<String>,
    started_at_ms: u64,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// 单实例锁与登记文件（[`HubConfig::run_dir`]）；丢弃时删除登记文件并释放锁。
    instance: Mutex<Option<Instance>>,
}

pub(crate) fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// 进程内 App 通道（M2 预留，尚无实现）。
#[derive(Debug)]
pub struct LocalAppChannel {
    _private: (),
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.shared.begin_closing();
        for t in lock(&self.tasks).iter() {
            t.abort();
        }
    }
}
