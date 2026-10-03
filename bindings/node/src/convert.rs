//! 错误与枚举转换：原生错误 → JS 错误（`code` 为大写代码），JS 字符串 → 原生枚举，原生枚举 → JS 字符串。

use app_mcp_native::{
    Activation, Audience, BusyPolicy, CancelReason, ClientKind, ErrorKind, HeartbeatMode, LifecycleMode, LogLevel, NativeError,
    Residency, ResultStatus, Risk, SleepReason, StateStatus, ToolSurface, Visibility, WakeKind, WakeReason,
};

/// 本绑定抛出的错误：`status` 字符串成为 JS 错误的 `code`（napi-derive 按名称 `Result` 识别返回类型）。
use napi::Result;

pub(super) fn native_error_code(e: &NativeError) -> &'static str {
    match e {
        NativeError::InvalidName(_) => "INVALID_NAME",
        NativeError::InvalidSchema(_) => "INVALID_SCHEMA",
        NativeError::DuplicateName(_) => "DUPLICATE_NAME",
        NativeError::InvalidJson(_) => "INVALID_JSON",
        NativeError::InvalidConfig(_) => "INVALID_CONFIG",
        NativeError::AlreadyCompleted => "ALREADY_COMPLETED",
        NativeError::Disposed => "DISPOSED",
        NativeError::Stopped => "STOPPED",
        NativeError::Internal(_) => "INTERNAL",
    }
}

pub(super) fn to_js_error(e: NativeError) -> napi::Error<String> {
    napi::Error::new(native_error_code(&e).to_string(), e.to_string())
}

pub(super) fn invalid_arg(message: String) -> napi::Error<String> {
    napi::Error::new("INVALID_ARG".to_string(), message)
}

pub(super) fn parse_risk(s: &str) -> Result<Risk, String> {
    Ok(match s {
        "read" => Risk::Read,
        "write" => Risk::Write,
        "destructive" => Risk::Destructive,
        "payment" => Risk::Payment,
        "os-sensitive" => Risk::OsSensitive,
        other => return Err(invalid_arg(format!("未知的 risk：{other:?}"))),
    })
}

pub(super) fn parse_activation(s: &str) -> Result<Activation, String> {
    Ok(match s {
        "headless" => Activation::Headless,
        "background" => Activation::Background,
        "foreground" => Activation::Foreground,
        other => return Err(invalid_arg(format!("未知的 activation：{other:?}"))),
    })
}

pub(super) fn parse_visibility(s: &str) -> Result<Visibility, String> {
    Ok(match s {
        "visible" => Visibility::Visible,
        "hidden" => Visibility::Hidden,
        "frozen" => Visibility::Frozen,
        other => return Err(invalid_arg(format!("未知的 visibility：{other:?}"))),
    })
}

pub(super) fn parse_client_kind(s: &str) -> Result<ClientKind, String> {
    Ok(match s {
        "native" => ClientKind::Native,
        "hybrid" => ClientKind::Hybrid,
        "web" => ClientKind::Web,
        other => return Err(invalid_arg(format!("未知的 clientKind：{other:?}"))),
    })
}

pub(super) fn parse_error_kind(s: &str) -> Result<ErrorKind, String> {
    ErrorKind::parse(s).ok_or_else(|| invalid_arg(format!("未知的错误类别：{s:?}")))
}

pub(super) fn parse_surface(s: &str) -> Result<ToolSurface, String> {
    Ok(match s {
        "app" => ToolSurface::App,
        "view" => ToolSurface::View,
        other => return Err(invalid_arg(format!("未知的 surface：{other:?}"))),
    })
}

pub(super) fn parse_result_status(s: &str) -> Result<ResultStatus, String> {
    Ok(match s {
        "done" => ResultStatus::Done,
        "pending" => ResultStatus::Pending,
        "partial" => ResultStatus::Partial,
        "noop" => ResultStatus::Noop,
        other => return Err(invalid_arg(format!("未知的 status：{other:?}"))),
    })
}

pub(super) fn parse_audience(s: &str) -> Result<Audience, String> {
    Ok(match s {
        "user" => Audience::User,
        "assistant" => Audience::Assistant,
        other => return Err(invalid_arg(format!("未知的 audience：{other:?}"))),
    })
}

pub(super) fn parse_lifecycle_mode(s: &str) -> Result<LifecycleMode, String> {
    Ok(match s {
        "persistent" => LifecycleMode::Persistent,
        "idle" => LifecycleMode::Idle,
        "on-demand" => LifecycleMode::OnDemand,
        other => return Err(invalid_arg(format!("未知的 lifecycle.mode：{other:?}"))),
    })
}

pub(super) fn parse_heartbeat(s: &str) -> Result<HeartbeatMode, String> {
    Ok(match s {
        "auto" => HeartbeatMode::Auto,
        "always" => HeartbeatMode::Always,
        "off" => HeartbeatMode::Off,
        other => return Err(invalid_arg(format!("未知的 heartbeat：{other:?}"))),
    })
}

pub(super) fn parse_busy_policy(s: &str) -> Result<BusyPolicy, String> {
    Ok(match s {
        "reject" => BusyPolicy::Reject,
        "queue" => BusyPolicy::Queue,
        other => return Err(invalid_arg(format!("未知的 busyPolicy：{other:?}"))),
    })
}

pub(super) fn parse_residency(s: &str) -> Result<Residency, String> {
    Ok(match s {
        "keep" => Residency::Keep,
        "exit-when-idle" => Residency::ExitWhenIdle,
        "exit-always" => Residency::ExitAlways,
        other => return Err(invalid_arg(format!("未知的 lifecycle.residency：{other:?}"))),
    })
}

pub(super) fn parse_wake_kind(s: &str) -> Result<WakeKind, String> {
    Ok(match s {
        "uri" => WakeKind::Uri,
        "aumid" => WakeKind::Aumid,
        "apple-event" => WakeKind::AppleEvent,
        "dbus" => WakeKind::Dbus,
        "android-intent" => WakeKind::AndroidIntent,
        "web-url" => WakeKind::WebUrl,
        "none" => WakeKind::None,
        other => return Err(invalid_arg(format!("未知的 wake.kind：{other:?}"))),
    })
}

pub(super) fn parse_wake_reason(s: &str) -> Result<WakeReason, String> {
    Ok(match s {
        "os-activation" => WakeReason::OsActivation,
        "app" => WakeReason::App,
        "visible" => WakeReason::Visible,
        "cold-start" => WakeReason::ColdStart,
        other => return Err(invalid_arg(format!("未知的唤醒原因：{other:?}"))),
    })
}

pub(super) fn parse_sleep_reason(s: &str) -> Result<SleepReason, String> {
    Ok(match s {
        "idle" => SleepReason::Idle,
        "grace" => SleepReason::Grace,
        "background" => SleepReason::Background,
        "app" => SleepReason::App,
        other => return Err(invalid_arg(format!("未知的休眠原因：{other:?}"))),
    })
}

/// JS 数字 → 毫秒数（非负、有限；小数向下取整）。
pub(super) fn parse_millis(field: &str, v: f64) -> Result<u64, String> {
    if !v.is_finite() || v < 0.0 {
        return Err(invalid_arg(format!("{field} 必须是非负有限数：{v}")));
    }
    // 已检查非负有限；超出 u64 范围时饱和。
    Ok(v.floor().min(u64::MAX as f64) as u64)
}

pub(super) fn status_str(s: StateStatus) -> &'static str {
    match s {
        StateStatus::Idle => "idle",
        StateStatus::Connecting => "connecting",
        StateStatus::Handshaking => "handshaking",
        StateStatus::PendingPairing => "pending-pairing",
        StateStatus::Connected => "connected",
        StateStatus::Backoff => "backoff",
        StateStatus::Rejected => "rejected",
        StateStatus::Stopped => "stopped",
        StateStatus::Dormant => "dormant",
        StateStatus::Waking => "waking",
        StateStatus::HostMismatch => "host-mismatch",
    }
}

pub(super) fn cancel_reason_str(r: CancelReason) -> &'static str {
    match r {
        CancelReason::Requested => "requested",
        CancelReason::Timeout => "timeout",
        CancelReason::Disconnected => "disconnected",
        CancelReason::Stopped => "stopped",
    }
}

pub(super) fn log_level_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}
