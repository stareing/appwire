//! 配置文件 `config.json` 的结构（各分节）与读写。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use app_mcp_hub::{
    LeaseOverrides, LimitOverrides, McpProtocolMode, OutputValidation, ToolExposure, UpstreamConfig,
    WakerConfig,
};
use serde::{Deserialize, Serialize};

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

/// MCP 出口（spec/hub-api.md 3.6「协议版本」「通知」）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpSection {
    /// 协商的协议版本范围：`"auto"`（默认，可协商 2026-07-28）/ `"legacyOnly"`（回退开关，只声明到 2025-11-25）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_mode: Option<McpProtocolMode>,
    /// 每个主体同时打开的 `subscriptions/listen` 流数上限，默认 16；0 = 不提供 listen。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_listen_streams: Option<usize>,
    /// 一个 listen 流接受的资源 URI 数上限，默认 256。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_listen_resources: Option<usize>,
    /// 每个主体同时存在的任务句柄数上限（spec/hub-api.md 3.6「任务句柄」），默认 32；0 = 不提供任务句柄。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_task_handles: Option<usize>,
    /// 每个任务同时持有的对象锁数上限（spec/hub-api.md 3.6「对象锁」），默认 16；0 = 不提供对象锁。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_locks: Option<usize>,
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
    #[serde(skip_serializing_if = "is_default")]
    pub mcp: McpSection,
    /// 资源保护（spec/hub-api.md 3.11）：`{"toolRatePerMinute","toolRateBurst","appRatePerMinute","appRateBurst",
    /// "maxArgumentsBytes","maxResultBytes","maxResourceBytes"}`，缺省字段取默认值。
    #[serde(skip_serializing_if = "is_default")]
    pub limits: LimitOverrides,
    /// 只读结果缓存的上限（spec/hub-api.md 3.20）：`{"maxEntries","maxBytes","maxEntryBytes"}`，缺省字段取默认值。
    #[serde(skip_serializing_if = "is_default")]
    pub result_cache: super::ResultCacheSection,
    /// 撤销记录的上限（spec/hub-api.md 3.23）：`{"ttlMs","maxPerTask"}`，缺省字段取默认值；`maxPerTask: 0` 关闭撤销。
    #[serde(skip_serializing_if = "is_default")]
    pub undo: super::UndoSection,
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
