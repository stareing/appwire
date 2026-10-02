//! Hub 配置：上游、租约、资源上限，及 [`HubConfig`] 到 `app_mcp_hub::HubConfig` 的转换。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use app_mcp_hub as hub;

use super::{HubError, McpProtocolMode, OutputValidation, PolicyConfig, Risk, ToolExposure, WakerConfig};

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
    /// HTTP 监听地址：`/app`（App 的 WebSocket 连接）、`/healthz`，`mcp_http` 时另有 `/mcp`（spec/protocol.md 1.3）。
    /// 端口 0 = 随机；显式给出时只绑定该地址。为空时为 `127.0.0.1:7717`（被占用时依次尝试 7737、7757）。
    #[uniffi(default = None)]
    pub listen: Option<String>,
    /// `false` = 不开 HTTP 服务（仅本地 IPC / 上游）。
    #[uniffi(default = true)]
    pub enable_listen: bool,
    /// 是否在 `listen` 上提供 MCP Streamable HTTP（`/mcp`），默认 `false`。
    #[uniffi(default = false)]
    pub mcp_http: bool,
    /// 单实例锁与登记文件目录（`<run_dir>/hub.lock`、`endpoints.json`，spec/protocol.md 1.5、1.7）；为空时不参与。
    #[uniffi(default = None)]
    pub run_dir: Option<String>,
    /// 持久状态目录（spec/hub-api.md 3.5「持久化」）：休眠记录写到 `<state_dir>/dormant/<appId>.json`（原子写、仅当前用户可读），
    /// 启动时读回，重启前休眠的 App 仍可列出、可唤醒。为空时不读写任何文件。
    #[uniffi(default = None)]
    pub state_dir: Option<String>,
    /// 本地 IPC 端点（`unix:<绝对路径>` / `pipe:\\.\pipe\<名称>`，spec/protocol.md 1.2）；
    /// 为空时为平台默认端点（原生 App 默认连接这里）。
    #[uniffi(default = None)]
    pub ipc_endpoint: Option<String>,
    /// `false` = 不开本地 IPC 服务。
    #[uniffi(default = true)]
    pub enable_ipc: bool,
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
    /// 导航等待上限（App 回复 + 目标工具注册，默认 5 s，独立于 `wake_timeout_ms`；spec/hub-api.md 3.14 / 3.15），
    /// 超时 → `NAVIGATION_FAILED`。
    #[uniffi(default = None)]
    pub navigate_timeout_ms: Option<u64>,
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
    // ---- 功耗（spec/lifecycle.md 第 11–13 节）----
    /// 每 App 每分钟最多实际发出的唤醒激活次数（默认 6）；`0` 不限。超出时调用以 `LAUNCH_FAILED`
    /// （`details.code = "WAKE_RATE_LIMITED"`）结束。
    #[uniffi(default = None)]
    pub wake_rate_limit: Option<u32>,
    /// 回退到旧心跳：对所有 App 连接发 `ping` 并按无消息断开（默认 `false`）。
    #[uniffi(default = None)]
    pub legacy_heartbeat: Option<bool>,
    /// 自适应租约（默认见 [`LeaseConfig`]）。
    #[uniffi(default = None)]
    pub lease: Option<LeaseConfig>,
    // ---- 资源保护（spec/hub-api.md 3.11）----
    /// 限流与大小上限（默认见 [`LimitsConfig`]）。
    #[uniffi(default = None)]
    pub limits: Option<LimitsConfig>,
    /// 结果与 `outputSchema` 不符时的处理（默认 `Log`）。
    #[uniffi(default = None)]
    pub output_validation: Option<OutputValidation>,
    // ---- 策略挂点（spec/hub-api.md 3.13）----
    /// 隐藏 / 拒绝规则；为空 = 无规则（行为与没有策略时相同）。规则不合法时 `start` 返回 `InvalidConfig`。
    /// 运行中用 `AppMcpHub::set_policy` 替换。
    #[uniffi(default = None)]
    pub policy: Option<PolicyConfig>,
    // ---- 渐进暴露（spec/hub-api.md 3.7）----
    /// 工具暴露方式（默认 `Auto`）。
    #[uniffi(default = None)]
    pub tool_exposure: Option<ToolExposure>,
    /// `Auto` 的阈值：App 与上游工具总数超过此值时渐进暴露（默认 40）。
    #[uniffi(default = None)]
    pub tool_exposure_threshold: Option<u32>,
    // ---- 按名寻址（spec/hub-api.md 3.16）----
    /// 按名拨入的通道在最后一次调用后保持的时间（spec/naming.md 7.2 `graceMs`，默认 15 s）。只对
    /// `AppMcpHub::start_with_name_service` 的名字服务生效。
    #[uniffi(default = None)]
    pub channel_grace_ms: Option<u64>,
    // ---- 无会话 MCP 请求（spec/hub-api.md 3.6 / 3.7）----
    /// 无会话调用方（`principal:<主体>`）的 Agent 任务在请求流空闲多久后回收（收回租约、清除选择），默认 10 分钟；
    /// `0` 不因空闲回收。
    #[uniffi(default = None)]
    pub task_idle_ttl_ms: Option<u64>,
    /// 无会话请求的工具暴露方式（默认 `All`）；渐进时列表只含内置工具与全局选定实例的 App，不随调用变化。
    #[uniffi(default = None)]
    pub stateless_tool_exposure: Option<ToolExposure>,
    /// 无会话请求的主体级 `apps.select` 选择的空闲有效期（默认 60 s）；`0` 不单独过期。
    #[uniffi(default = None)]
    pub principal_select_ttl_ms: Option<u64>,
    /// 无会话请求的列表结果所带缓存提示 `ttlMs`（默认 5 s）。
    #[uniffi(default = None)]
    pub stateless_list_ttl_ms: Option<u64>,
    // ---- MCP 出口协议版本与通知（spec/hub-api.md 3.6）----
    /// 协商的协议版本范围（默认 `Auto`）。
    #[uniffi(default = None)]
    pub mcp_protocol_mode: Option<McpProtocolMode>,
    /// 每个主体同时打开的 `subscriptions/listen` 流数上限（默认 16）；`0` 不提供 listen。
    #[uniffi(default = None)]
    pub max_listen_streams: Option<u32>,
    /// 一个 listen 流接受的资源 URI 数上限（默认 256）。
    #[uniffi(default = None)]
    pub max_listen_resources: Option<u32>,
    // ---- 任务句柄（spec/hub-api.md 3.6「任务句柄」）----
    /// 每个主体同时存在的任务句柄数上限（默认 32，超出时 `apps.task.begin` 报 `RATE_LIMITED`）；`0` 不提供任务句柄。
    #[uniffi(default = None)]
    pub max_task_handles: Option<u32>,
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig {
            listen: None,
            enable_listen: true,
            mcp_http: false,
            run_dir: None,
            state_dir: None,
            ipc_endpoint: None,
            enable_ipc: true,
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
            navigate_timeout_ms: None,
            wake_token_ttl_ms: None,
            dormant_ttl_ms: None,
            dormant_replaced_by_new_instance: None,
            wake_from_launch: None,
            waker: None,
            wake_rate_limit: None,
            legacy_heartbeat: None,
            lease: None,
            limits: None,
            output_validation: None,
            policy: None,
            tool_exposure: None,
            tool_exposure_threshold: None,
            channel_grace_ms: None,
            task_idle_ttl_ms: None,
            stateless_tool_exposure: None,
            principal_select_ttl_ms: None,
            stateless_list_ttl_ms: None,
            mcp_protocol_mode: None,
            max_listen_streams: None,
            max_listen_resources: None,
            max_task_handles: None,
        }
    }
}

/// 自适应租约策略（spec/lifecycle.md 第 13 节 B2）：租约 = 同一（会话, App）最近 `window` 个调用间隔的 p90 + `margin_ms`，
/// 限制在 [`min_ms`, `max_ms`]；样本不足 3 个时用 `HubConfig.lease_ttl_ms`。为空的字段取默认值。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct LeaseConfig {
    /// 是否按调用间隔自适应（默认 `true`）；`false` = 固定 `lease_ttl_ms`（4e 之前的行为）。
    #[uniffi(default = None)]
    pub adaptive: Option<bool>,
    /// 统计最近多少个间隔（默认 20，须 ≥ 1）。
    #[uniffi(default = None)]
    pub window: Option<u32>,
    /// p90 之上的余量（默认 5000）。
    #[uniffi(default = None)]
    pub margin_ms: Option<u64>,
    /// 下限（默认 5000）。
    #[uniffi(default = None)]
    pub min_ms: Option<u64>,
    /// 上限（默认 60000，须 ≥ `min_ms`）；超过它的间隔不计入统计。
    #[uniffi(default = None)]
    pub max_ms: Option<u64>,
    /// 会话无请求多久后收回其默认租约（默认 30000）；`0` 不收回。
    #[uniffi(default = None)]
    pub idle_revoke_ms: Option<u64>,
}

impl From<LeaseConfig> for hub::LeaseOverrides {
    fn from(c: LeaseConfig) -> Self {
        hub::LeaseOverrides {
            adaptive: c.adaptive,
            window: c.window,
            margin_ms: c.margin_ms,
            min_ms: c.min_ms,
            max_ms: c.max_ms,
            idle_revoke_ms: c.idle_revoke_ms,
        }
    }
}

/// 资源保护策略（spec/hub-api.md 3.11；与 JSON 配置 `limits` 同构）。为空的字段取默认值。
/// 也用于 [`HubStatus::limits`]（全部字段给出）。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct LimitsConfig {
    /// 每（App, 工具）每分钟调用数（默认 120）；`0` 不限。
    #[uniffi(default = None)]
    pub tool_rate_per_minute: Option<u32>,
    /// 每（App, 工具）允许的突发调用数（默认 30）；限流时须 ≥ 1。
    #[uniffi(default = None)]
    pub tool_rate_burst: Option<u32>,
    /// 每 App 每分钟调用数（默认 600）；`0` 不限。
    #[uniffi(default = None)]
    pub app_rate_per_minute: Option<u32>,
    /// 每 App 允许的突发调用数（默认 60）；限流时须 ≥ 1。
    #[uniffi(default = None)]
    pub app_rate_burst: Option<u32>,
    /// 调用参数字节上限（默认 1 MiB）；`0` 不限。超出 → `PAYLOAD_TOO_LARGE`。
    #[uniffi(default = None)]
    pub max_arguments_bytes: Option<u64>,
    /// 调用结果字节上限（默认 4 MiB）；`0` 不限。
    #[uniffi(default = None)]
    pub max_result_bytes: Option<u64>,
    /// 资源内容字节上限（默认 4 MiB）；`0` 不限。
    #[uniffi(default = None)]
    pub max_resource_bytes: Option<u64>,
    /// 每个已登记 Agent（所有 App 合计）每分钟调用数（默认 `0` 不限，第 16 项 P3）；本机主体与 Hub API 不受此限。
    #[uniffi(default = None)]
    pub agent_rate_per_minute: Option<u32>,
    /// 每个已登记 Agent 允许的突发调用数；限流时须 ≥ 1。
    #[uniffi(default = None)]
    pub agent_rate_burst: Option<u32>,
}

impl From<LimitsConfig> for hub::LimitOverrides {
    fn from(c: LimitsConfig) -> Self {
        hub::LimitOverrides {
            tool_rate_per_minute: c.tool_rate_per_minute,
            tool_rate_burst: c.tool_rate_burst,
            app_rate_per_minute: c.app_rate_per_minute,
            app_rate_burst: c.app_rate_burst,
            agent_rate_per_minute: c.agent_rate_per_minute,
            agent_rate_burst: c.agent_rate_burst,
            max_arguments_bytes: c.max_arguments_bytes,
            max_result_bytes: c.max_result_bytes,
            max_resource_bytes: c.max_resource_bytes,
        }
    }
}

impl From<hub::LimitOverrides> for LimitsConfig {
    fn from(c: hub::LimitOverrides) -> Self {
        LimitsConfig {
            tool_rate_per_minute: c.tool_rate_per_minute,
            tool_rate_burst: c.tool_rate_burst,
            app_rate_per_minute: c.app_rate_per_minute,
            app_rate_burst: c.app_rate_burst,
            max_arguments_bytes: c.max_arguments_bytes,
            max_result_bytes: c.max_result_bytes,
            max_resource_bytes: c.max_resource_bytes,
            agent_rate_per_minute: c.agent_rate_per_minute,
            agent_rate_burst: c.agent_rate_burst,
        }
    }
}

impl HubConfig {
    pub(crate) fn into_hub(self) -> Result<hub::HubConfig, HubError> {
        let mut c = hub::HubConfig::default();
        if !self.enable_listen {
            c.listen = None;
        } else if let Some(addr) = self.listen {
            // 显式地址：只绑定它，不尝试备选端口。
            c.listen = Some(addr);
            c.listen_alternates = Vec::new();
        }
        c.mcp_http = self.mcp_http;
        c.run_dir = self.run_dir.map(PathBuf::from);
        c.state_dir = self.state_dir.map(PathBuf::from);
        if !self.enable_ipc {
            c.ipc_endpoint = None;
        } else if let Some(endpoint) = self.ipc_endpoint {
            c.ipc_endpoint = Some(endpoint);
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
        set(&mut c.navigate_timeout, self.navigate_timeout_ms);
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
        if let Some(v) = self.wake_rate_limit {
            c.wake_rate_limit = v;
        }
        if let Some(v) = self.legacy_heartbeat {
            c.legacy_heartbeat = v;
        }
        if let Some(l) = self.lease {
            hub::LeaseOverrides::from(l).apply(&mut c.lease);
            c.lease.validate().map_err(|detail| HubError::InvalidConfig { detail: format!("lease：{detail}") })?;
        }
        if let Some(l) = self.limits {
            hub::LimitOverrides::from(l).apply(&mut c.limits);
            c.limits.validate().map_err(|detail| HubError::InvalidConfig { detail })?;
        }
        if let Some(v) = self.output_validation {
            c.output_validation = v.into();
        }
        if let Some(p) = self.policy {
            let p = hub::PolicyConfig::from(p);
            p.validate().map_err(|detail| HubError::InvalidConfig { detail: format!("policy：{detail}") })?;
            c.policy = p;
        }
        if let Some(v) = self.tool_exposure {
            c.tool_exposure = v.into();
        }
        if let Some(v) = self.tool_exposure_threshold {
            c.tool_exposure_threshold = v as usize;
        }
        set(&mut c.channel_grace, self.channel_grace_ms);
        set(&mut c.task_idle_ttl, self.task_idle_ttl_ms);
        set(&mut c.principal_select_ttl, self.principal_select_ttl_ms);
        set(&mut c.stateless_list_ttl, self.stateless_list_ttl_ms);
        if let Some(v) = self.stateless_tool_exposure {
            c.stateless_tool_exposure = v.into();
        }
        if let Some(v) = self.mcp_protocol_mode {
            c.mcp_protocol_mode = v.into();
        }
        if let Some(v) = self.max_listen_streams {
            c.max_listen_streams = v as usize;
        }
        if let Some(v) = self.max_listen_resources {
            c.max_listen_resources = v as usize;
        }
        if let Some(v) = self.max_task_handles {
            c.max_task_handles = v as usize;
        }
        Ok(c)
    }
}
