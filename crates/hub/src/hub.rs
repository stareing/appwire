//! Hub 主体：共享状态、公开 API（spec/hub-api.md 第 3 节）、列表变化通知、资源订阅、事件。
//!
//! 工具调用的唯一实现见 [`crate::call`]；MCP 出口（[`McpSession`]）、[`Hub::call_tool`]、
//! [`Hub::dispatch`] 都经由它。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::identity::HostIdentity;
use app_mcp_protocol::registry::EndpointRegistry;
use app_mcp_protocol::{
    DEFAULT_LISTEN_ADDR, ErrorKind, ResourceInfo, ResourceSubscribeParams, ResourcesReadParams,
    ResourcesReadResult, ToolError, method,
};
#[cfg(any(feature = "mcp-server", feature = "upstream"))]
use rmcp::model::Resource;
use rmcp::model::{ResourceUpdatedNotificationParam, Tool};
use rmcp::{Peer, RoleClient, RoleServer};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Notify, broadcast, oneshot};

use crate::call::{self, CallCtx};
use crate::connection::RequestError;
use crate::format::{self, NameCodec, ToolFormat};
use crate::http_server::{Health, HttpOptions, Router, Transport};
use crate::instance::Instance;
#[cfg(feature = "mcp-server")]
use crate::mcp::McpSession;
use crate::origin::OriginPolicy;
#[cfg(feature = "mcp-server")]
use crate::overview::AppSummary;
use crate::overview::{Overview, OverviewSource};
use crate::registry::Registry;
use crate::types::{
    AppInfo, AppKind, AppOverviewInfo, AppState, AppStatus, ApprovalHandler, ApprovalPolicy, AuthStatus,
    CallOutcome, CallRequest, DiagnosticReport, HubError, HubEvent, HubResource, HubStatus, HubTool,
    InstanceState, InstanceStatus, LastError, PairingHandler, ResourceContent, ToolExposure, ToolFilter,
};
use crate::upstream::{UpstreamConfig, UpstreamState, encode_uri_component};
use crate::wake::{Waker, WakerConfig};

/// 资源 URI 前缀：`app-mcp://<appId>/<resourceName>`。
pub const RESOURCE_URI_SCHEME: &str = "app-mcp://";

/// 默认资源 MIME 类型。
pub(crate) const DEFAULT_MIME: &str = "application/json";

/// Hub API 自身（[`Hub::subscribe`]）使用的订阅会话 ID；MCP 会话 ID 从 1 开始。
const API_SUBSCRIBER: u64 = 0;

/// 事件通道容量；接收方落后超过此数时会收到 `Lagged`。
const EVENT_CAPACITY: usize = 256;

/// [`HubStatus::reports`] 保留的 SDK 诊断上报条数。
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

/// Hub 配置。
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// HTTP 服务监听地址（spec/protocol.md 1.3）：同一端口承载 `/app`（App 的 WebSocket 连接）、
    /// `/healthz`，以及开启 [`HubConfig::mcp_http`] 时的 `/mcp`。默认 `127.0.0.1:7717`；端口为 0 时随机分配；
    /// `None` = 不开 TCP 服务（仅本地 IPC / 进程内 / 上游）。网页只能经这里连接；原生 App 默认走
    /// [`HubConfig::ipc_endpoint`]。非回环地址需要 [`HttpOptions::allow_remote`]。
    pub listen: Option<String>,
    /// `listen` 被占用（`AddrInUse`）时依次尝试的地址。默认 `127.0.0.1:7737`、`127.0.0.1:7757`
    /// （与网页 SDK 依次握手的端口一致，[`app_mcp_protocol::LISTEN_CANDIDATE_PORTS`]）；
    /// 显式指定 `listen` 时通常应清空，绑定不到指定地址即报错。
    pub listen_alternates: Vec<String>,
    /// HTTP 服务选项（令牌、是否允许远程）。令牌只作用于 `/mcp`。
    pub http: HttpOptions,
    /// 是否提供 MCP Streamable HTTP（`/mcp`）：同时作用于 `listen`（TCP，受 [`HubConfig::http`] 的令牌策略约束）
    /// 与 [`HubConfig::ipc_endpoint`]（本地 IPC，对端已确认是同一用户，不需要令牌）。默认 `false`
    /// （嵌入式 Hub 通常只需要 App 连接）；`app-mcp-host serve` 开启。
    pub mcp_http: bool,
    /// 单实例锁与登记文件所在目录（spec/protocol.md 1.5、1.7）：取得 `<run_dir>/hub.lock` 之后才开始监听，
    /// 绑定完成后写 `<run_dir>/endpoints.json`，停止时删除。已被锁定时 [`Hub::start`] 返回
    /// `ResourceBusy`。默认 `None`（嵌入式 Hub 不参与）；`app-mcp-host` 为 `<配置目录>/run`。
    pub run_dir: Option<PathBuf>,
    /// 本地 IPC 端点（spec/protocol.md 1.2）：`unix:<绝对路径>`（Linux / macOS）或
    /// `pipe:\\.\pipe\<名称>`（Windows）。默认为平台默认端点
    /// （[`app_mcp_protocol::endpoint::default_ipc_endpoint`]；Android / iOS 上为 `None`）。
    /// `None` = 不开 IPC 服务。已有 Hub 在该端点监听时 [`Hub::start`] 返回 `AddrInUse`。
    pub ipc_endpoint: Option<String>,
    /// 已加载并校验的静态清单（后面的覆盖前面的同 appId 清单）。
    pub manifests: Vec<Manifest>,
    /// 额外允许的 Origin 模式（默认已允许 localhost / 127.0.0.1 任意端口）。
    pub allow_origins: Vec<String>,
    /// 向 SDK 发送 `ping` 的间隔。只对未声明 `heartbeatMs` 的旧 SDK（或 [`HubConfig::legacy_heartbeat`]）发送
    /// （spec/lifecycle.md 第 11 节）。
    pub ping_interval: Duration,
    /// 多久没收到 SDK 的任何消息就断开。握手前一律适用；握手后：旧 SDK 照旧，声明 `heartbeatMs > 0` 的取
    /// `max(本值, 3 × heartbeatMs)`，声明 `heartbeatMs: 0`（本地传输，靠连接断开感知）的不做无消息断开。
    pub idle_timeout: Duration,
    /// 实例最近上报的可见性为 `hidden` / `frozen` 时使用的放宽超时（后台标签页定时器会被限流）。
    pub hidden_idle_timeout: Duration,
    /// `tools/invoke` 中的 `timeoutMs`（SDK 侧超时）。
    pub invoke_timeout: Duration,
    /// Hub 侧等待 SDK 响应的时间；超时后发送 `tools/cancel` 并返回 `TIMEOUT`。
    pub response_timeout: Duration,
    /// 列表变化通知的合并窗口。
    pub list_changed_debounce: Duration,
    /// 上游 MCP 服务器：名称 → 启动方式。名称须满足 appId 规则，且不能与清单 appId 或保留名冲突。
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    /// 调用审批策略（spec/hub-api.md 3.3）。默认不审批。
    pub approval: ApprovalPolicy,
    /// 等待 [`PairingHandler`] 的上限，超时视为拒绝。
    pub pairing_timeout: Duration,
    /// 每次调用某实例完成后发送的 `app/lease` 时长（spec/lifecycle.md 4.2）；`0` 关闭租约功能。
    pub lease_ttl: Duration,
    /// 唤醒后等待 App 回连的上限，超时返回 `APP_NOT_RESPONDING`。
    pub wake_timeout: Duration,
    /// 唤醒令牌的有效期。
    pub wake_token_ttl: Duration,
    /// 每个 App 每分钟最多实际发出的唤醒激活次数（spec/lifecycle.md 第 12 节）；`0` = 不限。默认
    /// [`DEFAULT_WAKE_RATE_LIMIT`]。超出时调用返回 `LAUNCH_FAILED`（`data.code = "WAKE_RATE_LIMITED"`）。
    pub wake_rate_limit: u32,
    /// 回退到 4e 之前的心跳：忽略 SDK 的 `heartbeatMs` 声明，对所有连接发 `ping` 并按无消息断开。默认 `false`。
    pub legacy_heartbeat: bool,
    /// 休眠实例记录的保留时长，过期后移除（工具不再列出）。
    pub dormant_ttl: Duration,
    /// 同一 appId 以新的实例 ID 连接时，移除该 App 的全部休眠记录。默认 `true`。
    pub dormant_replaced_by_new_instance: bool,
    /// App 未运行、清单没有显式声明 `wake` 时，是否由清单 `launch` 推导唤醒方式并冷启动
    /// （`launch.web` 的地址会被打开）。默认 `false`：只返回 `APP_DISCONNECTED` 与启动提示。
    pub wake_from_launch: bool,
    /// 唤醒器：`System`（默认，按平台执行系统激活）/ `None`（不唤醒，返回 `APP_DISCONNECTED`）/
    /// `Exec`（执行指定程序）。[`Hub::set_waker`] 可再替换为自定义实现。
    pub waker: WakerConfig,
    /// 工具暴露方式（spec/hub-api.md 3.7）：`All` / `Progressive` / `Auto`（默认）。
    pub tool_exposure: ToolExposure,
    /// `Auto` 的阈值：App 与上游工具（不含内置工具）总数**超过**此值时按渐进暴露。默认 40。
    pub tool_exposure_threshold: usize,
}

/// [`HubConfig::tool_exposure_threshold`] 的默认值。
pub const DEFAULT_TOOL_EXPOSURE_THRESHOLD: usize = 40;

/// [`HubConfig::wake_rate_limit`] 的默认值（每 App 每分钟）。
///
/// @why 一次唤醒约 5.6 ms SDK 线程 / 约 19 ms 进程 CPU（TASKS.md 4e0 真机测量）。正常调用经唤醒去重与 60 秒租约
/// 合并为每分钟至多约 1 次；6 次给 `on-demand`（10 秒 grace）下的断续调用留余量，同时把唤醒 / 休眠循环
/// 限制在约 0.1 秒 CPU / 分钟。
pub const DEFAULT_WAKE_RATE_LIMIT: u32 = 6;

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            listen: Some(DEFAULT_LISTEN_ADDR.to_owned()),
            listen_alternates: app_mcp_protocol::LISTEN_CANDIDATE_PORTS[1..]
                .iter()
                .map(|p| format!("127.0.0.1:{p}"))
                .collect(),
            http: HttpOptions::default(),
            mcp_http: false,
            run_dir: None,
            ipc_endpoint: app_mcp_protocol::endpoint::default_ipc_endpoint().map(|e| e.to_string()),
            manifests: Vec::new(),
            allow_origins: Vec::new(),
            ping_interval: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(45),
            hidden_idle_timeout: Duration::from_secs(180),
            invoke_timeout: Duration::from_secs(30),
            response_timeout: Duration::from_secs(35),
            list_changed_debounce: Duration::from_millis(50),
            upstreams: BTreeMap::new(),
            approval: ApprovalPolicy::default(),
            pairing_timeout: Duration::from_secs(120),
            lease_ttl: Duration::from_secs(60),
            wake_timeout: Duration::from_secs(15),
            wake_token_ttl: Duration::from_secs(60),
            wake_rate_limit: DEFAULT_WAKE_RATE_LIMIT,
            legacy_heartbeat: false,
            dormant_ttl: Duration::from_secs(24 * 60 * 60),
            dormant_replaced_by_new_instance: true,
            wake_from_launch: false,
            waker: WakerConfig::System,
            tool_exposure: ToolExposure::Auto,
            tool_exposure_threshold: DEFAULT_TOOL_EXPOSURE_THRESHOLD,
        }
    }
}

/// 一个会话（MCP 会话或 API 的 `CallRequest.session`）的状态。
#[derive(Debug, Default)]
pub(crate) struct SessionState {
    /// `apps.select`：appId → instanceId。
    pub selected: HashMap<String, String>,
    /// 已附带的总览：appId → 版本。
    pub delivered: HashMap<String, String>,
    /// 本会话发出的租约：连接 ID → (连接, 到期时刻)。
    pub leases: HashMap<u64, (std::sync::Weak<crate::connection::Connection>, tokio::time::Instant)>,
    /// 渐进暴露：本会话展开过（`apps.tools`）或调用过的 App（含上游）。
    pub exposed: HashSet<String>,
}

/// API 调用的会话键。
pub(crate) fn api_session_key(session: Option<&str>) -> String {
    match session {
        Some(s) => format!("api:{s}"),
        None => "api".to_owned(),
    }
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
    /// 已完成初始化的 MCP 会话：会话 ID → peer。
    sessions: Mutex<HashMap<u64, Peer<RoleServer>>>,
    /// 资源订阅：URI → 订阅方（MCP 会话 ID；[`API_SUBSCRIBER`] 为 Hub API）。
    resource_subs: Mutex<HashMap<String, HashSet<u64>>>,
    tools_dirty: AtomicBool,
    resources_dirty: AtomicBool,
    dirty: Notify,
    next_id: AtomicU64,
    /// 上游 MCP 服务器状态。
    upstreams: Mutex<BTreeMap<String, UpstreamState>>,
    events: broadcast::Sender<HubEvent>,
    /// 会话键 → 会话状态。
    session_state: Mutex<HashMap<String, SessionState>>,
    /// [`Hub::select_instance`] 的选择（所有会话共用，会话自己的 `apps.select` 优先）。
    global_selected: Mutex<HashMap<String, String>>,
    /// 进行中的调用：callId → (登记序号, 取消信号)。
    pub(crate) calls: Mutex<HashMap<String, (u64, oneshot::Sender<()>)>>,
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
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl HubShared {
    fn new(config: HubConfig, waker: Option<Arc<dyn Waker>>) -> Self {
        let mut registry = Registry::new();
        for m in &config.manifests {
            if registry.set_manifest(m.clone()).is_some() {
                tracing::warn!(app_id = %m.app_id, "同一 appId 的清单出现多次，后加载的覆盖先加载的");
            }
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
        Self {
            identity: HostIdentity::current(env!("CARGO_PKG_VERSION")),
            run_tag: format!("{:06x}", rand::random::<u32>() & 0x00ff_ffff),
            endpoints: OnceLock::new(),
            diagnostics: Mutex::new(Diagnostics::default()),
            upstreams: Mutex::new(upstreams),
            origins: OriginPolicy::new(config.allow_origins.iter().cloned()),
            config,
            registry: Mutex::new(registry),
            sessions: Mutex::new(HashMap::new()),
            resource_subs: Mutex::new(HashMap::new()),
            tools_dirty: AtomicBool::new(false),
            resources_dirty: AtomicBool::new(false),
            dirty: Notify::new(),
            next_id: AtomicU64::new(1),
            events,
            session_state: Mutex::new(HashMap::new()),
            global_selected: Mutex::new(HashMap::new()),
            calls: Mutex::new(HashMap::new()),
            approval_handler: Mutex::new(None),
            pairing_handler: Mutex::new(None),
            paired_tokens: Mutex::new(HashMap::new()),
            paired_origins: Mutex::new(HashSet::new()),
            export_names: Mutex::new(HashMap::new()),
            waker: Mutex::new(waker),
            wakes: Mutex::new(Vec::new()),
            power: Mutex::new(crate::power::PowerBook::default()),
        }
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

    // ------------------------------------------------------------------
    // 诊断（`/status`）
    // ------------------------------------------------------------------

    /// 记录某 App 最近一次错误（握手被拒、唤醒失败 / 超时）。
    pub(crate) fn record_app_error(&self, app_id: &str, code: Option<&str>, message: &str) {
        lock(&self.diagnostics).last_errors.insert(
            app_id.to_owned(),
            LastError { code: code.map(str::to_owned), message: message.to_owned(), at_ms: unix_millis() },
        );
    }

    /// 记录 SDK 的 `app/diagnostic` 上报，并发 [`HubEvent::AppDiagnostic`]。
    pub(crate) fn record_report(&self, report: DiagnosticReport) {
        self.emit(HubEvent::AppDiagnostic {
            app_id: report.app_id.clone(),
            instance_id: report.instance_id.clone(),
            code: report.code.clone(),
            message: report.message.clone(),
            count: report.count,
        });
        let mut d = lock(&self.diagnostics);
        if d.reports.len() >= MAX_REPORTS {
            d.reports.pop_front();
        }
        d.reports.push_back(report);
    }

    /// 当前运行状态（[`Hub::status`]、`GET /status`）。
    pub(crate) fn status(&self) -> HubStatus {
        let endpoints = self.endpoints.get();
        let waking: Vec<(String, Option<String>)> = {
            let now = tokio::time::Instant::now();
            lock(&self.wakes)
                .iter()
                .filter(|w| w.is_active(now))
                .map(|w| w.target())
                .collect()
        };
        let (last_errors, reports) = {
            let d = lock(&self.diagnostics);
            (d.last_errors.clone(), d.reports.iter().cloned().collect())
        };
        let infos = self.registry().app_infos(&HashMap::new());
        let now = tokio::time::Instant::now();
        let leased = self.leased_connections(now);
        let mut apps: Vec<AppStatus> = infos
            .into_iter()
            .map(|a| {
                let power = |inst: &str, connected: bool| {
                    let mut p = lock(&self.power).instance(&a.app_id, inst, now)?;
                    if connected {
                        p.awake_reasons = self.awake_reasons(&a.app_id, inst, p.lifecycle_mode, &leased);
                    }
                    Some(p)
                };
                let is_waking = |inst: Option<&str>| {
                    waking.iter().any(|(app, i)| *app == a.app_id && (i.is_none() || i.as_deref() == inst))
                };
                let mut instances: Vec<InstanceStatus> = a
                    .instances
                    .into_iter()
                    .map(|info| InstanceStatus {
                        power: power(&info.instance_id, true),
                        info,
                        state: InstanceState::Connected,
                    })
                    .collect();
                instances.extend(a.dormant_instances.into_iter().map(|info| {
                    let state = if is_waking(Some(&info.instance_id)) {
                        InstanceState::Waking
                    } else {
                        InstanceState::Dormant
                    };
                    InstanceStatus { power: power(&info.instance_id, false), info, state }
                }));
                let state = if a.connected {
                    AppState::Connected
                } else if waking.iter().any(|(app, _)| *app == a.app_id) {
                    AppState::Waking
                } else if instances.is_empty() {
                    AppState::Disconnected
                } else {
                    AppState::Dormant
                };
                AppStatus {
                    last_error: last_errors.get(&a.app_id).cloned(),
                    wakes: lock(&self.power).app_wakes(&a.app_id),
                    app_id: a.app_id,
                    name: a.name,
                    kind: AppKind::App,
                    state,
                    instances,
                }
            })
            .collect();
        apps.extend(lock(&self.upstreams).iter().map(|(name, st)| AppStatus {
            app_id: name.clone(),
            name: st.server_name.clone().unwrap_or_else(|| name.clone()),
            kind: AppKind::Upstream,
            state: if st.connected() { AppState::Connected } else { AppState::Disconnected },
            instances: Vec::new(),
            last_error: st.last_error.as_ref().map(|m| LastError { code: None, message: m.clone(), at_ms: 0 }),
            wakes: 0,
        }));
        // 只出现过错误（如握手被拒）、从未登记的 App 也列出，便于诊断。
        for (app_id, err) in &last_errors {
            if !apps.iter().any(|a| &a.app_id == app_id) {
                apps.push(AppStatus {
                    app_id: app_id.clone(),
                    name: app_id.clone(),
                    kind: AppKind::App,
                    state: AppState::Disconnected,
                    instances: Vec::new(),
                    last_error: Some(err.clone()),
                    wakes: lock(&self.power).app_wakes(app_id),
                });
            }
        }
        apps.sort_by(|a, b| a.app_id.cmp(&b.app_id));
        HubStatus {
            identity: self.identity.clone(),
            listen: endpoints.and_then(|e| e.listen.clone()),
            ipc_endpoint: endpoints.and_then(|e| e.ipc_endpoint.clone()),
            started_at_ms: endpoints.map(|e| e.started_at_ms).unwrap_or(0),
            mcp_http: self.config.mcp_http,
            auth: AuthStatus {
                token_configured: self.config.http.token.is_some(),
                token_required_without_origin: self.config.http.token.is_some()
                    && self.config.http.require_token_without_origin,
            },
            mcp_sessions: lock(&self.sessions).len(),
            apps,
            reports,
        }
    }

    pub(crate) fn emit(&self, ev: HubEvent) {
        // 没有接收方时发送失败，忽略。
        let _ = self.events.send(ev);
    }

    pub(crate) fn mark_tools_changed(&self) {
        self.tools_dirty.store(true, Ordering::SeqCst);
        self.dirty.notify_one();
    }

    pub(crate) fn mark_resources_changed(&self) {
        self.resources_dirty.store(true, Ordering::SeqCst);
        self.dirty.notify_one();
    }

    /// 合并 `list_changed_debounce` 内的多次变化，发一次事件并通知所有 MCP 会话。
    async fn notify_loop(self: Arc<Self>) {
        loop {
            self.dirty.notified().await;
            tokio::time::sleep(self.config.list_changed_debounce).await;
            let tools = self.tools_dirty.swap(false, Ordering::SeqCst);
            let resources = self.resources_dirty.swap(false, Ordering::SeqCst);
            if !tools && !resources {
                continue;
            }
            if tools {
                self.emit(HubEvent::ToolsChanged);
            }
            if resources {
                self.emit(HubEvent::ResourcesChanged);
            }
            let peers: Vec<(u64, Peer<RoleServer>)> = lock(&self.sessions)
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            for (id, peer) in peers {
                let mut ok = true;
                if tools {
                    ok &= peer.notify_tool_list_changed().await.is_ok();
                }
                if resources && ok {
                    ok &= peer.notify_resource_list_changed().await.is_ok();
                }
                if !ok {
                    tracing::debug!(session = id, "MCP 会话已关闭，移除");
                    self.remove_session(id);
                }
            }
        }
    }

    #[cfg(feature = "mcp-server")]
    pub(crate) fn register_session(&self, id: u64, peer: Peer<RoleServer>) {
        lock(&self.sessions).insert(id, peer);
    }

    /// 移除 MCP 会话及其资源订阅。
    pub(crate) fn remove_session(self: &Arc<Self>, id: u64) {
        lock(&self.sessions).remove(&id);
        let uris: Vec<String> = lock(&self.resource_subs)
            .iter()
            .filter(|(_, s)| s.contains(&id))
            .map(|(u, _)| u.clone())
            .collect();
        for uri in uris {
            self.unsubscribe(id, &uri);
        }
    }

    // ------------------------------------------------------------------
    // 会话状态
    // ------------------------------------------------------------------

    pub(crate) fn drop_session_state(&self, key: &str) {
        self.release_leases(key);
        lock(&self.session_state).remove(key);
    }

    pub(crate) fn session_state(&self) -> MutexGuard<'_, HashMap<String, SessionState>> {
        lock(&self.session_state)
    }

    /// 会话的 `apps.select` 优先，其次 [`Hub::select_instance`]。
    pub(crate) fn selected_for(&self, key: &str, app_id: &str) -> Option<String> {
        lock(&self.session_state)
            .get(key)
            .and_then(|s| s.selected.get(app_id).cloned())
            .or_else(|| lock(&self.global_selected).get(app_id).cloned())
    }

    pub(crate) fn merged_selection(&self, key: &str) -> HashMap<String, String> {
        let mut out = lock(&self.global_selected).clone();
        if let Some(s) = lock(&self.session_state).get(key) {
            out.extend(s.selected.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        out
    }

    pub(crate) fn select_in_session(&self, key: &str, app_id: &str, instance_id: &str) {
        lock(&self.session_state)
            .entry(key.to_owned())
            .or_default()
            .selected
            .insert(app_id.to_owned(), instance_id.to_owned());
    }

    pub(crate) fn mark_delivered(&self, key: &str, app_id: &str, version: &str) {
        lock(&self.session_state)
            .entry(key.to_owned())
            .or_default()
            .delivered
            .insert(app_id.to_owned(), version.to_owned());
    }

    /// 该会话首次接触某 App（或其总览版本变化）时返回总览，并记为已附带。
    pub(crate) fn attach_overview(&self, key: &str, app_id: &str) -> Option<Overview> {
        let ov = self.overview(app_id)?;
        let mut st = lock(&self.session_state);
        let s = st.entry(key.to_owned()).or_default();
        if s.delivered.get(app_id) == Some(&ov.version) {
            return None;
        }
        s.delivered.insert(app_id.to_owned(), ov.version.clone());
        Some(ov)
    }

    // ------------------------------------------------------------------
    // 渐进暴露（spec/hub-api.md 3.7）
    // ------------------------------------------------------------------

    /// App 与上游工具（不含内置工具）的总数。
    fn tool_count(&self) -> usize {
        let apps = self.registry().tools().len();
        apps + lock(&self.upstreams).values().map(|s| s.tools.len()).sum::<usize>()
    }

    /// 当前是否按渐进暴露列出工具。
    pub(crate) fn progressive(&self) -> bool {
        match self.config.tool_exposure {
            ToolExposure::All => false,
            ToolExposure::Progressive => true,
            ToolExposure::Auto => self.tool_count() > self.config.tool_exposure_threshold,
        }
    }

    /// 会话中直接列出工具的 App：展开过 / 调用过的，以及选定了实例的（会话 `apps.select` 与全局选择）。
    /// 渐进暴露未生效时返回 `None`（全部列出）。
    pub(crate) fn exposed_apps(&self, key: &str) -> Option<HashSet<String>> {
        if !self.progressive() {
            return None;
        }
        let mut out: HashSet<String> = self.merged_selection(key).into_keys().collect();
        if let Some(s) = lock(&self.session_state).get(key) {
            out.extend(s.exposed.iter().cloned());
        }
        Some(out)
    }

    /// 把 App 记为本会话已展开。渐进暴露生效且此前未列出时返回 `true`（会话的工具列表因此变化）。
    pub(crate) fn expose_app(&self, key: &str, app_id: &str) -> bool {
        let selected = self.merged_selection(key).contains_key(app_id);
        let inserted = lock(&self.session_state)
            .entry(key.to_owned())
            .or_default()
            .exposed
            .insert(app_id.to_owned());
        inserted && !selected && self.progressive()
    }

    /// 只通知一个 MCP 会话工具列表已变化（渐进暴露下该会话展开了新的 App）。
    pub(crate) fn notify_session_tools_changed(self: &Arc<Self>, session: u64) {
        let Some(peer) = lock(&self.sessions).get(&session).cloned() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let shared = self.clone();
        rt.spawn(async move {
            if peer.notify_tool_list_changed().await.is_err() {
                tracing::debug!(session, "MCP 会话已关闭，移除");
                shared.remove_session(session);
            }
        });
    }

    /// MCP `tools/list`：内置工具 + （渐进暴露时只含已展开 App 的）App 工具 + 上游工具。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn mcp_tools(&self, key: &str) -> Vec<Tool> {
        let exposed = self.exposed_apps(key);
        let listed = |app_id: &str| exposed.as_ref().is_none_or(|e| e.contains(app_id));
        let mut tools = call::builtin_tools(exposed.is_some());
        tools.extend(
            self.registry()
                .tools()
                .iter()
                .filter(|t| listed(&t.app_id))
                .map(|t| call::to_mcp_tool(&t.app_id, &t.info, t.availability)),
        );
        let ups = lock(&self.upstreams);
        for (name, st) in ups.iter().filter(|(name, _)| listed(name)) {
            for t in &st.tools {
                let mut t = t.clone();
                t.name = format!("{name}.{}", t.name).into();
                tools.push(t);
            }
        }
        tools
    }

    /// `apps.tools` 的结果：某个 App（或上游）的全部工具定义。
    pub(crate) fn app_tools(&self, app_id: &str) -> Vec<HubTool> {
        self.all_tools(false)
            .into_iter()
            .filter(|(t, builtin)| !builtin && t.app_id == app_id)
            .map(|(t, _)| t)
            .collect()
    }

    // ------------------------------------------------------------------
    // 策略回调
    // ------------------------------------------------------------------

    pub(crate) fn approval_handler(&self) -> Option<Arc<dyn ApprovalHandler>> {
        lock(&self.approval_handler).clone()
    }

    pub(crate) fn pairing_handler(&self) -> Option<Arc<dyn PairingHandler>> {
        lock(&self.pairing_handler).clone()
    }

    /// 之前已配对过（token 匹配，或同一 appId + Origin 经 PairingHandler 同意过）。
    pub(crate) fn is_paired(&self, app_id: &str, origin: Option<&str>, token: Option<&str>) -> bool {
        if let Some(t) = token
            && lock(&self.paired_tokens).get(t).is_some_and(|a| a == app_id)
        {
            return true;
        }
        lock(&self.paired_origins).contains(&(app_id.to_owned(), origin.map(str::to_owned)))
    }

    pub(crate) fn remember_pairing(&self, app_id: &str, origin: Option<&str>, token: &str, approved: bool) {
        lock(&self.paired_tokens).insert(token.to_owned(), app_id.to_owned());
        if approved {
            lock(&self.paired_origins).insert((app_id.to_owned(), origin.map(str::to_owned)));
        }
    }

    // ------------------------------------------------------------------
    // 资源
    // ------------------------------------------------------------------

    pub(crate) async fn read_app_resource(
        self: &Arc<Self>,
        app_id: &str,
        name: &str,
        selected: Option<String>,
    ) -> Result<(ResourceInfo, ResourcesReadResult), ToolError> {
        // 只有休眠实例提供该资源：先唤醒（spec/lifecycle.md §9）。
        let plan = self
            .registry()
            .wake_plan_resource(app_id, name, selected.as_deref());
        let selected = match plan {
            Some(plan) => Some(
                self.wake_and_wait(&plan, std::pin::pin!(std::future::pending::<()>()))
                    .await?,
            ),
            None => selected,
        };
        let target = self
            .registry()
            .route_resource(app_id, name, selected.as_deref())?;
        let _work = target.conn.begin_work();
        let params = serde_json::to_value(ResourcesReadParams {
            name: name.to_owned(),
        })
        .unwrap_or(Value::Null);
        let v = target
            .conn
            .request(method::RESOURCES_READ, params, self.config.response_timeout)
            .await
            .map_err(|e| request_error(e, app_id))?;
        let result = serde_json::from_value::<ResourcesReadResult>(v.clone()).unwrap_or(
            ResourcesReadResult {
                contents: v,
                mime_type: None,
            },
        );
        Ok((target.resource, result))
    }

    /// 订阅资源（`session` 为 MCP 会话 ID 或 [`API_SUBSCRIBER`]）。
    pub(crate) fn subscribe(self: &Arc<Self>, session: u64, uri: &str) -> Result<(), ToolError> {
        let Some((app_id, _)) = parse_resource_uri(uri) else {
            return Err(ToolError::new(
                ErrorKind::ResourceNotFound,
                format!("无法识别的资源 URI：{uri}"),
            ));
        };
        lock(&self.resource_subs)
            .entry(uri.to_owned())
            .or_default()
            .insert(session);
        self.ensure_subscriptions(app_id);
        Ok(())
    }

    /// 取消订阅；没有订阅方时转发 `resources/unsubscribe`。
    pub(crate) fn unsubscribe(self: &Arc<Self>, session: u64, uri: &str) {
        let now_empty = {
            let mut subs = lock(&self.resource_subs);
            match subs.get_mut(uri) {
                Some(set) => {
                    set.remove(&session);
                    let empty = set.is_empty();
                    if empty {
                        subs.remove(uri);
                    }
                    empty
                }
                None => false,
            }
        };
        let Some((app_id, name)) = parse_resource_uri(uri) else {
            return;
        };
        if !now_empty {
            return;
        }
        let conns: Vec<_> = {
            let mut reg = self.registry();
            let holders: Vec<_> = reg
                .resource_holders(app_id, name)
                .into_iter()
                .filter(|(_, s)| *s)
                .collect();
            for (c, _) in &holders {
                reg.mark_subscribed(app_id, c.id, name, false);
            }
            holders.into_iter().map(|(c, _)| c).collect()
        };
        // 可能在会话析构（Drop）中调用，此时不一定处于 tokio 运行时内。
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for conn in conns {
            let params = serde_json::to_value(ResourceSubscribeParams {
                name: name.to_owned(),
            })
            .unwrap_or(Value::Null);
            let timeout = self.config.response_timeout;
            rt.spawn(async move {
                if let Err(e) = conn
                    .request(method::RESOURCES_UNSUBSCRIBE, params, timeout)
                    .await
                {
                    tracing::debug!(error = ?e, "resources/unsubscribe 失败");
                }
            });
        }
    }

    /// 让该 App 所有提供了已订阅资源的实例都处于订阅状态（连接、同步后调用）。
    pub(crate) fn ensure_subscriptions(self: &Arc<Self>, app_id: &str) {
        let names: Vec<String> = lock(&self.resource_subs)
            .keys()
            .filter_map(|uri| parse_resource_uri(uri))
            .filter(|(a, _)| *a == app_id)
            .map(|(_, n)| n.to_owned())
            .collect();
        let mut todo = Vec::new();
        {
            let mut reg = self.registry();
            for name in names {
                for (conn, subscribed) in reg.resource_holders(app_id, &name) {
                    if !subscribed {
                        reg.mark_subscribed(app_id, conn.id, &name, true);
                        todo.push((conn, name.clone()));
                    }
                }
            }
        }
        for (conn, name) in todo {
            let shared = self.clone();
            let app_id = app_id.to_owned();
            tokio::spawn(async move {
                let params = serde_json::to_value(ResourceSubscribeParams { name: name.clone() })
                    .unwrap_or(Value::Null);
                let r = conn
                    .request(
                        method::RESOURCES_SUBSCRIBE,
                        params,
                        shared.config.response_timeout,
                    )
                    .await;
                if let Err(e) = r {
                    tracing::warn!(app_id, resource = %name, error = ?e, "向 App 订阅资源失败");
                    shared
                        .registry()
                        .mark_subscribed(&app_id, conn.id, &name, false);
                }
            });
        }
    }

    /// SDK 报告资源内容变化：发事件，并通知订阅了该资源的 MCP 会话。
    pub(crate) fn resource_updated(self: &Arc<Self>, app_id: &str, name: &str) {
        let uri = resource_uri(app_id, name);
        self.emit(HubEvent::ResourceUpdated { uri: uri.clone() });
        let sessions: Vec<u64> = lock(&self.resource_subs)
            .get(&uri)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        let peers: Vec<(u64, Peer<RoleServer>)> = {
            let all = lock(&self.sessions);
            sessions
                .iter()
                .filter_map(|id| all.get(id).map(|p| (*id, p.clone())))
                .collect()
        };
        for (id, peer) in peers {
            let uri = uri.clone();
            let shared = self.clone();
            tokio::spawn(async move {
                if peer
                    .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                    .await
                    .is_err()
                {
                    shared.remove_session(id);
                }
            });
        }
    }

    pub(crate) fn apps_json(&self, selected: &HashMap<String, String>) -> Value {
        let mut v = self.registry().apps_json(selected);
        let ups: Vec<Value> = lock(&self.upstreams)
            .iter()
            .map(|(name, st)| {
                json!({
                    "appId": name,
                    "kind": "upstream",
                    "name": st.server_name.clone().unwrap_or_else(|| name.clone()),
                    "summary": upstream_overview(name, st).map(|o| o.summary),
                    "connected": st.connected(),
                    "command": st.config.command,
                    "tools": st.tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(),
                    "resourceCount": st.resources.len(),
                    "restarts": st.restarts,
                    "lastError": st.last_error,
                })
            })
            .collect();
        if let Some(apps) = v["apps"].as_array_mut() {
            apps.extend(ups);
        }
        v
    }

    // ------------------------------------------------------------------
    // 上游 MCP 服务器
    // ------------------------------------------------------------------

    pub(crate) fn is_upstream(&self, name: &str) -> bool {
        lock(&self.upstreams).contains_key(name)
    }

    /// 上游的 peer：不是上游时为 `None`，未连接时为 `Some(None)`。
    pub(crate) fn upstream_peer(&self, name: &str) -> Option<Option<Peer<RoleClient>>> {
        lock(&self.upstreams).get(name).map(|s| s.peer.clone())
    }

    /// 上游工具的原始定义（名称不含前缀）。
    pub(crate) fn upstream_tool(&self, name: &str, tool: &str) -> Option<Tool> {
        lock(&self.upstreams)
            .get(name)?
            .tools
            .iter()
            .find(|t| t.name == tool)
            .cloned()
    }

    pub(crate) fn upstream_display_name(&self, name: &str) -> String {
        lock(&self.upstreams)
            .get(name)
            .and_then(|s| s.server_name.clone())
            .unwrap_or_else(|| name.to_owned())
    }

    #[cfg(feature = "upstream")]
    pub(crate) fn upstream_connected(
        &self,
        name: &str,
        peer: Peer<RoleClient>,
        tools: Vec<Tool>,
        resources: Vec<Resource>,
        instructions: Option<String>,
        server_name: Option<String>,
    ) {
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            st.peer = Some(peer);
            st.tools = tools;
            st.resources = resources;
            st.instructions = instructions;
            st.server_name = server_name;
            st.last_error = None;
        }
        self.emit(HubEvent::UpstreamState {
            name: name.to_owned(),
            connected: true,
            error: None,
        });
        self.mark_tools_changed();
        self.mark_resources_changed();
    }

    pub(crate) fn upstream_disconnected(&self, name: &str, error: Option<String>) {
        let mut last_error = None;
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            let was_connected = st.connected();
            st.peer = None;
            st.tools.clear();
            st.resources.clear();
            st.restarts += 1;
            if error.is_some() {
                st.last_error = error;
            } else if was_connected {
                st.last_error = Some("进程已退出".into());
            }
            last_error = st.last_error.clone();
        }
        self.emit(HubEvent::UpstreamState {
            name: name.to_owned(),
            connected: false,
            error: last_error,
        });
        self.mark_tools_changed();
        self.mark_resources_changed();
    }

    #[cfg(feature = "upstream")]
    pub(crate) fn set_upstream_tools(&self, name: &str, tools: Vec<Tool>) {
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            st.tools = tools;
        }
        self.mark_tools_changed();
    }

    #[cfg(feature = "upstream")]
    pub(crate) fn set_upstream_resources(&self, name: &str, resources: Vec<Resource>) {
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            st.resources = resources;
        }
        self.mark_resources_changed();
    }

    /// 已连接上游的资源，URI 改为 `app-mcp://<name>/<编码后的上游 URI>`。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn upstream_resources(&self) -> Vec<Resource> {
        let ups = lock(&self.upstreams);
        let mut out = Vec::new();
        for (name, st) in ups.iter() {
            for r in &st.resources {
                let mut r = r.clone();
                r.uri = resource_uri(name, &encode_uri_component(&r.uri));
                r.name = format!("{name}.{}", r.name);
                out.push(r);
            }
        }
        out
    }

    /// App 或上游当前生效的总览。
    pub(crate) fn overview(&self, app_id: &str) -> Option<Overview> {
        if let Some(st) = lock(&self.upstreams).get(app_id) {
            return upstream_overview(app_id, st);
        }
        self.registry().overview(app_id)
    }

    /// 所有已知 App 与上游的一句话简介（按 appId 排序）。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn summaries(&self) -> Vec<AppSummary> {
        let mut out = self.registry().summaries();
        out.extend(lock(&self.upstreams).iter().map(|(name, st)| AppSummary {
            app_id: name.clone(),
            name: st.server_name.clone().unwrap_or_else(|| name.clone()),
            summary: upstream_overview(name, st).map(|o| o.summary),
        }));
        out.sort_by(|a, b| a.app_id.cmp(&b.app_id));
        out
    }

    // ------------------------------------------------------------------
    // 列表（Hub API / 格式导出）
    // ------------------------------------------------------------------

    /// 全部工具：`(工具, 是否内置)`，顺序为内置、App（按 appId）、上游。
    /// `with_apps_tools`：内置工具是否包含 `apps.tools`（渐进暴露生效时才列出）。
    pub(crate) fn all_tools(&self, with_apps_tools: bool) -> Vec<(HubTool, bool)> {
        let mut out: Vec<(HubTool, bool)> = call::builtin_hub_tools(with_apps_tools)
            .into_iter()
            .map(|t| (t, true))
            .collect();
        out.extend(
            self.registry()
                .tools()
                .into_iter()
                .map(|t| (call::app_hub_tool(&t.app_id, t.info, t.availability), false)),
        );
        let ups = lock(&self.upstreams);
        for (name, st) in ups.iter() {
            for t in &st.tools {
                out.push((call::upstream_hub_tool(name, t), false));
            }
        }
        out
    }

    /// 按当前全部工具计算导出名，并并入历史映射。
    pub(crate) fn name_codec(&self) -> NameCodec {
        // 导出名按全部工具（含 apps.tools）计算，与渐进暴露无关，保证名称稳定。
        let tools = self.all_tools(true);
        let codec = NameCodec::new(tools.iter().map(|(t, _)| t.name.as_str()));
        lock(&self.export_names).extend(codec.pairs().map(|(full, export)| (export.to_owned(), full.to_owned())));
        codec
    }

    /// 把导出名解析为全名；不认识的名称原样返回（视为全名）。
    pub(crate) fn resolve_export_name(&self, name: &str) -> String {
        if let Some(full) = lock(&self.export_names).get(name) {
            return full.clone();
        }
        let codec = self.name_codec();
        codec.full_name(name).unwrap_or(name).to_owned()
    }
}

/// 上游的总览：取 `instructions` 的前 100 个字符为简介，其余（≤ 2000）为正文。
fn upstream_overview(name: &str, st: &UpstreamState) -> Option<Overview> {
    let text = st.instructions.as_deref()?.trim();
    let summary: String = text
        .chars()
        .take(app_mcp_protocol::OVERVIEW_SUMMARY_MAX_CHARS)
        .collect();
    let rest: String = text
        .chars()
        .skip(app_mcp_protocol::OVERVIEW_SUMMARY_MAX_CHARS)
        .collect();
    let raw = app_mcp_protocol::AppOverview {
        summary,
        body: Some(rest.trim().to_owned()).filter(|b| !b.is_empty()),
        locale: None,
    };
    let display = st.server_name.as_deref().unwrap_or(name);
    Overview::new(name, display, &raw, OverviewSource::Upstream)
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

/// 绑定 `listen`；被占用时依次尝试 `alternates`。全部失败时返回第一个错误（带尝试过的地址）。
async fn bind_listen(listen: &str, alternates: &[String]) -> std::io::Result<TcpListener> {
    let first = match TcpListener::bind(listen).await {
        Ok(l) => return Ok(l),
        Err(e) => e,
    };
    if first.kind() != std::io::ErrorKind::AddrInUse || alternates.is_empty() {
        return Err(first);
    }
    for alt in alternates {
        match TcpListener::bind(alt).await {
            Ok(l) => {
                tracing::warn!("监听地址 {listen} 已被占用，改用备选地址 {alt}（网页 SDK 会依次尝试这些端口）");
                return Ok(l);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        format!("{first}（{listen} 及备选地址 {} 均已被占用）", alternates.join("、")),
    ))
}

pub(crate) fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl Hub {
    /// 取得单实例锁（若配置 [`HubConfig::run_dir`]），绑定 HTTP 服务与本地 IPC（若配置），启动后台任务，
    /// 写登记文件。必须在 tokio 运行时中调用。
    ///
    /// 错误：`ResourceBusy`（单实例锁已被持有）、`AddrInUse`（地址 / IPC 端点被占用）、
    /// `PermissionDenied`（非回环地址未允许远程）、`InvalidInput`（配置不合法）。
    pub async fn start(config: HubConfig) -> std::io::Result<Hub> {
        crate::features::check_config(&config)?;
        // 锁先于任何监听：并发启动的两个 Host 只有一个能走到绑定。
        let instance = config.run_dir.as_deref().map(Instance::acquire).transpose()?;
        let listener = match &config.listen {
            Some(addr) => Some(bind_listen(addr, &config.listen_alternates).await?),
            None => None,
        };
        let listen_addr = listener.as_ref().map(TcpListener::local_addr).transpose()?;
        if let Some(local) = listen_addr
            && !local.ip().is_loopback()
            && !config.http.allow_remote
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("监听地址 {local} 不是回环地址；如确需远程访问请允许远程（--http-allow-remote）"),
            ));
        }
        let ipc = match &config.ipc_endpoint {
            Some(text) => {
                let endpoint = app_mcp_protocol::Endpoint::parse(text)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                if !endpoint.is_ipc() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("ipc_endpoint 必须是本地 IPC 端点（unix: / pipe:）：{text}"),
                    ));
                }
                let listener = crate::ipc::IpcListener::bind(&endpoint).await?;
                Some((endpoint.to_string(), listener))
            }
            None => None,
        };
        let waker = config
            .waker
            .build()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.0.message))?;
        let shared = Arc::new(HubShared::new(config, waker));
        let ipc_endpoint = ipc.as_ref().map(|(e, _)| e.clone());
        let mut hub = Hub {
            shared: shared.clone(),
            listen_addr,
            ipc_endpoint,
            started_at_ms: unix_millis(),
            tasks: Mutex::new(vec![
                tokio::spawn(shared.clone().notify_loop()),
                tokio::spawn(shared.clone().dormant_sweep_loop()),
            ]),
            instance: Mutex::new(instance),
        };
        let health = hub.health_base();
        // 先于任何监听器开始服务：/status 从这里读监听位置与启动时刻。
        let _ = shared.endpoints.set(hub.endpoint_registry());
        let mut tasks = Vec::new();
        if let Some(listener) = listener {
            let router = Router::new(
                shared.clone(),
                Transport::Tcp,
                shared.config.http.clone(),
                shared.config.mcp_http,
                health.clone(),
            );
            tasks.push(tokio::spawn(router.serve_tcp(listener)));
        }
        if let Some((endpoint, listener)) = ipc {
            // IPC 上与 TCP 同样的路由；对端用户已由操作系统核对，不需要令牌（spec/protocol.md 1.4）。
            let router = Router::new(
                shared.clone(),
                Transport::Ipc,
                HttpOptions::default(),
                shared.config.mcp_http,
                health,
            );
            tasks.push(tokio::spawn(router.serve_ipc(listener)));
            tracing::info!(%endpoint, "本地 IPC 连接服务已启动");
        }
        let upstreams: Vec<(String, UpstreamConfig)> = lock(&shared.upstreams)
            .iter()
            .map(|(n, s)| (n.clone(), s.config.clone()))
            .collect();
        for (name, cfg) in upstreams {
            tasks.push(tokio::spawn(crate::upstream::run(shared.clone(), name, cfg)));
        }
        lock(&hub.tasks).extend(tasks);
        match listen_addr {
            Some(addr) => tracing::info!(
                "HTTP 服务已启动：App 连接 ws://{addr}/app{}",
                if shared.config.mcp_http { format!("，MCP http://{addr}/mcp") } else { String::new() }
            ),
            None => tracing::info!("Hub 已启动（未开启 TCP 服务）"),
        }
        let registry = hub.endpoint_registry();
        if let Some(inst) = hub.instance.get_mut().unwrap_or_else(|e| e.into_inner()).as_mut() {
            inst.publish(&registry)?;
            tracing::info!(path = %inst.registry_path().display(), "已写登记文件");
        }
        Ok(hub)
    }

    /// HTTP 服务（`/app`、`/mcp`、`/healthz`）实际监听的地址；未开启时为 `None`。
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr
    }

    /// 本地 IPC 连接服务的端点字符串（`unix:…` / `pipe:…`，可直接作为原生 SDK 的 `host_url`）；
    /// 未开启时为 `None`。
    pub fn ipc_endpoint(&self) -> Option<&str> {
        self.ipc_endpoint.as_deref()
    }

    /// 本进程的 Host 身份（`service` / `version` / `user` / `pid`）。
    pub fn identity(&self) -> &HostIdentity {
        &self.shared.identity
    }

    /// 登记文件的内容（实际监听位置与身份；配置了 [`HubConfig::run_dir`] 时已写入 `endpoints.json`）。
    pub fn endpoint_registry(&self) -> EndpointRegistry {
        EndpointRegistry {
            identity: self.shared.identity.clone(),
            listen: self.listen_addr.map(|a| a.to_string()),
            ipc_endpoint: self.ipc_endpoint.clone(),
            started_at_ms: self.started_at_ms,
        }
    }

    /// `/healthz` 的公共部分（`mcp_path` 等由各监听器的 [`Router`] 填写）。
    fn health_base(&self) -> Health {
        Health {
            identity: self.shared.identity.clone(),
            listen: self.listen_addr.map(|a| a.to_string()),
            ipc_endpoint: self.ipc_endpoint.clone(),
            app_path: crate::http_server::APP_PATH.to_owned(),
            mcp_path: None,
            token_required_for_browsers: false,
        }
    }

    /// 停止：中止后台任务（含上游子进程）、关闭所有 App 连接，删除登记文件并释放单实例锁。
    pub async fn shutdown(self) {
        for t in lock(&self.tasks).drain(..) {
            t.abort();
        }
        let conns = self.shared.registry().all_connections();
        for c in &conns {
            c.close();
        }
        // 给写任务一点时间发送 Close 帧。
        tokio::time::sleep(Duration::from_millis(20)).await;
        lock(&self.instance).take();
        tracing::info!("Hub 已停止");
    }

    // ---- 查询 ----

    /// 所有已知 App（静态清单、已连接实例）与上游（`kind = Upstream`）。
    pub fn apps(&self) -> Vec<AppInfo> {
        let selected = lock(&self.shared.global_selected).clone();
        let mut out = self.shared.registry().app_infos(&selected);
        out.extend(lock(&self.shared.upstreams).iter().map(|(name, st)| AppInfo {
            app_id: name.clone(),
            name: st.server_name.clone().unwrap_or_else(|| name.clone()),
            kind: AppKind::Upstream,
            summary: upstream_overview(name, st).map(|o| o.summary),
            connected: st.connected(),
            instances: Vec::new(),
            selected_instance: None,
            dormant_instances: Vec::new(),
        }));
        out
    }

    /// 工具列表。渐进暴露生效（spec/hub-api.md 3.7）且 `filter.apps` 为 `None` 时，只含内置工具
    /// （此时另有 `apps.tools`）与 `filter.session` 会话已展开 / 调用过 / 选定了实例的 App 的工具；
    /// 显式给出 `filter.apps` 时列出这些 App 的全部工具。
    pub fn tools(&self, filter: &ToolFilter) -> Vec<HubTool> {
        let exposed = self.shared.exposed_apps(&api_session_key(filter.session.as_deref()));
        let progressive = exposed.is_some();
        let exposed = exposed.filter(|_| filter.apps.is_none());
        self.shared
            .all_tools(progressive)
            .into_iter()
            .filter(|(t, builtin)| filter.accepts(t, *builtin))
            .filter(|(t, builtin)| *builtin || exposed.as_ref().is_none_or(|e| e.contains(&t.app_id)))
            .map(|(t, _)| t)
            .collect()
    }

    pub fn resources(&self) -> Vec<HubResource> {
        let mut out: Vec<HubResource> = self
            .shared
            .registry()
            .resources()
            .into_iter()
            .map(|r| HubResource {
                uri: resource_uri(&r.app_id, &r.info.name),
                name: format!("{}.{}", r.app_id, r.info.name),
                app_id: r.app_id,
                description: r.info.description,
                mime_type: r.info.mime_type,
                available: r.available,
            })
            .collect();
        for (name, st) in lock(&self.shared.upstreams).iter() {
            for r in &st.resources {
                out.push(HubResource {
                    uri: resource_uri(name, &encode_uri_component(&r.uri)),
                    name: format!("{name}.{}", r.name),
                    app_id: name.clone(),
                    description: r.description.clone().unwrap_or_default(),
                    mime_type: r.mime_type.clone(),
                    available: true,
                });
            }
        }
        out
    }

    pub fn overview(&self, app_id: &str) -> Option<AppOverviewInfo> {
        self.shared.overview(app_id).as_ref().map(overview_info)
    }

    /// 运行状态：身份、监听位置、令牌策略、各 App 与实例的状态（在线 / 休眠 / 唤醒中）、最近错误与 SDK 诊断上报
    /// （spec/hub-api.md 3.9）。`GET /status` 返回同样的内容。
    pub fn status(&self) -> HubStatus {
        self.shared.status()
    }

    // ---- 操作 ----

    /// 调用工具（与 MCP 出口同一实现：schema 校验 → 审批 → 路由 → 转发）。
    ///
    /// 工具执行层面的失败（参数不合法、用户拒绝、超时、App 报错等）在 `Ok(CallOutcome)` 的
    /// `result` 中返回；只有名称无法解析（格式不对或 appId 未知）时返回 `Err`。
    pub async fn call_tool(&self, req: CallRequest) -> Result<CallOutcome, HubError> {
        let ctx = CallCtx::from_request(req);
        self.shared
            .call(ctx, std::future::pending())
            .await
            .into_outcome()
    }

    /// 取消进行中的调用（含等待审批中的）；未知 callId 忽略。
    pub fn cancel_call(&self, call_id: &str) {
        if let Some((_, tx)) = lock(&self.shared.calls).remove(call_id) {
            let _ = tx.send(());
        }
    }

    /// 读取资源（`app-mcp://<appId>/<name>`）。
    pub async fn read_resource(&self, uri: &str) -> Result<ResourceContent, HubError> {
        call::read_resource(&self.shared, uri, &api_session_key(None))
            .await
            .map_err(|e| HubError(call::mcp_error_to_tool(&e)))
            .map(|r| call::first_content(uri, r))
    }

    /// 订阅资源变化，之后收到 [`HubEvent::ResourceUpdated`]。
    pub fn subscribe(&self, uri: &str) -> Result<(), HubError> {
        if let Some((app_id, _)) = parse_resource_uri(uri)
            && self.shared.is_upstream(app_id)
        {
            return Err(HubError::new(
                ErrorKind::ResourceNotFound,
                "上游 MCP 服务器的资源暂不支持订阅，请直接读取。",
            ));
        }
        self.shared
            .subscribe(API_SUBSCRIBER, uri)
            .map_err(HubError::from)
    }

    pub fn unsubscribe(&self, uri: &str) {
        self.shared.unsubscribe(API_SUBSCRIBER, uri);
    }

    /// 指定某 App 的目标实例（所有会话共用；会话内 `apps.select` 的选择优先）。`None` 清除。
    /// 渐进暴露生效时，选定实例的 App 在所有会话中直接列出，因此会触发一次工具列表变化。
    pub fn select_instance(&self, app_id: &str, instance_id: Option<&str>) {
        let changed = {
            let mut sel = lock(&self.shared.global_selected);
            match instance_id {
                Some(id) => sel.insert(app_id.to_owned(), id.to_owned()).is_none(),
                None => sel.remove(app_id).is_some(),
            }
        };
        if changed && self.shared.progressive() {
            self.shared.mark_tools_changed();
        }
    }

    /// 清除某个 API 会话的状态（已附带的总览、`apps.select`），并取消该会话发出的租约（`app/lease { ttlMs: 0 }`）。
    /// 厂商开始新对话时调用。spec 之外的补充方法。
    pub fn reset_session(&self, session: Option<&str>) {
        self.shared.drop_session_state(&api_session_key(session));
    }

    // ---- 事件 ----

    pub fn events(&self) -> broadcast::Receiver<HubEvent> {
        self.shared.events.subscribe()
    }

    // ---- 策略回调 ----

    pub fn set_approval_handler(&self, h: Arc<dyn ApprovalHandler>) {
        *lock(&self.shared.approval_handler) = Some(h);
    }

    /// 设置后，未知 App（无静态清单，或 Origin 不在白名单）连接时先返回 `pending`，
    /// 询问 handler 后以 `app/pairingResult` 通知结果。未设置时行为与原 Host 一致。
    pub fn set_pairing_handler(&self, h: Arc<dyn PairingHandler>) {
        *lock(&self.shared.pairing_handler) = Some(h);
    }

    /// 替换唤醒实现（默认由 `HubConfig::waker` 决定，即 [`crate::SystemWaker`]）。spec/hub-api.md 3.5。
    pub fn set_waker(&self, w: Arc<dyn Waker>) {
        *lock(&self.shared.waker) = Some(w);
    }

    /// 撤销 [`Hub::set_waker`]，恢复按 `HubConfig::waker` 构造的唤醒器（`none` 即不唤醒）。
    /// 绑定层清除自定义唤醒回调时调用。spec 之外的补充方法。
    pub fn reset_waker(&self) {
        // 配置在 Hub::start 时已成功构造过一次，这里不会失败；万一失败按不唤醒处理。
        *lock(&self.shared.waker) = self.shared.config.waker.build().ok().flatten();
    }

    // ---- 对外出口 ----

    /// 为一个 MCP 连接创建会话处理器（rmcp `ServerHandler`）。feature `mcp-server`。
    #[cfg(feature = "mcp-server")]
    pub fn mcp_session(&self) -> McpSession {
        McpSession::new(self.shared.clone())
    }

    /// 以 stdio 作为 MCP 传输运行，直到 MCP 客户端断开。feature `mcp-server`。
    #[cfg(feature = "mcp-server")]
    pub async fn serve_stdio(&self) -> anyhow::Result<()> {
        use rmcp::ServiceExt;
        let service = self.mcp_session().serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }

    /// 另开一个 HTTP 监听器（路径与主服务相同：`/app`、`/mcp`、`/healthz`，且总是提供 `/mcp`），返回实际监听地址。
    ///
    /// 非回环地址需要 `allow_remote`；否则返回错误。等价于
    /// [`Hub::serve_http_with`]`(addr, HttpOptions { allow_remote, ..Default::default() })`。
    pub async fn serve_http(&self, addr: &str, allow_remote: bool) -> std::io::Result<SocketAddr> {
        self.serve_http_with(
            addr,
            HttpOptions {
                allow_remote,
                ..Default::default()
            },
        )
        .await
    }

    /// 另开一个 HTTP 监听器，带访问令牌等选项（`/mcp` 总是开启）。主服务见 [`HubConfig::listen`]；
    /// 本方法用于额外的地址（如 `app-mcp-host` 兼容期内的旧 MCP 端口）。
    ///
    /// 可多次调用；同一 Hub 上的所有 HTTP 会话共享 App 连接，每个会话有独立的 `apps.select` 选择与
    /// “已附带总览”状态。
    pub async fn serve_http_with(
        &self,
        addr: &str,
        options: HttpOptions,
    ) -> std::io::Result<SocketAddr> {
        // 本方法总是提供 /mcp：缺少 MCP 出口时 Unsupported（spec/hub-api.md cargo features）。
        crate::features::require_mcp_server("MCP 出口（Hub::serve_http）")?;
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        if !local.ip().is_loopback() && !options.allow_remote {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "HTTP 监听地址 {local} 不是回环地址；如确需远程访问请加 --http-allow-remote"
                ),
            ));
        }
        let router = Router::new(self.shared.clone(), Transport::Tcp, options, true, self.health_base());
        let task = tokio::spawn(router.serve_tcp(listener));
        lock(&self.tasks).push(task);
        tracing::info!(%local, "额外的 HTTP 服务已启动：MCP http://{local}/mcp，App 连接 ws://{local}/app");
        Ok(local)
    }

    // ---- 工具格式导出（spec/hub-api.md 第 5 节）----

    /// 按格式导出工具定义（名称已编码为 `[a-zA-Z0-9_-]{1,64}`）。
    pub fn export_tools(&self, format: ToolFormat, filter: &ToolFilter) -> Value {
        let codec = self.shared.name_codec();
        format::export(format, &self.tools(filter), &codec)
    }

    /// 执行模型返回的一个工具调用，返回该格式的“工具结果”消息。永不失败：错误以该格式的错误结果返回。
    pub async fn dispatch(&self, format: ToolFormat, tool_call: Value) -> Value {
        self.dispatch_in_session(format, tool_call, None).await
    }

    /// 同 [`Hub::dispatch`]，指定会话（总览首次附带按会话计算）。spec 之外的补充方法。
    pub async fn dispatch_in_session(
        &self,
        format: ToolFormat,
        tool_call: Value,
        session: Option<&str>,
    ) -> Value {
        let parsed = match format::parse_call(format, &tool_call) {
            Ok(p) => p,
            Err(p) => {
                let r = call::error_result(&ToolError::new(ErrorKind::InvalidInput, p.error.clone().unwrap_or_default()));
                return format::render_result(format, &p, &r);
            }
        };
        if let Some(msg) = &parsed.error {
            let r = call::error_result(&ToolError::new(ErrorKind::InvalidInput, msg.clone()));
            return format::render_result(format, &parsed, &r);
        }
        let ctx = CallCtx {
            mcp_session: None,
            name: self.shared.resolve_export_name(&parsed.name),
            arguments: parsed.arguments.clone(),
            session_key: api_session_key(session),
            session: session.map(str::to_owned),
            instance_id: None,
            timeout: None,
            call_id: parsed.id.clone(),
        };
        let inv = self.shared.call(ctx, std::future::pending()).await;
        let r = match inv.to_mcp() {
            Ok(r) => r,
            Err(e) => call::error_result(&call::mcp_error_to_tool(&e)),
        };
        format::render_result(format, &parsed, &r)
    }

    /// 进程内 App（spec/hub-api.md 第 4 节，M2）：本期只预留，总是返回未实现错误。
    pub fn attach_local(
        &self,
        _hello: app_mcp_protocol::HelloParams,
    ) -> Result<LocalAppChannel, HubError> {
        Err(HubError::new(
            ErrorKind::UnsupportedProtocol,
            "进程内 App（attach_local）尚未实现，请通过 WebSocket 连接。",
        ))
    }
}

/// 进程内 App 通道（M2 预留，尚无实现）。
#[derive(Debug)]
pub struct LocalAppChannel {
    _private: (),
}

impl Drop for Hub {
    fn drop(&mut self) {
        for t in lock(&self.tasks).iter() {
            t.abort();
        }
    }
}

/// 按 spec/manifest.md 第 4 节加载清单：先读目录（按文件名排序），再读 `--manifest` 文件。
/// 失败的清单记录错误后跳过。
pub fn load_manifests(files: &[PathBuf], dir: Option<&Path>, dir_required: bool) -> Vec<Manifest> {
    let mut out = Vec::new();
    let mut push =
        |r: Result<app_mcp_manifest::LoadedManifest, app_mcp_manifest::ManifestError>| match r {
            Ok(loaded) => {
                for w in &loaded.warnings {
                    tracing::warn!(app_id = %loaded.manifest.app_id, "清单警告：{w}");
                }
                tracing::info!(app_id = %loaded.manifest.app_id, path = ?loaded.path, "已加载清单");
                out.push(loaded.manifest);
            }
            Err(e) => tracing::error!("{e}"),
        };
    if let Some(dir) = dir {
        match app_mcp_manifest::load_dir(dir) {
            Ok(results) => results.into_iter().for_each(&mut push),
            Err(e) if !dir_required && e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(dir = %dir.display(), "清单目录不存在，忽略");
            }
            Err(e) => tracing::error!(dir = %dir.display(), "读取清单目录失败：{e}"),
        }
    }
    for f in files {
        push(app_mcp_manifest::load_file(f));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip() {
        let uri = resource_uri("shop", "cart.state");
        assert_eq!(uri, "app-mcp://shop/cart.state");
        assert_eq!(parse_resource_uri(&uri), Some(("shop", "cart.state")));
        assert_eq!(parse_resource_uri("app-mcp://shop/"), None);
        assert_eq!(parse_resource_uri("app-mcp:///x"), None);
        assert_eq!(parse_resource_uri("file:///x"), None);
    }

    #[test]
    fn load_manifests_override_and_skip() {
        let dir =
            std::env::temp_dir().join(format!("app-mcp-hub-manifests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.json"),
            r#"{"manifestVersion":1,"appId":"a","name":"A1"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("bad.json"),
            r#"{"manifestVersion":1,"appId":"apps","name":"x"}"#,
        )
        .unwrap();
        let extra = dir.join("extra.txt");
        std::fs::write(&extra, r#"{"manifestVersion":1,"appId":"a","name":"A2"}"#).unwrap();
        let ms = load_manifests(&[extra], Some(&dir), false);
        assert_eq!(ms.len(), 2);
        let shared = HubShared::new(
            HubConfig {
                manifests: ms,
                ..Default::default()
            },
            None,
        );
        assert_eq!(
            shared.registry().manifest("a").map(|m| m.name.clone()),
            Some("A2".into())
        );
        assert!(load_manifests(&[], Some(&dir.join("missing")), false).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_selection_and_overview_delivery() {
        let shared = HubShared::new(HubConfig::default(), None);
        lock(&shared.global_selected).insert("shop".into(), "g".into());
        assert_eq!(shared.selected_for("a", "shop").as_deref(), Some("g"));
        shared.select_in_session("a", "shop", "s");
        assert_eq!(shared.selected_for("a", "shop").as_deref(), Some("s"));
        assert_eq!(shared.selected_for("b", "shop").as_deref(), Some("g"));
        assert_eq!(shared.merged_selection("a")["shop"], "s");
        shared.drop_session_state("a");
        assert_eq!(shared.selected_for("a", "shop").as_deref(), Some("g"));
        assert_eq!(api_session_key(None), "api");
        assert_eq!(api_session_key(Some("x")), "api:x");
    }

    #[test]
    fn pairing_memory() {
        let shared = HubShared::new(HubConfig::default(), None);
        assert!(!shared.is_paired("a", Some("o"), Some("t")));
        shared.remember_pairing("a", Some("o"), "t", false);
        assert!(shared.is_paired("a", None, Some("t")));
        assert!(!shared.is_paired("b", None, Some("t")));
        assert!(!shared.is_paired("a", Some("o"), None));
        shared.remember_pairing("a", Some("o"), "t2", true);
        assert!(shared.is_paired("a", Some("o"), None));
    }
}
