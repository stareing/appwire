//! 绑定层错误 [`HubError`] 及错误转换、JSON 文本解析。

use app_mcp_hub as hub;
use serde_json::Value;

/// Hub 操作的错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error, uniffi::Error)]
pub enum HubError {
    /// 协议错误（spec/protocol.md §4）：`kind` 如 `"TOOL_NOT_FOUND"`、`"RESOURCE_NOT_FOUND"`。
    #[error("{kind}: {reason}")]
    Tool {
        kind: String,
        reason: String,
        details_json: Option<String>,
    },
    #[error("JSON 不合法：{detail}")]
    InvalidJson { detail: String },
    #[error("配置不合法：{detail}")]
    InvalidConfig { detail: String },
    #[error("I/O 错误：{detail}")]
    Io { detail: String },
    #[error("Hub 已停止")]
    Shutdown,
    /// 本构建未包含所需能力（cargo feature，spec/hub-api.md 3.10），如 Android 精简库上 `mcp_http = true`、
    /// `upstreams` 非空或 `serve_http`。`detail` 说明缺哪个 feature。换完整构建或关闭该配置；重试无效。
    #[error("不支持：{detail}")]
    Unsupported { detail: String },
}

impl From<hub::HubError> for HubError {
    fn from(e: hub::HubError) -> Self {
        let e = e.0;
        HubError::Tool {
            kind: e.kind.as_str().to_owned(),
            reason: e.message,
            details_json: e.details.map(|d| d.to_string()),
        }
    }
}

impl From<std::io::Error> for HubError {
    fn from(e: std::io::Error) -> Self {
        let detail = e.to_string();
        match e.kind() {
            std::io::ErrorKind::Unsupported => HubError::Unsupported { detail },
            _ => HubError::Io { detail },
        }
    }
}

/// `WakeResponder::fail` 的错误：`kind` 为协议错误类别（如 `"LAUNCH_FAILED"`、`"APP_NOT_INSTALLED"`），
/// 不认识的类别按 `LAUNCH_FAILED` 处理。
pub(crate) fn wake_error(kind: &str, reason: String) -> hub::HubError {
    let kind = serde_json::from_value::<hub::ErrorKind>(Value::String(kind.to_owned()))
        .unwrap_or(hub::ErrorKind::LaunchFailed);
    let message = if reason.is_empty() { "唤醒失败。".to_owned() } else { reason };
    hub::HubError::new(kind, message)
}

pub(crate) fn parse_json(text: &str) -> Result<Value, HubError> {
    serde_json::from_str(text).map_err(|e| HubError::InvalidJson {
        detail: e.to_string(),
    })
}
