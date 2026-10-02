//! 配置目录与配置文件（`~/.app-mcp/config.json`）。
//!
//! 优先级：命令行参数 > 配置文件 > 默认值。配置目录：`--home` > 环境变量 `APP_MCP_HOME` > `~/.app-mcp`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use app_mcp_hub::{
    LeaseOverrides, LeasePolicy, LimitOverrides, LimitPolicy, OutputValidation, ToolExposure, UpstreamConfig, WakerConfig,
};
use serde::{Deserialize, Serialize};

/// 默认的 HTTP 监听地址：同一端口承载 `/app`（App 连接）、`/mcp`、`/healthz`。
pub const DEFAULT_LISTEN_ADDR: &str = app_mcp_protocol::DEFAULT_LISTEN_ADDR;
/// 关闭本地 IPC 服务时 `ipcEndpoint` / `--ipc-endpoint` 的取值。
pub const IPC_NONE: &str = "none";
/// 配置目录环境变量。
pub const HOME_ENV: &str = app_mcp_protocol::registry::HOME_ENV;

/// 配置目录（`~/.app-mcp`）及其中的固定文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppHome {
    pub dir: PathBuf,
}

impl AppHome {
    /// `--home` > `APP_MCP_HOME` > `~/.app-mcp`。结果为绝对路径。
    pub fn resolve(explicit: Option<&Path>) -> anyhow::Result<Self> {
        let dir = match explicit {
            Some(d) => d.to_path_buf(),
            None => match std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
                Some(d) => PathBuf::from(d),
                None => dirs::home_dir()
                    .context("无法确定用户主目录；请用 --home 或 APP_MCP_HOME 指定配置目录")?
                    .join(".app-mcp"),
            },
        };
        Ok(Self {
            dir: absolute(&dir)?,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.json")
    }
    pub fn token_file(&self) -> PathBuf {
        self.dir.join("token")
    }
    /// 策略规则（spec/hub-api.md 3.13）：启动时加载，`app-mcp-host policy reload` 重载。
    pub fn policy_file(&self) -> PathBuf {
        self.dir.join("policy.json")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
    pub fn manifest_dir(&self) -> PathBuf {
        self.dir.join("manifests")
    }
    /// 运行时目录：单实例锁 `hub.lock` 与登记文件 `endpoints.json`（spec/protocol.md 1.5、1.7）。
    pub fn run_dir(&self) -> PathBuf {
        app_mcp_protocol::registry::run_dir(&self.dir)
    }
    pub fn registry_file(&self) -> PathBuf {
        self.run_dir().join(app_mcp_protocol::registry::REGISTRY_FILE)
    }
    /// 持久状态目录（`HubConfig::state_dir`）：休眠记录 `dormant/<appId>.json`（spec/hub-api.md 3.5「持久化」）。
    pub fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }
}

/// 转为绝对路径（不要求存在），并展开开头的 `~/`。
pub fn absolute(p: &Path) -> anyhow::Result<PathBuf> {
    let p = expand_tilde(p);
    if p.is_absolute() {
        return Ok(p);
    }
    Ok(std::env::current_dir().context("无法读取当前目录")?.join(p))
}

fn expand_tilde(p: &Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_path_buf();
    };
    let rest = if s == "~" {
        Some("")
    } else {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    };
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

/// HTTP 端点的令牌策略（见 README「安全」）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// 默认：带 `Origin` 头的请求（浏览器）必须携带令牌；本地非浏览器客户端可不带。
    #[default]
    Browser,
    /// 所有请求都必须携带令牌（多用户共享的机器建议使用）。
    All,
    /// 不使用令牌（只做 Origin / Host 校验）。
    Off,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpSection {
    /// 已弃用（兼容期）：旧的独立 MCP 端口（如 `127.0.0.1:7718`）。设置时另开一个监听器提供同样的
    /// `/mcp`（启动时记录弃用提示）；MCP 已合并到 `listen` 的 `/mcp`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_remote: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LifecycleSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_timeout_ms: Option<u64>,
    /// 导航等待上限（毫秒，spec/hub-api.md 3.14 / 3.15），默认 5000（`HubConfig::navigate_timeout`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigate_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_from_launch: Option<bool>,
    /// 唤醒令牌有效期（毫秒），默认 60000。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_token_ttl_ms: Option<u64>,
    /// 每 App 每分钟最多唤醒次数，默认 6；0 不限。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_rate_limit: Option<u32>,
    /// 回退到旧心跳（Hub 对所有连接发 ping 并按无消息断开），默认 `false`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_heartbeat: Option<bool>,
    /// 自适应租约（spec/hub-api.md 3.5）：`{"adaptive","window","marginMs","minMs","maxMs","idleRevokeMs"}`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseOverrides>,
    /// `"system"`（默认）/ `"none"` / `{"exec": [program, ...args]}`（spec/hub-api.md 3.5）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waker: Option<WakerConfig>,
    /// 按名寻址（spec/naming.md）：经系统名字服务发现 App、调用时按名拨号（Linux：D-Bus 会话总线），默认 `false`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_service: Option<bool>,
    /// 按名拨入的通道在最后一次调用后保持的时间（毫秒，spec/naming.md 7.2），默认 15000。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_grace_ms: Option<u64>,
    /// 按需启动（spec/protocol.md 1.9）时空闲多久退出（毫秒），默认 600000；0 = 不退出。
    /// 只在监听套接字由 systemd / launchd 交来时生效（否则退出后没有谁再启动 Host）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_exit_ms: Option<u64>,
    /// 无会话 MCP 请求的 Agent 任务在请求流空闲多久后回收（毫秒，spec/hub-api.md 3.6），默认 600000；0 = 不因空闲回收。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_idle_ttl_ms: Option<u64>,
    /// 无会话请求的主体级 `apps.select` 选择的空闲有效期（毫秒，spec/hub-api.md 3.6），默认 60000；0 = 不单独过期。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_select_ttl_ms: Option<u64>,
}

/// 工具列表（spec/hub-api.md 3.7）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsSection {
    /// `"auto"`（默认）/ `"progressive"` / `"all"`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure: Option<ToolExposure>,
    /// `auto` 的阈值（App 与上游工具总数超过它时渐进暴露），默认 40。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<usize>,
    /// App 结果与其 `outputSchema` 不符时：`"log"`（默认）/ `"reject"` / `"off"`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_validation: Option<OutputValidation>,
    /// 调用进度转发给 Agent 的最小间隔（毫秒，spec/hub-api.md 3.12），默认 250；0 = 不合并。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_interval_ms: Option<u64>,
    /// 无会话 MCP 请求的工具暴露方式（spec/hub-api.md 3.7「无会话请求的列表与总览」）：`"all"`（默认）/ `"progressive"` / `"auto"`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stateless_exposure: Option<ToolExposure>,
    /// 无会话请求列表结果的缓存提示 `ttlMs`（毫秒，spec/hub-api.md 3.7），默认 5000。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stateless_list_ttl_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// 常驻模式是否写日志文件（`<home>/logs/app-mcp-host.log`），默认 `true`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<bool>,
    /// 单个日志文件上限（字节），超过后轮转；默认 5 MiB。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// 保留的历史文件数（`.1` … `.N`），默认 3。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep: Option<usize>,
}

/// `config.json` 的内容。所有字段都可省略；兼容旧的 `--config` 文件（只有 `upstreams`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileConfig {
    /// HTTP 监听地址（`/app`、`/mcp`、`/healthz`），默认 `127.0.0.1:7717`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// 已弃用：`listen` 的旧名（合并端口之前的 App 连接地址）。只设置它时按 `listen` 使用并记录提示；
    /// 与 `listen` 同时设置且不同时报错。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_addr: Option<String>,
    /// 本地 IPC 端点（`unix:<绝对路径>` / `pipe:\\.\pipe\<名称>`）；`"none"` 关闭；省略时为平台默认端点。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipc_endpoint: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub http: HttpSection,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manifests: Vec<PathBuf>,
    /// 清单目录；省略时为 `<home>/manifests`（不存在时忽略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_dirs: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_origins: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    #[serde(skip_serializing_if = "is_default")]
    pub lifecycle: LifecycleSection,
    #[serde(skip_serializing_if = "is_default")]
    pub tools: ToolsSection,
    /// 资源保护（spec/hub-api.md 3.11）：`{"toolRatePerMinute","toolRateBurst","appRatePerMinute","appRateBurst",
    /// "maxArgumentsBytes","maxResultBytes","maxResourceBytes"}`，缺省字段取默认值。
    #[serde(skip_serializing_if = "is_default")]
    pub limits: LimitOverrides,
    #[serde(skip_serializing_if = "is_default")]
    pub log: LogSection,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl FileConfig {
    /// 读取配置文件。`must_exist = false` 时文件不存在返回默认配置。
    pub fn load(path: &Path, must_exist: bool) -> anyhow::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if !must_exist && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(anyhow::anyhow!("读取配置 {} 失败：{e}", path.display())),
        };
        serde_json::from_str(&text).with_context(|| format!("解析配置 {} 失败", path.display()))
    }

    /// 写入配置文件（格式化 JSON），必要时创建目录。
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        std::fs::write(path, text).with_context(|| format!("写入配置 {} 失败", path.display()))
    }
}

/// 命令行给出的覆盖项（`None` / 空 = 未指定，沿用配置文件）。
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub listen: Option<String>,
    /// 已弃用的 `--ws-addr`（同 `wsAddr`）。
    pub ws_addr: Option<String>,
    pub ipc_endpoint: Option<String>,
    pub http_addr: Option<String>,
    pub http_allow_remote: Option<bool>,
    pub auth: Option<AuthMode>,
    pub manifests: Vec<PathBuf>,
    pub manifest_dirs: Vec<PathBuf>,
    pub allow_origins: Vec<String>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub lease_ms: Option<u64>,
    pub wake_timeout_ms: Option<u64>,
    pub navigate_timeout_ms: Option<u64>,
    pub wake_from_launch: Option<bool>,
    pub wake_token_ttl_ms: Option<u64>,
    pub wake_rate_limit: Option<u32>,
    pub legacy_heartbeat: Option<bool>,
    /// 自适应租约的命令行覆盖项（字段级合并到配置文件的 `lifecycle.lease`）。
    pub lease: LeaseOverrides,
    pub waker: Option<WakerConfig>,
    pub tool_exposure: Option<ToolExposure>,
    pub tool_exposure_threshold: Option<usize>,
    /// 资源保护的命令行覆盖项（字段级合并到配置文件的 `limits`）。
    pub limits: LimitOverrides,
    pub output_validation: Option<OutputValidation>,
    pub log_level: Option<String>,
    pub log_file: Option<bool>,
    pub name_service: Option<bool>,
    pub channel_grace_ms: Option<u64>,
    pub idle_exit_ms: Option<u64>,
    pub task_idle_ttl_ms: Option<u64>,
    pub principal_select_ttl_ms: Option<u64>,
    pub stateless_tool_exposure: Option<ToolExposure>,
}

impl FileConfig {
    /// 把命令行覆盖项写入配置（`service install` 用于持久化；`serve` 用于合并）。
    /// 标量覆盖；列表 / 映射追加（同名上游被覆盖）。路径转为绝对路径。
    pub fn apply(&mut self, o: &Overrides) -> anyhow::Result<()> {
        fn set<T: Clone>(dst: &mut Option<T>, src: &Option<T>) {
            if let Some(v) = src {
                *dst = Some(v.clone());
            }
        }
        if o.listen.is_some() {
            // 新名称取代旧名称：写回配置时迁移。
            self.ws_addr = None;
        }
        set(&mut self.listen, &o.listen);
        set(&mut self.ws_addr, &o.ws_addr);
        set(&mut self.ipc_endpoint, &o.ipc_endpoint);
        set(&mut self.http.addr, &o.http_addr);
        set(&mut self.http.allow_remote, &o.http_allow_remote);
        set(&mut self.http.auth, &o.auth);
        set(&mut self.lifecycle.lease_ms, &o.lease_ms);
        set(&mut self.lifecycle.wake_timeout_ms, &o.wake_timeout_ms);
        set(&mut self.lifecycle.navigate_timeout_ms, &o.navigate_timeout_ms);
        set(&mut self.lifecycle.wake_from_launch, &o.wake_from_launch);
        set(&mut self.lifecycle.wake_token_ttl_ms, &o.wake_token_ttl_ms);
        set(&mut self.lifecycle.wake_rate_limit, &o.wake_rate_limit);
        set(&mut self.lifecycle.legacy_heartbeat, &o.legacy_heartbeat);
        if o.lease != LeaseOverrides::default() {
            self.lifecycle.lease.get_or_insert_with(LeaseOverrides::default).merge(&o.lease);
        }
        set(&mut self.lifecycle.waker, &o.waker);
        set(&mut self.lifecycle.name_service, &o.name_service);
        set(&mut self.lifecycle.channel_grace_ms, &o.channel_grace_ms);
        set(&mut self.lifecycle.idle_exit_ms, &o.idle_exit_ms);
        set(&mut self.lifecycle.task_idle_ttl_ms, &o.task_idle_ttl_ms);
        set(&mut self.lifecycle.principal_select_ttl_ms, &o.principal_select_ttl_ms);
        set(&mut self.tools.stateless_exposure, &o.stateless_tool_exposure);
        set(&mut self.tools.exposure, &o.tool_exposure);
        set(&mut self.tools.threshold, &o.tool_exposure_threshold);
        set(&mut self.tools.output_validation, &o.output_validation);
        self.limits.merge(&o.limits);
        set(&mut self.log.level, &o.log_level);
        set(&mut self.log.file, &o.log_file);
        for m in &o.manifests {
            let m = absolute(m)?;
            if !self.manifests.contains(&m) {
                self.manifests.push(m);
            }
        }
        if !o.manifest_dirs.is_empty() {
            let dirs = o
                .manifest_dirs
                .iter()
                .map(|d| absolute(d))
                .collect::<anyhow::Result<Vec<_>>>()?;
            self.manifest_dirs = Some(dirs);
        }
        for origin in &o.allow_origins {
            if !self.allow_origins.contains(origin) {
                self.allow_origins.push(origin.clone());
            }
        }
        for (name, cfg) in &o.upstreams {
            self.upstreams.insert(name.clone(), cfg.clone());
        }
        Ok(())
    }
}

/// 合并后的最终设置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// HTTP 监听地址（`/app`、`/mcp`、`/healthz`）。
    pub listen: String,
    /// `listen` 是否为显式配置：显式时只绑定该地址；缺省时默认端口被占用会依次尝试备选端口 7737、7757。
    pub listen_explicit: bool,
    /// 兼容期的额外 MCP 监听地址（已弃用的 `http.addr` / `--http`）；`None` = 不开（默认）。
    pub compat_http_addr: Option<String>,
    /// 解析配置时产生的提示（弃用的配置项等），日志初始化后记录。
    pub notices: Vec<String>,
    /// 本地 IPC 端点；`None` = 关闭（配置为 `"none"`，或本平台没有默认端点）。
    pub ipc_endpoint: Option<String>,
    pub http_allow_remote: bool,
    pub auth: AuthMode,
    pub manifests: Vec<PathBuf>,
    /// `(目录, 是否必须存在)`。显式配置的目录必须存在；默认目录不存在时忽略。
    pub manifest_dirs: Vec<(PathBuf, bool)>,
    pub allow_origins: Vec<String>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub lease_ms: u64,
    pub wake_timeout_ms: u64,
    pub navigate_timeout_ms: u64,
    pub wake_from_launch: bool,
    pub wake_token_ttl_ms: u64,
    pub wake_rate_limit: u32,
    pub legacy_heartbeat: bool,
    pub lease: LeasePolicy,
    pub waker: WakerConfig,
    /// 按名寻址（Linux：D-Bus 会话总线连接器）。
    pub name_service: bool,
    pub channel_grace_ms: u64,
    /// 按需启动时的空闲退出时间（毫秒）；0 = 不退出。
    pub idle_exit_ms: u64,
    /// 无会话 Agent 任务的空闲回收时长（毫秒）；0 = 不回收。
    pub task_idle_ttl_ms: u64,
    /// 主体级 `apps.select` 的空闲有效期（毫秒）；0 = 不单独过期。
    pub principal_select_ttl_ms: u64,
    pub tool_exposure: ToolExposure,
    pub tool_exposure_threshold: usize,
    pub limits: LimitPolicy,
    pub output_validation: OutputValidation,
    pub progress_interval_ms: u64,
    pub stateless_tool_exposure: ToolExposure,
    pub stateless_list_ttl_ms: u64,
    pub log_level: String,
    pub log_file: bool,
    pub log_max_bytes: u64,
    pub log_keep: usize,
}

impl Settings {
    /// 配置文件 + 命令行覆盖 → 最终设置。
    pub fn resolve(file: &FileConfig, o: &Overrides, home: &AppHome) -> anyhow::Result<Self> {
        let mut c = file.clone();
        c.apply(o)?;
        let manifest_dirs = match c.manifest_dirs {
            Some(dirs) => dirs
                .iter()
                .map(|d| absolute(d).map(|d| (d, true)))
                .collect::<anyhow::Result<_>>()?,
            None => vec![(home.manifest_dir(), false)],
        };
        let manifests = c
            .manifests
            .iter()
            .map(|m| absolute(m))
            .collect::<anyhow::Result<_>>()?;
        let ipc_endpoint = match c.ipc_endpoint.as_deref() {
            Some(IPC_NONE) => None,
            Some(text) => {
                let e = app_mcp_protocol::Endpoint::parse(text).map_err(anyhow::Error::msg)?;
                anyhow::ensure!(
                    e.is_ipc(),
                    "ipcEndpoint 必须是 unix:<绝对路径> 或 pipe:\\\\.\\pipe\\<名称>（或 none）：{text}"
                );
                Some(e.to_string())
            }
            None => app_mcp_protocol::endpoint::default_ipc_endpoint().map(|e| e.to_string()),
        };
        let mut notices = Vec::new();
        let listen = match (c.listen, c.ws_addr) {
            (Some(l), Some(w)) if l != w => anyhow::bail!(
                "配置同时设置了 listen（{l}）与已弃用的 wsAddr / --ws-addr（{w}）：请只保留 listen"
            ),
            (Some(l), _) => Some(l),
            (None, Some(w)) => {
                notices.push(format!(
                    "wsAddr / --ws-addr 已弃用，按 listen = {w} 使用：App 连接、MCP 与 /healthz 已合并到同一端口（listen）"
                ));
                Some(w)
            }
            (None, None) => None,
        };
        let mut lease = LeasePolicy::default();
        if let Some(o) = &c.lifecycle.lease {
            o.apply(&mut lease);
        }
        lease.validate().map_err(|e| anyhow::anyhow!("lifecycle.lease 无效：{e}"))?;
        let mut limits = LimitPolicy::default();
        c.limits.apply(&mut limits);
        limits.validate().map_err(|e| anyhow::anyhow!("配置无效：{e}"))?;
        let listen_explicit = listen.is_some();
        let listen = listen.unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned());
        let compat_http_addr = c.http.addr.filter(|a| *a != listen);
        if let Some(a) = &compat_http_addr {
            notices.push(format!(
                "http.addr / --http 已弃用：MCP 已合并到 http://{listen}/mcp；兼容期内另在 {a} 提供同样的服务，请改用 listen 并更新 MCP 客户端配置"
            ));
        }
        Ok(Self {
            listen,
            listen_explicit,
            compat_http_addr,
            notices,
            ipc_endpoint,
            http_allow_remote: c.http.allow_remote.unwrap_or(false),
            auth: c.http.auth.unwrap_or_default(),
            manifests,
            manifest_dirs,
            allow_origins: c.allow_origins,
            upstreams: c.upstreams,
            lease_ms: c.lifecycle.lease_ms.unwrap_or(60_000),
            wake_timeout_ms: c.lifecycle.wake_timeout_ms.unwrap_or(15_000),
            navigate_timeout_ms: c
                .lifecycle
                .navigate_timeout_ms
                .unwrap_or(app_mcp_hub::DEFAULT_NAVIGATE_TIMEOUT.as_millis() as u64),
            wake_from_launch: c.lifecycle.wake_from_launch.unwrap_or(false),
            wake_token_ttl_ms: c.lifecycle.wake_token_ttl_ms.unwrap_or(60_000),
            wake_rate_limit: c.lifecycle.wake_rate_limit.unwrap_or(app_mcp_hub::DEFAULT_WAKE_RATE_LIMIT),
            legacy_heartbeat: c.lifecycle.legacy_heartbeat.unwrap_or(false),
            lease,
            waker: c.lifecycle.waker.unwrap_or_default(),
            name_service: c.lifecycle.name_service.unwrap_or(false),
            channel_grace_ms: c
                .lifecycle
                .channel_grace_ms
                .unwrap_or(app_mcp_hub::DEFAULT_CHANNEL_GRACE.as_millis() as u64),
            idle_exit_ms: c.lifecycle.idle_exit_ms.unwrap_or(crate::activation::DEFAULT_IDLE_EXIT_MS),
            task_idle_ttl_ms: c
                .lifecycle
                .task_idle_ttl_ms
                .unwrap_or(app_mcp_hub::DEFAULT_TASK_IDLE_TTL.as_millis() as u64),
            principal_select_ttl_ms: c
                .lifecycle
                .principal_select_ttl_ms
                .unwrap_or(app_mcp_hub::DEFAULT_PRINCIPAL_SELECT_TTL.as_millis() as u64),
            tool_exposure: c.tools.exposure.unwrap_or_default(),
            tool_exposure_threshold: c
                .tools
                .threshold
                .unwrap_or(app_mcp_hub::DEFAULT_TOOL_EXPOSURE_THRESHOLD),
            limits,
            output_validation: c.tools.output_validation.unwrap_or_default(),
            progress_interval_ms: c
                .tools
                .progress_interval_ms
                .unwrap_or(app_mcp_hub::DEFAULT_PROGRESS_INTERVAL.as_millis() as u64),
            stateless_tool_exposure: c
                .tools
                .stateless_exposure
                .unwrap_or_else(|| app_mcp_hub::HubConfig::default().stateless_tool_exposure),
            stateless_list_ttl_ms: c
                .tools
                .stateless_list_ttl_ms
                .unwrap_or(app_mcp_hub::DEFAULT_STATELESS_LIST_TTL.as_millis() as u64),
            log_level: c.log.level.unwrap_or_else(|| "info".to_owned()),
            log_file: c.log.file.unwrap_or(true),
            log_max_bytes: c.log.max_bytes.unwrap_or(5 * 1024 * 1024),
            log_keep: c.log.keep.unwrap_or(3),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> PathBuf {
        absolute(Path::new(p)).unwrap()
    }

    fn home() -> AppHome {
        AppHome {
            dir: PathBuf::from("/h/.app-mcp"),
        }
    }

    #[test]
    fn name_service_overrides() {
        let o = Overrides { name_service: Some(true), channel_grace_ms: Some(500), ..Default::default() };
        let s = Settings::resolve(&FileConfig::default(), &o, &home()).unwrap();
        assert!(s.name_service);
        assert_eq!(s.channel_grace_ms, 500);
        let file: FileConfig =
            serde_json::from_str(r#"{"lifecycle":{"nameService":true,"channelGraceMs":2000}}"#).unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
        assert!(s.name_service);
        assert_eq!(s.channel_grace_ms, 2000);
    }

    #[test]
    fn stateless_settings_from_file_and_cli() {
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
        let hub = app_mcp_hub::HubConfig::default();
        assert_eq!(
            (s.task_idle_ttl_ms, s.principal_select_ttl_ms, s.stateless_tool_exposure, s.stateless_list_ttl_ms),
            (
                hub.task_idle_ttl.as_millis() as u64,
                hub.principal_select_ttl.as_millis() as u64,
                hub.stateless_tool_exposure,
                hub.stateless_list_ttl.as_millis() as u64
            ),
            "默认值与 HubConfig 一致"
        );
        let file: FileConfig = serde_json::from_str(
            r#"{"lifecycle":{"taskIdleTtlMs":0,"principalSelectTtlMs":1500},
                "tools":{"statelessExposure":"progressive","statelessListTtlMs":0}}"#,
        )
        .unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
        assert_eq!(
            (s.task_idle_ttl_ms, s.principal_select_ttl_ms, s.stateless_tool_exposure, s.stateless_list_ttl_ms),
            (0, 1500, ToolExposure::Progressive, 0)
        );
        // 命令行覆盖；service install 持久化为配置文件的键
        let o = Overrides {
            task_idle_ttl_ms: Some(2000),
            principal_select_ttl_ms: Some(0),
            stateless_tool_exposure: Some(ToolExposure::Auto),
            ..Default::default()
        };
        let s = Settings::resolve(&file, &o, &home()).unwrap();
        assert_eq!(
            (s.task_idle_ttl_ms, s.principal_select_ttl_ms, s.stateless_tool_exposure, s.stateless_list_ttl_ms),
            (2000, 0, ToolExposure::Auto, 0)
        );
        let mut f = FileConfig::default();
        f.apply(&o).unwrap();
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["lifecycle"], serde_json::json!({"taskIdleTtlMs": 2000, "principalSelectTtlMs": 0}));
        assert_eq!(v["tools"], serde_json::json!({"statelessExposure": "auto"}));
        // 类型不对：明确报错
        assert!(serde_json::from_str::<FileConfig>(r#"{"tools":{"statelessExposure":"some"}}"#).is_err());
        assert!(serde_json::from_str::<FileConfig>(r#"{"lifecycle":{"taskIdleTtlMs":-1}}"#).is_err());
    }

    #[test]
    fn idle_exit_from_file_and_override() {
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
        assert_eq!(s.idle_exit_ms, crate::activation::DEFAULT_IDLE_EXIT_MS);
        let mut file: FileConfig = serde_json::from_str(r#"{"lifecycle":{"idleExitMs":0}}"#).unwrap();
        assert_eq!(Settings::resolve(&file, &Overrides::default(), &home()).unwrap().idle_exit_ms, 0);
        let o = Overrides { idle_exit_ms: Some(1500), ..Default::default() };
        assert_eq!(Settings::resolve(&file, &o, &home()).unwrap().idle_exit_ms, 1500);
        // service install 持久化覆盖项
        file.apply(&o).unwrap();
        assert!(serde_json::to_string(&file).unwrap().contains(r#""idleExitMs":1500"#));
    }

    #[test]
    fn defaults() {
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
        assert_eq!(s.listen, "127.0.0.1:7717");
        assert!(!s.listen_explicit);
        assert_eq!(s.compat_http_addr, None, "旧 MCP 端口默认不开");
        assert!(s.notices.is_empty());
        assert_eq!(s.auth, AuthMode::Browser);
        assert_eq!(
            s.manifest_dirs,
            vec![(PathBuf::from("/h/.app-mcp/manifests"), false)]
        );
        assert!(s.log_file);
        assert!(!s.name_service, "按名寻址默认关闭");
        assert_eq!(s.channel_grace_ms, 15_000);
        assert_eq!(s.lease_ms, 60_000);
        assert_eq!(
            (s.wake_token_ttl_ms, s.wake_rate_limit, s.legacy_heartbeat),
            (60_000, app_mcp_hub::DEFAULT_WAKE_RATE_LIMIT, false)
        );
        assert_eq!(s.waker, WakerConfig::System);
        assert_eq!(s.tool_exposure, ToolExposure::Auto);
        assert_eq!(s.tool_exposure_threshold, 40);
        assert_eq!(
            s.ipc_endpoint,
            app_mcp_protocol::endpoint::default_ipc_endpoint().map(|e| e.to_string())
        );
    }

    #[test]
    fn ipc_endpoint_config() {
        let resolve = |file: &str, cli: Option<&str>| {
            let f: FileConfig = serde_json::from_str(file).unwrap();
            let o = Overrides {
                ipc_endpoint: cli.map(str::to_owned),
                ..Default::default()
            };
            Settings::resolve(&f, &o, &home()).map(|s| s.ipc_endpoint)
        };
        assert_eq!(
            resolve(r#"{"ipcEndpoint": "unix:/run/x/hub.sock"}"#, None).unwrap().as_deref(),
            Some("unix:/run/x/hub.sock")
        );
        assert_eq!(
            resolve(r#"{"ipcEndpoint": "unix:/run/x/hub.sock"}"#, Some(r"pipe:\\.\pipe\p")).unwrap().as_deref(),
            Some(r"pipe:\\.\pipe\p")
        );
        assert_eq!(resolve(r#"{"ipcEndpoint": "none"}"#, None).unwrap(), None);
        assert_eq!(resolve("{}", Some("none")).unwrap(), None);
        assert!(resolve(r#"{"ipcEndpoint": "ws://127.0.0.1:1"}"#, None).is_err());
        assert!(resolve("{}", Some("hub.sock")).is_err());
    }

    #[test]
    fn parse_full_and_legacy() {
        let full: FileConfig = serde_json::from_str(
            r#"{
              "listen": "127.0.0.1:9000",
              "http": { "addr": "127.0.0.1:9001", "auth": "all" },
              "manifests": ["/m/a.json"],
              "manifestDirs": ["/m"],
              "allowOrigins": ["https://app.example.com"],
              "upstreams": { "files": { "command": "npx", "args": ["x"] } },
              "lifecycle": { "leaseMs": 500, "wakeTimeoutMs": 2000, "navigateTimeoutMs": 3000, "wakeFromLaunch": true,
                             "waker": { "exec": ["node", "wake.mjs"] } },
              "tools": { "exposure": "progressive", "threshold": 10 },
              "log": { "level": "debug", "file": false, "maxBytes": 1024, "keep": 1 }
            }"#,
        )
        .unwrap();
        let s = Settings::resolve(&full, &Overrides::default(), &home()).unwrap();
        assert_eq!(s.listen, "127.0.0.1:9000");
        assert!(s.listen_explicit);
        // 旧的独立 MCP 端口：兼容期内显式配置才另开，并记录弃用提示
        assert_eq!(s.compat_http_addr.as_deref(), Some("127.0.0.1:9001"));
        assert_eq!(s.notices.len(), 1);
        assert_eq!(s.auth, AuthMode::All);
        // Windows 上 "/m" 不是绝对路径，会接到当前目录（盘符）上
        assert_eq!(s.manifest_dirs, vec![(abs("/m"), true)]);
        assert_eq!(s.upstreams["files"].command, "npx");
        assert_eq!(
            (s.lease_ms, s.wake_timeout_ms, s.navigate_timeout_ms, s.wake_from_launch),
            (500, 2000, 3000, true)
        );
        assert_eq!(
            s.waker,
            WakerConfig::Exec(vec!["node".into(), "wake.mjs".into()])
        );
        assert_eq!(
            (s.tool_exposure, s.tool_exposure_threshold),
            (ToolExposure::Progressive, 10)
        );
        assert_eq!(
            (
                s.log_level.as_str(),
                s.log_file,
                s.log_max_bytes,
                s.log_keep
            ),
            ("debug", false, 1024, 1)
        );

        // 旧的 --config 文件（只有 upstreams）仍可解析
        let legacy: FileConfig =
            serde_json::from_str(r#"{"upstreams":{"e":{"command":"echo"}}}"#).unwrap();
        assert_eq!(legacy.upstreams.len(), 1);
    }

    #[test]
    fn cli_overrides_file() {
        let file: FileConfig = serde_json::from_str(
            r#"{"listen":"127.0.0.1:9000","manifests":["/m/a.json"],"lifecycle":{"leaseMs":500}}"#,
        )
        .unwrap();
        let o = Overrides {
            listen: Some("127.0.0.1:1".into()),
            manifests: vec![PathBuf::from("/m/b.json")],
            auth: Some(AuthMode::Off),
            ..Default::default()
        };
        let s = Settings::resolve(&file, &o, &home()).unwrap();
        assert_eq!(s.listen, "127.0.0.1:1");
        assert_eq!(s.manifests, vec![abs("/m/a.json"), abs("/m/b.json")]);
        assert_eq!(s.lease_ms, 500);
        assert_eq!(s.auth, AuthMode::Off);
    }

    #[test]
    fn power_settings_from_file_and_cli() {
        let file: FileConfig = serde_json::from_str(
            r#"{"lifecycle":{"wakeTokenTtlMs":30000,"wakeRateLimit":2,"legacyHeartbeat":true}}"#,
        )
        .unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
        assert_eq!((s.wake_token_ttl_ms, s.wake_rate_limit, s.legacy_heartbeat), (30_000, 2, true));
        let o = Overrides { wake_token_ttl_ms: Some(5_000), wake_rate_limit: Some(0), ..Default::default() };
        let s = Settings::resolve(&file, &o, &home()).unwrap();
        assert_eq!((s.wake_token_ttl_ms, s.wake_rate_limit, s.legacy_heartbeat), (5_000, 0, true));
        // service install 写入配置：命令行覆盖项持久化为 lifecycle 下的键
        let mut f = FileConfig::default();
        f.apply(&Overrides {
            wake_token_ttl_ms: Some(1_000),
            wake_rate_limit: Some(3),
            legacy_heartbeat: Some(true),
            ..Default::default()
        })
        .unwrap();
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(
            v["lifecycle"],
            serde_json::json!({"wakeTokenTtlMs": 1000, "wakeRateLimit": 3, "legacyHeartbeat": true})
        );
    }

    #[test]
    fn lease_settings_from_file_and_cli() {
        let file: FileConfig =
            serde_json::from_str(r#"{"lifecycle":{"lease":{"window":8,"maxMs":30000}}}"#).unwrap();
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
        assert_eq!(s.lease, LeasePolicy::default());
        let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
        assert_eq!((s.lease.window, s.lease.max), (8, std::time::Duration::from_secs(30)));
        // 命令行按字段覆盖
        let o = Overrides {
            lease: LeaseOverrides { adaptive: Some(false), window: Some(3), ..Default::default() },
            ..Default::default()
        };
        let s = Settings::resolve(&file, &o, &home()).unwrap();
        assert_eq!(
            (s.lease.adaptive, s.lease.window, s.lease.max),
            (false, 3, std::time::Duration::from_secs(30))
        );
        let mut f = file.clone();
        f.apply(&o).unwrap();
        assert_eq!(
            serde_json::to_value(&f).unwrap()["lifecycle"]["lease"],
            serde_json::json!({"adaptive": false, "window": 3, "maxMs": 30000})
        );
        // 不合法：明确报错
        let bad: FileConfig = serde_json::from_str(r#"{"lifecycle":{"lease":{"minMs":9000,"maxMs":1000}}}"#).unwrap();
        let e = Settings::resolve(&bad, &Overrides::default(), &home()).unwrap_err().to_string();
        assert!(e.contains("lifecycle.lease"), "{e}");
        assert!(serde_json::from_str::<FileConfig>(r#"{"lifecycle":{"lease":{"bogus":1}}}"#).is_err());
    }

    #[test]
    fn limit_settings_from_file_and_cli() {
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
        assert_eq!((s.limits.clone(), s.output_validation), (LimitPolicy::default(), OutputValidation::Log));
        let file: FileConfig = serde_json::from_str(
            r#"{"limits":{"toolRatePerMinute":10,"toolRateBurst":2,"maxResultBytes":0},"tools":{"outputValidation":"reject"}}"#,
        )
        .unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
        assert_eq!((s.limits.tool_rate.per_minute, s.limits.tool_rate.burst, s.limits.max_result_bytes), (10, 2, 0));
        assert_eq!(s.output_validation, OutputValidation::Reject);
        assert_eq!(s.progress_interval_ms, 250, "默认进度间隔");
        let no_merge: FileConfig = serde_json::from_str(r#"{"tools":{"progressIntervalMs":0}}"#).unwrap();
        assert_eq!(Settings::resolve(&no_merge, &Overrides::default(), &home()).unwrap().progress_interval_ms, 0);
        // 命令行按字段覆盖，写回配置时合并
        let o = Overrides {
            limits: LimitOverrides { tool_rate_burst: Some(5), app_rate_per_minute: Some(0), ..Default::default() },
            output_validation: Some(OutputValidation::Off),
            ..Default::default()
        };
        let s = Settings::resolve(&file, &o, &home()).unwrap();
        assert_eq!((s.limits.tool_rate.per_minute, s.limits.tool_rate.burst), (10, 5));
        assert!(s.limits.app_rate.is_unlimited());
        assert_eq!(s.output_validation, OutputValidation::Off);
        let mut f = file.clone();
        f.apply(&o).unwrap();
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(
            v["limits"],
            serde_json::json!({"toolRatePerMinute": 10, "toolRateBurst": 5, "appRatePerMinute": 0, "maxResultBytes": 0})
        );
        assert_eq!(v["tools"]["outputValidation"], "off");
        // 不合法：明确报错
        let bad: FileConfig = serde_json::from_str(r#"{"limits":{"appRateBurst":0}}"#).unwrap();
        let e = Settings::resolve(&bad, &Overrides::default(), &home()).unwrap_err().to_string();
        assert!(e.contains("appRateBurst"), "{e}");
        assert!(serde_json::from_str::<FileConfig>(r#"{"limits":{"bogus":1}}"#).is_err());
    }

    #[test]
    fn deprecated_ws_addr_and_http_addr() {
        let resolve = |file: &str, o: Overrides| {
            let f: FileConfig = serde_json::from_str(file).unwrap();
            Settings::resolve(&f, &o, &home())
        };
        // 只有旧名 wsAddr：按 listen 使用，带提示
        let s = resolve(r#"{"wsAddr":"127.0.0.1:9000"}"#, Overrides::default()).unwrap();
        assert_eq!((s.listen.as_str(), s.listen_explicit), ("127.0.0.1:9000", true));
        assert_eq!(s.notices.len(), 1);
        // 新旧同时设置且不同：报错；相同：接受
        assert!(resolve(r#"{"wsAddr":"127.0.0.1:9000","listen":"127.0.0.1:9001"}"#, Overrides::default()).is_err());
        assert!(resolve(r#"{"wsAddr":"127.0.0.1:9000","listen":"127.0.0.1:9000"}"#, Overrides::default()).is_ok());
        // 命令行 --listen 取代文件中的旧名（写回时迁移）
        let o = Overrides { listen: Some("127.0.0.1:1".into()), ..Default::default() };
        let mut f: FileConfig = serde_json::from_str(r#"{"wsAddr":"127.0.0.1:9000"}"#).unwrap();
        f.apply(&o).unwrap();
        assert_eq!((f.listen.as_deref(), f.ws_addr.as_deref()), (Some("127.0.0.1:1"), None));
        // http.addr 与 listen 相同：不另开监听
        let s = resolve(r#"{"listen":"127.0.0.1:9000","http":{"addr":"127.0.0.1:9000"}}"#, Overrides::default()).unwrap();
        assert_eq!(s.compat_http_addr, None);
    }

    #[test]
    fn save_roundtrip_skips_empty() {
        let dir = std::env::temp_dir().join(format!("app-mcp-cfg-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut c = FileConfig::default();
        c.apply(&Overrides {
            listen: Some("127.0.0.1:7717".into()),
            ..Default::default()
        })
        .unwrap();
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\n  \"listen\": \"127.0.0.1:7717\"\n}\n");
        assert_eq!(FileConfig::load(&path, true).unwrap(), c);
        assert_eq!(
            FileConfig::load(&dir.join("missing.json"), false).unwrap(),
            FileConfig::default()
        );
        assert!(FileConfig::load(&dir.join("missing.json"), true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
