//! FFI 枚举（风险、状态、生命周期、唤醒等）及其与原生运行时类型的双向转换。

use app_mcp_native as native;

/// 风险等级（旧写法：优先用 [`ToolSpec::annotations`]；两者同时存在时注解中声明的字段优先，缺少的按 risk 推导）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Risk {
    Read,
    Write,
    Destructive,
    Payment,
    OsSensitive,
}

/// 工具对界面的依赖（spec/protocol.md 3.4）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ToolSurface {
    /// 不依赖界面：后台可调、可唤醒（缺省）。
    App,
    /// 依赖界面：只在所在界面可见且处于最上层时注册。
    View,
}

impl From<ToolSurface> for native::ToolSurface {
    fn from(s: ToolSurface) -> Self {
        match s {
            ToolSurface::App => native::ToolSurface::App,
            ToolSurface::View => native::ToolSurface::View,
        }
    }
}

/// 调用结果的业务状态（spec/protocol.md 3.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ResultStatus {
    /// 已完成（缺省）。
    Done,
    /// 已受理、尚未完成（等待用户在 App 内确认或异步处理）；后续状态见 `CallResult.state_resource`。
    Pending,
    /// 只完成了一部分，说明见 `CallResult.summary`。
    Partial,
    /// 没有做任何改动（目标状态已满足或无事可做）。
    Noop,
}

/// 内容面向的对象（MCP 内容注解 `audience`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Audience {
    User,
    Assistant,
}

impl From<ResultStatus> for native::ResultStatus {
    fn from(v: ResultStatus) -> Self {
        match v {
            ResultStatus::Done => native::ResultStatus::Done,
            ResultStatus::Pending => native::ResultStatus::Pending,
            ResultStatus::Partial => native::ResultStatus::Partial,
            ResultStatus::Noop => native::ResultStatus::Noop,
        }
    }
}

impl From<Audience> for native::Audience {
    fn from(v: Audience) -> Self {
        match v {
            Audience::User => native::Audience::User,
            Audience::Assistant => native::Audience::Assistant,
        }
    }
}

/// 调用时 App 需要的激活方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Activation {
    Headless,
    Background,
    Foreground,
}

/// 实例可见性。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Visibility {
    Visible,
    Hidden,
    Frozen,
}

/// 客户端类型。原生 App 用 `Native`（默认）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ClientKind {
    Web,
    Native,
    Hybrid,
}

/// 调用被取消的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CancelReason {
    /// Host 请求取消。
    Requested,
    /// 调用超时。
    Timeout,
    /// 与 Host 断开连接。
    Disconnected,
    /// 客户端已停止。
    Stopped,
}

/// 日志级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// 连接状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StateStatus {
    Idle,
    Connecting,
    Handshaking,
    PendingPairing,
    Connected,
    Backoff,
    Rejected,
    Stopped,
    Dormant,
    Waking,
    /// 对端不是期望的 Host（不是 app-mcp，或属于其他用户；spec/protocol.md 1.6）。不再自动重连，
    /// `wake()` / `connect_now()` 时再试一次；原因见 `StateInfo.reason`。
    HostMismatch,
}

/// 生命周期模式（spec/lifecycle.md 第 3 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LifecycleMode {
    /// 不休眠（兼容现有行为）。
    Persistent,
    /// 启动即连接；空闲后休眠；唤醒后回连。
    Idle,
    /// 启动时不连接；被唤醒或 `connect_now` 时连接，任务完成后经过 `grace_ms` 休眠。
    OnDemand,
}

/// 心跳策略（spec/lifecycle.md 第 11 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HeartbeatMode {
    /// 按传输：本地 IPC / 桌面本机回环不发心跳，远程（含 Android / iOS 上经 adb reverse 的回环）发。
    Auto,
    /// 总是发心跳。
    Always,
    /// 从不发心跳。
    Off,
}

impl From<HeartbeatMode> for native::HeartbeatMode {
    fn from(v: HeartbeatMode) -> Self {
        match v {
            HeartbeatMode::Auto => native::HeartbeatMode::Auto,
            HeartbeatMode::Always => native::HeartbeatMode::Always,
            HeartbeatMode::Off => native::HeartbeatMode::Off,
        }
    }
}

/// 休眠后的进程驻留策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Residency {
    /// 只断开连接，进程照常运行。
    Keep,
    /// 仅当本进程由唤醒冷启动时，休眠后回调 `on_idle_exit`。
    ExitWhenIdle,
    /// 每次休眠后都回调 `on_idle_exit`（无界面的辅助进程）。
    ExitAlways,
}

/// 唤醒方式（spec/lifecycle.md 第 5 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WakeKind {
    /// 自定义 URL scheme：`<scheme>://app-mcp/wake?token=`。
    Uri,
    /// Windows 打包应用 AUMID。
    Aumid,
    /// macOS Apple Event。
    AppleEvent,
    /// Linux D-Bus `org.freedesktop.Application.ActivateAction`。
    Dbus,
    /// Android 显式广播。
    AndroidIntent,
    /// 网页 URL。
    WebUrl,
    /// 不可唤醒（Host 回退到清单 `launch`）。
    None,
}

/// 回连原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WakeReason {
    /// 由 Host 通过操作系统激活机制唤醒。
    OsActivation,
    /// App 主动回连。
    App,
    /// 窗口 / 页面重新可见。
    Visible,
    /// 进程启动后的首次连接。
    ColdStart,
}

/// 休眠原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SleepReason {
    Idle,
    Grace,
    /// 进入后台。
    Background,
    /// App 主动请求。
    App,
}

impl From<LifecycleMode> for native::LifecycleMode {
    fn from(v: LifecycleMode) -> Self {
        match v {
            LifecycleMode::Persistent => native::LifecycleMode::Persistent,
            LifecycleMode::Idle => native::LifecycleMode::Idle,
            LifecycleMode::OnDemand => native::LifecycleMode::OnDemand,
        }
    }
}

impl From<Residency> for native::Residency {
    fn from(v: Residency) -> Self {
        match v {
            Residency::Keep => native::Residency::Keep,
            Residency::ExitWhenIdle => native::Residency::ExitWhenIdle,
            Residency::ExitAlways => native::Residency::ExitAlways,
        }
    }
}

impl From<WakeKind> for native::WakeKind {
    fn from(v: WakeKind) -> Self {
        match v {
            WakeKind::Uri => native::WakeKind::Uri,
            WakeKind::Aumid => native::WakeKind::Aumid,
            WakeKind::AppleEvent => native::WakeKind::AppleEvent,
            WakeKind::Dbus => native::WakeKind::Dbus,
            WakeKind::AndroidIntent => native::WakeKind::AndroidIntent,
            WakeKind::WebUrl => native::WakeKind::WebUrl,
            WakeKind::None => native::WakeKind::None,
        }
    }
}

impl From<WakeReason> for native::WakeReason {
    fn from(v: WakeReason) -> Self {
        match v {
            WakeReason::OsActivation => native::WakeReason::OsActivation,
            WakeReason::App => native::WakeReason::App,
            WakeReason::Visible => native::WakeReason::Visible,
            WakeReason::ColdStart => native::WakeReason::ColdStart,
        }
    }
}

impl From<SleepReason> for native::SleepReason {
    fn from(v: SleepReason) -> Self {
        match v {
            SleepReason::Idle => native::SleepReason::Idle,
            SleepReason::Grace => native::SleepReason::Grace,
            SleepReason::Background => native::SleepReason::Background,
            SleepReason::App => native::SleepReason::App,
        }
    }
}

impl From<Risk> for native::Risk {
    fn from(v: Risk) -> Self {
        match v {
            Risk::Read => native::Risk::Read,
            Risk::Write => native::Risk::Write,
            Risk::Destructive => native::Risk::Destructive,
            Risk::Payment => native::Risk::Payment,
            Risk::OsSensitive => native::Risk::OsSensitive,
        }
    }
}

impl From<Activation> for native::Activation {
    fn from(v: Activation) -> Self {
        match v {
            Activation::Headless => native::Activation::Headless,
            Activation::Background => native::Activation::Background,
            Activation::Foreground => native::Activation::Foreground,
        }
    }
}

impl From<Visibility> for native::Visibility {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Visible => native::Visibility::Visible,
            Visibility::Hidden => native::Visibility::Hidden,
            Visibility::Frozen => native::Visibility::Frozen,
        }
    }
}

impl From<ClientKind> for native::ClientKind {
    fn from(v: ClientKind) -> Self {
        match v {
            ClientKind::Web => native::ClientKind::Web,
            ClientKind::Native => native::ClientKind::Native,
            ClientKind::Hybrid => native::ClientKind::Hybrid,
        }
    }
}

impl From<native::CancelReason> for CancelReason {
    fn from(v: native::CancelReason) -> Self {
        match v {
            native::CancelReason::Requested => CancelReason::Requested,
            native::CancelReason::Timeout => CancelReason::Timeout,
            native::CancelReason::Disconnected => CancelReason::Disconnected,
            native::CancelReason::Stopped => CancelReason::Stopped,
        }
    }
}

impl From<native::LogLevel> for LogLevel {
    fn from(v: native::LogLevel) -> Self {
        match v {
            native::LogLevel::Debug => LogLevel::Debug,
            native::LogLevel::Info => LogLevel::Info,
            native::LogLevel::Warn => LogLevel::Warn,
            native::LogLevel::Error => LogLevel::Error,
        }
    }
}

impl From<native::StateStatus> for StateStatus {
    fn from(v: native::StateStatus) -> Self {
        match v {
            native::StateStatus::Idle => StateStatus::Idle,
            native::StateStatus::Connecting => StateStatus::Connecting,
            native::StateStatus::Handshaking => StateStatus::Handshaking,
            native::StateStatus::PendingPairing => StateStatus::PendingPairing,
            native::StateStatus::Connected => StateStatus::Connected,
            native::StateStatus::Backoff => StateStatus::Backoff,
            native::StateStatus::Rejected => StateStatus::Rejected,
            native::StateStatus::Stopped => StateStatus::Stopped,
            native::StateStatus::Dormant => StateStatus::Dormant,
            native::StateStatus::Waking => StateStatus::Waking,
            native::StateStatus::HostMismatch => StateStatus::HostMismatch,
        }
    }
}
