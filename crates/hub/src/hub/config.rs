//! Hub 配置（`HubConfig`）及各项默认值。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::DEFAULT_LISTEN_ADDR;

use crate::http_server::HttpOptions;
use crate::lease::LeasePolicy;
use crate::limits::{LimitPolicy, OutputValidation};
use crate::policy::PolicyConfig;
use crate::types::{ApprovalPolicy, McpProtocolMode, ToolExposure};
use crate::upstream::UpstreamConfig;
use crate::connector::Connector;
use crate::wake::WakerConfig;

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
    /// 绑定完成后写 `<run_dir>/endpoints.json`，停止时删除。已被锁定时 [`Hub::start`](crate::Hub::start) 返回
    /// `ResourceBusy`。默认 `None`（嵌入式 Hub 不参与）；`app-mcp-host` 为 `<配置目录>/run`。
    pub run_dir: Option<PathBuf>,
    /// 本地 IPC 端点（spec/protocol.md 1.2）：`unix:<绝对路径>`（Linux / macOS）或
    /// `pipe:\\.\pipe\<名称>`（Windows）。默认为平台默认端点
    /// （[`app_mcp_protocol::endpoint::default_ipc_endpoint`]；Android / iOS 上为 `None`）。
    /// `None` = 不开 IPC 服务。已有 Hub 在该端点监听时 [`Hub::start`](crate::Hub::start) 返回 `AddrInUse`。
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
    /// 等待 [`PairingHandler`](crate::types::PairingHandler) 的上限，超时视为拒绝。
    pub pairing_timeout: Duration,
    /// 每次调用某实例完成后发送的 `app/lease` 时长（spec/lifecycle.md 4.2）；`0` 关闭租约功能。
    /// 开启自适应租约（[`HubConfig::lease`]）时为无历史时的保守默认值。
    pub lease_ttl: Duration,
    /// 自适应租约策略（spec/lifecycle.md 第 13 节 B2，spec/hub-api.md 3.5）。`adaptive: false` 回退到固定 `lease_ttl`。
    pub lease: LeasePolicy,
    /// 唤醒后等待 App 回连的上限，超时返回 `APP_NOT_RESPONDING`。
    pub wake_timeout: Duration,
    /// 导航等待（spec/hub-api.md 3.14）：`app/navigate` 的回复与之后等待目标工具注册合计的上限，默认 5 秒
    /// （[`DEFAULT_NAVIGATE_TIMEOUT`]）。显式导航（`apps.navigate`）只等回复，同样受此约束。
    pub navigate_timeout: Duration,
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
    /// 持久状态目录（spec/hub-api.md 3.5「持久化」）：休眠记录写到 `<state_dir>/dormant/<appId>.json`（原子写、仅当前用户可读），
    /// [`Hub::start`](crate::Hub::start) 时读回，重启前休眠的 App 仍可列出、可唤醒。默认 `None`：Hub 不读写任何文件（嵌入式厂商按需开启）；
    /// `app-mcp-host` 为 `<配置目录>/state`。
    pub state_dir: Option<PathBuf>,
    /// App 未运行、清单没有显式声明 `wake` 时，是否由清单 `launch` 推导唤醒方式并冷启动
    /// （`launch.web` 的地址会被打开）。默认 `false`：只返回 `APP_DISCONNECTED` 与启动提示。
    pub wake_from_launch: bool,
    /// 唤醒器：`System`（默认，按平台执行系统激活）/ `None`（不唤醒，返回 `APP_DISCONNECTED`）/
    /// `Exec`（执行指定程序）。[`Hub::set_waker`](crate::Hub::set_waker) 可再替换为自定义实现。
    pub waker: WakerConfig,
    /// 工具暴露方式（spec/hub-api.md 3.7）：`All` / `Progressive` / `Auto`（默认）。
    pub tool_exposure: ToolExposure,
    /// `Auto` 的阈值：App 与上游工具（不含内置工具）总数**超过**此值时按渐进暴露。默认 40。
    pub tool_exposure_threshold: usize,
    /// 资源保护（spec/hub-api.md 3.11）：按（App, 工具）与按 App 的调用频率上限、参数 / 结果 / 资源内容的大小上限。
    /// 超出返回 `RATE_LIMITED` / `PAYLOAD_TOO_LARGE`。默认值宽松（[`LimitPolicy::default`]）。
    pub limits: LimitPolicy,
    /// App 结果与其声明的 `outputSchema` 不符时的处理（第 19 项 R2）。默认只记日志，不拒绝。
    pub output_validation: OutputValidation,
    /// 调用进度转发给 Agent（MCP `notifications/progress`）的最小间隔（spec/hub-api.md 3.12）：间隔内只保留最新一条。
    /// 默认 [`DEFAULT_PROGRESS_INTERVAL`]；`0` = 不合并（不递增的进度仍丢弃）。
    pub progress_interval: Duration,
    /// 策略规则（spec/hub-api.md 3.13）：`hide` 从所有列表中去掉 App / 工具（调用按不存在），`deny` 在调用 / 唤醒执行点拒绝
    /// （`POLICY_DENIED`）。默认无规则：行为与没有策略时完全一致。运行中可用 [`Hub::set_policy`](crate::Hub::set_policy) 替换。
    pub policy: PolicyConfig,
    /// 名字服务连接器（spec/naming.md、spec/hub-api.md 3.16）：Hub 经它们发现 App（只读名字列表与事件，从不为发现而
    /// 启动进程），调用时按名拨号（未运行由系统激活），通道在宽限后关闭。路由顺序：已有活连接 → 按名拨号 →
    /// 唤醒描述（[`HubConfig::waker`]）。默认空（不按名寻址）；Linux 上 `app-mcp-host serve --name-service` 加入
    /// D-Bus 会话总线连接器（[`crate::connector::DbusConnector`]）。
    pub connectors: Vec<Arc<dyn Connector>>,
    /// 按名拨入的通道在最后一次调用完成后保持的时间（spec/naming.md 7.2 `graceMs`）：关闭时刻为
    /// `max(最后一条消息 + 本值, 租约到期)`，宽限内到来的调用合并进同一通道。默认 [`DEFAULT_CHANNEL_GRACE`]。
    pub channel_grace: Duration,
    /// 无会话（modern）MCP 请求的 Agent 任务在请求流空闲多久后回收（docs/plans/16-agent-os.md P1 / U9；
    /// 回收 = 收回其仍未到期的租约、清除 `apps.select` 等状态）。`0` = 不因空闲回收。默认 [`DEFAULT_TASK_IDLE_TTL`]。
    /// legacy MCP 会话与 Hub API 会话的任务不受影响（随会话结束 / `reset_session`）。
    pub task_idle_ttl: Duration,
    /// 无会话（modern）MCP 请求的工具暴露方式（spec/hub-api.md 3.7「无会话请求的列表」）。默认 [`ToolExposure::All`]：
    /// 无会话请求的 `tools/list` 不能随调用 / `apps.tools` / `apps.select` 变化（MCP 2026-07-28，SEP-2567），渐进暴露只剩
    /// "内置工具 + 全局选定的 App"；通用客户端只能调用列出的工具（Claude Code 2.1.281 实测，docs/plans/12-mcp-stateless.md U3），
    /// 因此默认全部列出。
    /// legacy 会话与 Hub API 仍按 [`HubConfig::tool_exposure`]。
    pub stateless_tool_exposure: ToolExposure,
    /// 无会话请求的主体级 `apps.select` 选择的空闲有效期：选定或最近一次用于路由后这么久未再使用即失效，回到默认路由
    /// （docs/plans/12-mcp-stateless.md S6 / U6）。`0` = 不单独过期（仍随主体任务的空闲回收清除）。默认
    /// [`DEFAULT_PRINCIPAL_SELECT_TTL`]。legacy 会话与 Hub API 的选择不过期（随会话结束 / `reset_session`）。
    pub principal_select_ttl: Duration,
    /// 无会话请求的列表结果（`tools/list`、`resources/list`、`resources/templates/list`、`server/discover`）的 `ttlMs`
    /// （SEP-2549；`cacheScope` 恒为 `private`）。默认 [`DEFAULT_STATELESS_LIST_TTL`]。legacy 会话的结果不带这两个字段。
    pub stateless_list_ttl: Duration,
    /// MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」）。默认 [`McpProtocolMode::Auto`]：双版本，可协商 2026-07-28；
    /// [`McpProtocolMode::LegacyOnly`] 为回退开关，只声明到 2025-11-25（第 12 项 S7 之前的行为）。
    pub mcp_protocol_mode: McpProtocolMode,
    /// 每个主体同时打开的 `subscriptions/listen` 流数上限（B-07）；超出时该 listen 请求以 `RATE_LIMITED` 错误结束。
    /// `0` = 不提供 `subscriptions/listen`。默认 [`DEFAULT_MAX_LISTEN_STREAMS`]。
    pub max_listen_streams: usize,
    /// 一个 listen 流接受的资源 URI 数上限（超出的不接受，确认通知中只列出接受的部分）。默认 [`DEFAULT_MAX_LISTEN_RESOURCES`]。
    pub max_listen_resources: usize,
    /// 每个主体同时存在的任务句柄数上限（`apps.task.begin`，spec/hub-api.md 3.6「任务句柄」，B-07）；达到上限时签发以
    /// `RATE_LIMITED` 失败。`0` = 不提供任务句柄（`apps.task.*` 不列出，`taskId` 一律无效）。默认 [`DEFAULT_MAX_TASK_HANDLES`]。
    pub max_task_handles: usize,
    /// 已登记的 Agent 及其访问令牌（第 16 项 N5，spec/hub-api.md 3.6「Agent 身份」）：出示其令牌的 `/mcp` 请求的主体为
    /// `agent:<名>`，任务、任务句柄、`apps.select`、租约与 listen 流上限按 Agent 分开。默认无登记（所有请求为本机主体）。
    /// 运行中可用 [`Hub::set_agents`](crate::Hub::set_agents) 替换。
    pub agents: crate::agents::AgentsConfig,
}

/// [`HubConfig::max_task_handles`] 的默认值。
///
/// @why 32：一个 Agent 通常同时只有一到几个并行任务；N5 之前本机所有无会话 Agent 共用一个主体，32 足够多个 Agent 并存，
/// 又限制了异常 Agent 反复签发句柄占用的任务表与租约（每个句柄任务空闲 `task_idle_ttl` 后回收）。
pub const DEFAULT_MAX_TASK_HANDLES: usize = 32;

/// [`HubConfig::max_listen_streams`] 的默认值。
///
/// @why 16：一个 MCP 客户端通常每个服务器只开一个 listen 流（重连时旧流先断开）；N5 之前本机所有无会话客户端共用一个主体，
/// 16 足够多个客户端并存，又限制了异常客户端反复打开流占用的连接与内存。
pub const DEFAULT_MAX_LISTEN_STREAMS: usize = 16;

/// [`HubConfig::max_listen_resources`] 的默认值。
///
/// @why 256：远多于单个 Agent 实际关心的资源数；限制一个流在 Hub 的订阅表与 App 侧 `resources/subscribe` 上造成的负担。
pub const DEFAULT_MAX_LISTEN_RESOURCES: usize = 256;

/// [`HubConfig::principal_select_ttl`] 的默认值。
///
/// @why 60 秒：与租约空闲收回（`idle_revoke` 30 秒）、自适应租约上限（60 秒）同量级——选择失效时该实例的租约多半也已
/// 收回；N5 之前所有无会话 Agent 共用一个主体（docs/plans/12-mcp-stateless.md R1），较短的有效期限制一个 Agent 的选择
/// 影响另一个的时长。`apps.select` 的结果写明有效期，模型可重新选择。
pub const DEFAULT_PRINCIPAL_SELECT_TTL: Duration = Duration::from_secs(60);

/// [`HubConfig::stateless_list_ttl`] 的默认值。
///
/// @why 5 秒：列表随 App 连接 / 断开变化；第 12 项 S7 起无会话客户端可经 `subscriptions/listen` 收到 `list_changed`，
/// 但不开 listen 流的客户端只能靠过期重取，本机重取的代价很小。按实测再调（docs/plans/12-mcp-stateless.md U9）。
pub const DEFAULT_STATELESS_LIST_TTL: Duration = Duration::from_secs(5);

/// [`HubConfig::task_idle_ttl`] 的默认值。
///
/// @why 10 分钟：覆盖一轮对话中用户思考的停顿；不小于自适应租约默认上限（60 s），默认配置下空闲回收不会提前截断租约。
pub const DEFAULT_TASK_IDLE_TTL: Duration = Duration::from_secs(10 * 60);

/// [`HubConfig::channel_grace`] 的默认值（spec/naming.md 7.2）。
pub const DEFAULT_CHANNEL_GRACE: Duration = Duration::from_secs(15);

/// [`HubConfig::progress_interval`] 的默认值。
pub const DEFAULT_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// [`HubConfig::tool_exposure_threshold`] 的默认值。
pub const DEFAULT_TOOL_EXPOSURE_THRESHOLD: usize = 40;

/// [`HubConfig::wake_rate_limit`] 的默认值（每 App 每分钟）。
///
/// @why 一次唤醒约 5.6 ms SDK 线程 / 约 19 ms 进程 CPU（TASKS.md 4e0 真机测量）。正常调用经唤醒去重与 60 秒租约
/// 合并为每分钟至多约 1 次；6 次给 `on-demand`（10 秒 grace）下的断续调用留余量，同时把唤醒 / 休眠循环
/// 限制在约 0.1 秒 CPU / 分钟。
pub const DEFAULT_WAKE_RATE_LIMIT: u32 = 6;

/// [`HubConfig::navigate_timeout`] 的默认值。
pub const DEFAULT_NAVIGATE_TIMEOUT: Duration = Duration::from_secs(5);

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
            lease: LeasePolicy::default(),
            wake_timeout: Duration::from_secs(15),
            navigate_timeout: DEFAULT_NAVIGATE_TIMEOUT,
            wake_token_ttl: Duration::from_secs(60),
            wake_rate_limit: DEFAULT_WAKE_RATE_LIMIT,
            legacy_heartbeat: false,
            dormant_ttl: Duration::from_secs(24 * 60 * 60),
            dormant_replaced_by_new_instance: true,
            state_dir: None,
            wake_from_launch: false,
            waker: WakerConfig::System,
            tool_exposure: ToolExposure::Auto,
            tool_exposure_threshold: DEFAULT_TOOL_EXPOSURE_THRESHOLD,
            limits: LimitPolicy::default(),
            output_validation: OutputValidation::default(),
            progress_interval: DEFAULT_PROGRESS_INTERVAL,
            policy: PolicyConfig::default(),
            connectors: Vec::new(),
            channel_grace: DEFAULT_CHANNEL_GRACE,
            task_idle_ttl: DEFAULT_TASK_IDLE_TTL,
            stateless_tool_exposure: ToolExposure::All,
            principal_select_ttl: DEFAULT_PRINCIPAL_SELECT_TTL,
            stateless_list_ttl: DEFAULT_STATELESS_LIST_TTL,
            mcp_protocol_mode: McpProtocolMode::Auto,
            max_listen_streams: DEFAULT_MAX_LISTEN_STREAMS,
            max_listen_resources: DEFAULT_MAX_LISTEN_RESOURCES,
            max_task_handles: DEFAULT_MAX_TASK_HANDLES,
            agents: crate::agents::AgentsConfig::default(),
        }
    }
}
