//! 客户端配置相关记录：[`ClientConfig`]、生命周期策略、唤醒描述、调用去重与 App 总览。

use app_mcp_native as native;

use crate::enums::{ClientKind, HeartbeatMode, LifecycleMode, Residency, WakeKind};

/// 客户端配置。可选字段为空时使用原生运行时的默认值。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ClientConfig {
    /// App 标识，`[a-z][a-z0-9-]{0,62}`。
    pub app_id: String,
    pub app_name: String,
    /// 为空时自动生成（每个进程一个）。
    #[uniffi(default = None)]
    pub instance_id: Option<String>,
    /// 为空时为 `Native`。
    #[uniffi(default = None)]
    pub client_kind: Option<ClientKind>,
    /// Host 端点：`unix:<绝对路径>`、`pipe:\\.\pipe\<名称>`、`ws://…` 或 `wss://…`（spec/protocol.md 第 1 节）。
    /// 为空时：环境变量 `APP_MCP_ENDPOINT` → 平台默认本地 IPC 端点 → `ws://127.0.0.1:7717`（Android / iOS）。
    #[uniffi(default = None)]
    pub host_url: Option<String>,
    #[uniffi(default = None)]
    pub app_version: Option<String>,
    #[uniffi(default = None)]
    pub instance_title: Option<String>,
    /// 之前配对得到的 token（见 `ClientListener.on_paired`）。
    #[uniffi(default = None)]
    pub token: Option<String>,
    /// 为空时读取环境变量 `APP_MCP_LAUNCH_TOKEN`。
    #[uniffi(default = None)]
    pub launch_token: Option<String>,
    /// 同时执行的调用上限。
    #[uniffi(default = 1)]
    pub max_concurrent_calls: u32,
    /// App 总览：握手时发给 Host，模型在会话中首次接触本 App 时由 Host 附带。
    #[uniffi(default = None)]
    pub overview: Option<AppOverview>,
    /// 生命周期策略（spec/lifecycle.md）。为空时为 `persistent`（不休眠）。
    #[uniffi(default = None)]
    pub lifecycle: Option<LifecyclePolicy>,
    /// 建立连接的超时（毫秒）。为空时为 5000。
    #[uniffi(default = None)]
    pub connect_timeout_ms: Option<u32>,
    /// 心跳策略（spec/lifecycle.md 第 11 节）。为空时为 `Auto`。
    #[uniffi(default = None)]
    pub heartbeat: Option<HeartbeatMode>,
    /// 调用去重（spec/protocol.md 3.3）。为空时保留 5 分钟、最多 64 条；任一字段为 0 关闭。
    #[uniffi(default = None)]
    pub call_dedup: Option<CallDedupPolicy>,
    /// 按名寻址（spec/naming.md）：`start` 后在系统名字服务登记本 App，Hub 按名拨入（进程未运行时由系统激活）。
    /// Linux：D-Bus 会话总线名 `dev.appmcp.App.<appId>`；Windows：命名管道 `\\.\pipe\appmcp-<用户 SID>-<appId>`；
    /// 两者都需先 `app-mcp-host app install` 登记。Android 不使用本字段（按名寻址经导出的 ToolsService，spec/naming.md 4.2），
    /// macOS / iOS 不支持：本平台不支持时经 `ClientListener.on_log` 报告，其余照常。通常与 `LifecycleMode::OnDemand` 同用。
    #[uniffi(default = false)]
    pub register_name: bool,
    /// 登记实例名（`[a-z][a-z0-9-]{0,31}`，不能是 `default`）：另登记 `dev.appmcp.App.<appId>.<instance>`
    /// （Windows 管道 `…-<appId>.<instance>`），供 `appmcp://<appId>/<instance>` 寻址。不合法时构造客户端返回配置错误。
    #[uniffi(default = None)]
    pub name_instance: Option<String>,
    /// 排队中的调用上限（spec/protocol.md 5.3）。为空时为 64；0 表示不限。超出时新调用以 `RATE_LIMITED` 拒绝。
    #[uniffi(default = None)]
    pub max_queued_calls: Option<u32>,
}

/// 调用去重策略（spec/protocol.md 3.3）：已开始执行的 `callId` 的首次结果在有效期内重放。任一字段为 0 关闭去重。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CallDedupPolicy {
    /// 首次结果的保留时长（毫秒）。
    #[uniffi(default = 300000)]
    pub ttl_ms: u64,
    /// 最多保留的结果数，超出时淘汰最早的。
    #[uniffi(default = 64)]
    pub max_entries: u32,
}

impl From<CallDedupPolicy> for native::CallDedupPolicy {
    fn from(p: CallDedupPolicy) -> Self {
        native::CallDedupPolicy { ttl_ms: p.ttl_ms, max_entries: p.max_entries as usize }
    }
}

/// 本实例的唤醒描述，随 `app/sleep` 上报。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WakeDescriptor {
    pub kind: WakeKind,
    /// 各 kind 的定位信息（scheme、组件名、D-Bus 名等）。
    #[uniffi(default = None)]
    pub target: Option<String>,
    /// 能否不把窗口带到前台就唤醒。
    #[uniffi(default = false)]
    pub background: bool,
}

impl From<WakeDescriptor> for native::WakeDescriptor {
    fn from(w: WakeDescriptor) -> Self {
        native::WakeDescriptor {
            kind: w.kind.into(),
            target: w.target,
            background: w.background,
        }
    }
}

/// 生命周期策略（spec/lifecycle.md 第 3 节）。为空的字段取原生默认值。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct LifecyclePolicy {
    /// 为空时为 `Persistent`。
    #[uniffi(default = None)]
    pub mode: Option<LifecycleMode>,
    /// 空闲多久进入休眠（`Idle` 模式）。
    #[uniffi(default = 60000)]
    pub idle_timeout_ms: u64,
    /// 可见性为 hidden / frozen 时的空闲时间。
    #[uniffi(default = 15000)]
    pub hidden_idle_timeout_ms: u64,
    /// `OnDemand` 模式下任务完成后保留连接的时间。
    #[uniffi(default = 10000)]
    pub grace_ms: u64,
    /// 为空时为 `Keep`。
    #[uniffi(default = None)]
    pub residency: Option<Residency>,
    /// 为空时不上报（Host 回退到清单 `launch`）。
    #[uniffi(default = None)]
    pub wake: Option<WakeDescriptor>,
    /// `Idle` / `OnDemand` 下连续多少次"Host 不在"后停止重连、进入 `Dormant`；0 = 一直重连（spec/lifecycle.md 第 11 节）。
    #[uniffi(default = 3)]
    pub host_absent_retries: u32,
    /// 回退到 4e 之前的定时器行为（串行租约、无限重连、双向心跳、调用后按空闲时长、任何订阅都阻止休眠）。
    #[uniffi(default = false)]
    pub legacy_timers: bool,
    /// 调用 / 资源读取后的合并窗口：处理过调用后空闲时长取 min(本值, 空闲时长)，之后是否在线只由 Host 租约决定
    /// （spec/lifecycle.md 第 13 节 B1）。
    #[uniffi(default = 2000)]
    pub merge_window_ms: u64,
    /// `Idle` / `OnDemand` 下进入后台（可见 → 隐藏 / 冻结）且空闲时立即休眠，不等租约（B4）。移动端建议开启。
    #[uniffi(default = false)]
    pub sleep_on_background: bool,
}

impl From<LifecyclePolicy> for native::LifecyclePolicy {
    fn from(p: LifecyclePolicy) -> Self {
        let d = native::LifecyclePolicy::default();
        native::LifecyclePolicy {
            mode: p.mode.map_or(d.mode, Into::into),
            idle_timeout_ms: p.idle_timeout_ms,
            hidden_idle_timeout_ms: p.hidden_idle_timeout_ms,
            grace_ms: p.grace_ms,
            residency: p.residency.map_or(d.residency, Into::into),
            wake: p.wake.map(Into::into),
            host_absent_retries: p.host_absent_retries,
            legacy_timers: p.legacy_timers,
            merge_window_ms: p.merge_window_ms,
            sleep_on_background: p.sleep_on_background,
        }
    }
}

/// 从操作系统激活参数 / URL 中提取唤醒令牌（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、
/// `#app-mcp-wake=<token>`）。不是唤醒参数时返回空。
#[uniffi::export]
pub fn parse_wake_token(args: String) -> Option<String> {
    native::parse_wake_token(&args)
}

/// App 总览（spec/protocol.md 第 7 节）。只描述能力，不授权。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppOverview {
    /// 一句话简介（≤ 100 字符）。
    pub summary: String,
    /// 总览正文（Markdown，≤ 2000 字符）。
    #[uniffi(default = None)]
    pub body: Option<String>,
    /// 语言，如 `zh-CN`。
    #[uniffi(default = None)]
    pub locale: Option<String>,
}

impl From<AppOverview> for native::AppOverview {
    fn from(o: AppOverview) -> Self {
        native::AppOverview {
            summary: o.summary,
            body: o.body,
            locale: o.locale,
        }
    }
}

impl From<ClientConfig> for native::NativeConfig {
    fn from(c: ClientConfig) -> Self {
        let mut n = native::NativeConfig::new(c.app_id, c.app_name);
        n.instance_id = c.instance_id;
        if let Some(kind) = c.client_kind {
            n.client_kind = kind.into();
        }
        if let Some(url) = c.host_url {
            n.host_url = url;
        }
        n.app_version = c.app_version;
        n.instance_title = c.instance_title;
        n.token = c.token;
        n.launch_token = c.launch_token;
        n.max_concurrent_calls = c.max_concurrent_calls;
        if let Some(q) = c.max_queued_calls {
            n.max_queued_calls = q;
        }
        n.overview = c.overview.map(Into::into);
        if let Some(lifecycle) = c.lifecycle {
            n.lifecycle = lifecycle.into();
        }
        if let Some(ms) = c.connect_timeout_ms {
            n.connect_timeout_ms = ms;
        }
        if let Some(h) = c.heartbeat {
            n.heartbeat = h.into();
        }
        if let Some(d) = c.call_dedup {
            n.call_dedup = d.into();
        }
        n.register_name = c.register_name;
        n.name_instance = c.name_instance;
        n
    }
}
