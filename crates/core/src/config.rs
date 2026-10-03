//! 客户端配置与各项策略。

use super::*;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ClientConfig {
    pub app_id: String,
    pub app_name: String,
    /// 每个标签页或进程唯一，页面刷新后应保持不变（由驱动层负责持久化）。
    pub instance_id: String,
    pub client_kind: ClientKind,
    pub sdk_version: String,
    pub app_version: Option<String>,
    pub origin: Option<String>,
    pub instance_title: Option<String>,
    pub instance_url: Option<String>,
    /// 之前配对得到的 token（由驱动层持久化，见 [`Event::Paired`]）。
    pub token: Option<String>,
    /// 由 Host 唤醒时携带的一次性 token。
    pub launch_token: Option<String>,
    /// App 总览，握手时发送给 Host，由 Host 在模型首次接触该 App 时附带。
    pub overview: Option<AppOverview>,
    pub reconnect: ReconnectPolicy,
    pub heartbeat: HeartbeatPolicy,
    /// 同时执行的调用上限；超出的调用按到达顺序排队。默认 1（串行）。
    pub max_concurrent_calls: usize,
    /// 排队中的调用上限（B-07）：新到的调用需要排队且队列已满时以 `RATE_LIMITED`（`details.scope = "queue"`）拒绝。
    /// 默认 [`DEFAULT_MAX_QUEUED_CALLS`]；0 表示不限。
    pub max_queued_calls: usize,
    /// App 声明"用户正在操作"（[`Client::set_busy`]）期间写调用的处理方式（spec/protocol.md 5.3）。默认 [`BusyPolicy::Reject`]。
    pub busy_policy: BusyPolicy,
    /// 同一资源两次 `resources/updated` 通知之间的最小间隔。默认 100ms。
    pub resource_update_throttle_ms: Millis,
    /// 发送 `app/hello` 后等待结果的最长时间，超时断开并重连。默认 10s；0 表示不限。
    pub handshake_timeout_ms: Millis,
    /// 生命周期策略（spec/lifecycle.md 第 3 节）。默认 `persistent`（不休眠）。
    pub lifecycle: LifecyclePolicy,
    /// 期望的 Host 用户（spec/protocol.md 1.6）：握手结果的 `user` 与之不同时进入
    /// [`ConnectionState::HostMismatch`]。`None`（默认）不核对用户（网页、Android / iOS）；
    /// 原生运行时填入 [`app_mcp_protocol::identity::expected_host_user`]。
    pub expected_host_user: Option<String>,
    /// 到 Host 的传输类别（spec/lifecycle.md 第 11 节），由驱动层按端点判定
    /// （[`app_mcp_protocol::Endpoint::transport_kind`]）。[`HeartbeatMode::Auto`] 下本地传输（IPC / 本机回环）不发心跳。
    /// 默认 [`TransportKind::Unknown`]（按远程处理）。
    pub transport: TransportKind,
    /// 调用去重（spec/protocol.md 3.3）：同一 `callId` 在有效期内只执行一次、重复请求得到首次结果。
    /// 默认保留 5 分钟、最多 64 条；[`CallDedupPolicy::OFF`] 关闭。
    pub call_dedup: CallDedupPolicy,
    /// 能处理 Host 的 `app/navigate`（spec/protocol.md 3.4）：握手时声明 `capabilities.navigate`，收到导航请求时产生
    /// [`Event::Navigate`]。默认 `false`：不声明，收到的导航请求直接以 `NAVIGATION_FAILED`（`unsupported`）回复。
    /// 驱动层在 App 设置了导航回调时置为 `true`（[`Client::set_navigation`]）。
    pub navigation: bool,
    /// 实例不可见（[`Visibility::Hidden`] / [`Visibility::Frozen`]）时导航请求是否仍交给导航回调（spec/protocol.md 3.4）。
    /// 默认 `false`：直接以 `USER_ACTION_REQUIRED`（`reason: "foreground"`）回复、不产生 [`Event::Navigate`]——平台不允许
    /// App 自行回到前台（Android 10+、iOS、浏览器标签页）时不必等到超时。能把自己的窗口带到前台的平台（桌面）或要自行处理
    /// 后台导航的 App（如发通知请用户点开）由驱动层 / App 置为 `true`（[`Client::set_navigate_in_background`]）。
    pub navigate_in_background: bool,
    /// 本进程由系统名字服务激活启动（spec/naming.md 4.1：D-Bus 激活文件的 `Exec` 带 `--app-mcp-activation`）。
    /// 与配置了 `launch_token` 一样算"由唤醒冷启动"，影响 [`Residency::ExitWhenIdle`]。默认 `false`。
    pub launched_by_activation: bool,
}

/// [`ClientConfig::max_queued_calls`] 的默认值。
///
/// @why 64：串行（默认并发 1）时足以容纳一个 Agent 的突发批量调用；再多通常意味着 App 卡住或调用方失控，尽早拒绝比排到超时更好。
pub const DEFAULT_MAX_QUEUED_CALLS: usize = 64;

impl ClientConfig {
    /// 其余字段取默认值。
    pub fn new(
        app_id: impl Into<String>,
        app_name: impl Into<String>,
        instance_id: impl Into<String>,
        client_kind: ClientKind,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            app_name: app_name.into(),
            instance_id: instance_id.into(),
            client_kind,
            sdk_version: env!("CARGO_PKG_VERSION").to_owned(),
            app_version: None,
            origin: None,
            instance_title: None,
            instance_url: None,
            token: None,
            launch_token: None,
            overview: None,
            reconnect: ReconnectPolicy::default(),
            heartbeat: HeartbeatPolicy::default(),
            max_concurrent_calls: 1,
            max_queued_calls: DEFAULT_MAX_QUEUED_CALLS,
            busy_policy: BusyPolicy::Reject,
            resource_update_throttle_ms: 100,
            handshake_timeout_ms: 10_000,
            lifecycle: LifecyclePolicy::default(),
            expected_host_user: None,
            transport: TransportKind::Unknown,
            call_dedup: CallDedupPolicy::default(),
            navigation: false,
            navigate_in_background: false,
            launched_by_activation: false,
        }
    }
}

/// 用户正在操作（[`Client::set_busy`]）期间，写调用（生效注解不是 `readOnlyHint: true` 的工具）如何处理；只读调用不受影响。
///
/// @why 默认拒绝：Agent 立即得知原因、可转告用户或改做只读操作；排队则在用户停手后立刻执行，可能覆盖用户刚做的修改，
/// 且用户操作时间长时调用方只会等到 `TIMEOUT`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BusyPolicy {
    /// 写调用以 `RATE_LIMITED`（`details.scope = "busy"`）拒绝，未开始、不进去重表。
    #[default]
    Reject,
    /// 写调用留在队列中（仍受 `max_queued_calls` 与调用超时约束），用户操作结束后按到达顺序开始。
    Queue,
}

/// 休眠后的进程驻留策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Residency {
    /// 只断开连接，进程照常运行。
    #[default]
    Keep,
    /// 仅当本进程由唤醒冷启动时，休眠后发出 [`Event::IdleExit`]，由 App 决定是否退出。
    ExitWhenIdle,
    /// 每次休眠后都发出 [`Event::IdleExit`]（无界面的辅助进程）。
    ExitAlways,
}

/// 生命周期策略。
#[derive(Clone, Debug, PartialEq)]
pub struct LifecyclePolicy {
    pub mode: LifecycleMode,
    /// `idle` 模式下空闲多久进入休眠。默认 60s。
    pub idle_timeout_ms: Millis,
    /// 可见性为 `hidden` / `frozen` 时使用的空闲时间（与模式对应的超时取较小值）。默认 15s。
    pub hidden_idle_timeout_ms: Millis,
    /// `on-demand` 模式下任务完成后保留连接的时间。默认 10s。
    pub grace_ms: Millis,
    pub residency: Residency,
    /// 本实例的唤醒描述，随 `app/sleep` 上报；`None` 时 Host 回退到清单 `launch`。
    pub wake: Option<WakeDescriptor>,
    /// `idle` / `on-demand` 下连续多少次以"Host 不在"（[`ConnectionErrorCode::means_host_absent`]）建立连接失败后
    /// 停止重连、进入 `Dormant`（spec/lifecycle.md 第 11 节 A2）。默认 3；0 = 一直重连（旧行为）。`persistent` 不受影响。
    pub host_absent_retries: u32,
    /// 回退到 4e 之前的定时器行为（spec/lifecycle.md 第 11、13 节）：租约到期后才开始计空闲时长、Host 不在时一直重连、
    /// 不论传输一律双向心跳（`app/hello` 不声明 `heartbeatMs`，Host 照旧发 `ping`）；不用合并窗口、任何资源订阅都阻止休眠、
    /// 不做后台立即休眠。默认 `false`。
    pub legacy_timers: bool,
    /// 合并窗口（spec/lifecycle.md 第 13 节 B1）：本连接处理过调用 / 资源读取后，空闲时长取
    /// min(本值, 按模式与可见性的空闲时长)，之后是否在线只由 Host 租约决定。默认 2000 ms；
    /// 不小于 `idle_timeout_ms` 时等同于旧行为。
    pub merge_window_ms: Millis,
    /// 后台立即休眠（spec/lifecycle.md 第 13 节 B4）：`idle` / `on-demand` 下可见性从 `Visible` 变为隐藏 / 冻结后，
    /// 空闲条件一成立就以 [`SleepReason::Background`] 休眠，不等租约与空闲时长；`Backoff` 中直接进入 `Dormant`。
    /// 默认 `false`（移动端封装默认开启）。
    pub sleep_on_background: bool,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            mode: LifecycleMode::Persistent,
            idle_timeout_ms: 60_000,
            hidden_idle_timeout_ms: 15_000,
            grace_ms: 10_000,
            residency: Residency::Keep,
            wake: None,
            host_absent_retries: 3,
            legacy_timers: false,
            merge_window_ms: 2_000,
            sleep_on_background: false,
        }
    }
}

/// 断线重连的指数退避。第 n 次重试的延迟为 `min(initial * multiplier^n, max)`。
#[derive(Clone, Debug, PartialEq)]
pub struct ReconnectPolicy {
    pub initial_delay_ms: Millis,
    pub max_delay_ms: Millis,
    pub multiplier: f64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self { initial_delay_ms: 500, max_delay_ms: 30_000, multiplier: 2.0 }
    }
}

/// 心跳：连接期间每隔 `interval_ms` 发送一次 `ping`，
/// 超过超时时间未收到响应即视为断开。是否发送由 `mode` 与传输类别决定（[`Client::heartbeat_interval`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct HeartbeatPolicy {
    /// 默认 [`HeartbeatMode::Auto`]。
    pub mode: HeartbeatMode,
    pub interval_ms: Millis,
    /// 实例可见时的超时。
    pub timeout_ms: Millis,
    /// 实例隐藏或冻结时的超时（后台页面定时器会被浏览器限流）。
    pub hidden_timeout_ms: Millis,
}

impl Default for HeartbeatPolicy {
    fn default() -> Self {
        Self { mode: HeartbeatMode::Auto, interval_ms: 15_000, timeout_ms: 10_000, hidden_timeout_ms: 120_000 }
    }
}

/// 心跳策略（spec/lifecycle.md 第 11 节 A3）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HeartbeatMode {
    /// 按传输：本地 IPC / 本机回环不发心跳（靠连接断开感知），远程 / 未知发心跳。
    #[default]
    Auto,
    /// 总是发心跳。
    Always,
    /// 从不发心跳（只靠连接断开与调用超时感知）。
    Off,
}
