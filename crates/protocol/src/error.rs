//! 协议错误类别与错误码。
//!
//! 工具调用、资源读取等失败统一用 JSON-RPC 错误返回：
//! `code` 取自 [`ErrorKind::code`]，`data.kind` 为 [`ErrorKind`] 的字符串形式。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::jsonrpc::RpcError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorKind {
    /// 工具不存在（或当前实例没有注册）。
    ToolNotFound,
    /// 工具存在但当前被禁用。
    ToolDisabled,
    /// 参数不符合 inputSchema。
    InvalidInput,
    /// 用户在确认框中拒绝。
    UserRejected,
    /// 调用超时。
    Timeout,
    /// handler 抛出异常或返回错误。
    HandlerError,
    /// 调用被取消。
    Cancelled,
    /// App 未连接。
    AppDisconnected,
    /// App 未安装。
    AppNotInstalled,
    /// 唤醒失败。
    LaunchFailed,
    /// UI 线程无响应。
    AppNotResponding,
    /// 后台页面被冻结且无法激活。
    InstanceFrozen,
    /// 资源不存在。
    ResourceNotFound,
    /// 未配对或配对被拒绝。
    Unauthorized,
    /// 协议版本不兼容。
    UnsupportedProtocol,
}

impl ErrorKind {
    /// JSON-RPC 错误码（实现自定义区间 -32000 ~ -32099）。
    pub fn code(self) -> i64 {
        match self {
            ErrorKind::ToolNotFound => -32001,
            ErrorKind::ToolDisabled => -32002,
            ErrorKind::InvalidInput => -32003,
            ErrorKind::UserRejected => -32004,
            ErrorKind::Timeout => -32005,
            ErrorKind::HandlerError => -32006,
            ErrorKind::Cancelled => -32007,
            ErrorKind::AppDisconnected => -32008,
            ErrorKind::AppNotInstalled => -32009,
            ErrorKind::LaunchFailed => -32010,
            ErrorKind::AppNotResponding => -32011,
            ErrorKind::InstanceFrozen => -32012,
            ErrorKind::ResourceNotFound => -32013,
            ErrorKind::Unauthorized => -32014,
            ErrorKind::UnsupportedProtocol => -32015,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::ToolNotFound => "TOOL_NOT_FOUND",
            ErrorKind::ToolDisabled => "TOOL_DISABLED",
            ErrorKind::InvalidInput => "INVALID_INPUT",
            ErrorKind::UserRejected => "USER_REJECTED",
            ErrorKind::Timeout => "TIMEOUT",
            ErrorKind::HandlerError => "HANDLER_ERROR",
            ErrorKind::Cancelled => "CANCELLED",
            ErrorKind::AppDisconnected => "APP_DISCONNECTED",
            ErrorKind::AppNotInstalled => "APP_NOT_INSTALLED",
            ErrorKind::LaunchFailed => "LAUNCH_FAILED",
            ErrorKind::AppNotResponding => "APP_NOT_RESPONDING",
            ErrorKind::InstanceFrozen => "INSTANCE_FROZEN",
            ErrorKind::ResourceNotFound => "RESOURCE_NOT_FOUND",
            ErrorKind::Unauthorized => "UNAUTHORIZED",
            ErrorKind::UnsupportedProtocol => "UNSUPPORTED_PROTOCOL",
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 带类别的业务错误。与 [`RpcError`] 互相转换。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct ToolError {
    pub kind: ErrorKind,
    /// 面向模型的说明：原因与建议的下一步。
    pub message: String,
    /// 额外信息，会合并进 `data`（`kind` 字段总是由本类型写入）。
    pub details: Option<Value>,
}

impl ToolError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), details: None }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl From<ToolError> for RpcError {
    fn from(e: ToolError) -> Self {
        let mut data = json!({ "kind": e.kind });
        if let Some(Value::Object(extra)) = e.details {
            let obj = data.as_object_mut().expect("object");
            for (k, v) in extra {
                if k != "kind" {
                    obj.insert(k, v);
                }
            }
        } else if let Some(other) = e.details {
            data["details"] = other;
        }
        RpcError { code: e.kind.code(), message: e.message, data: Some(data) }
    }
}

impl RpcError {
    /// 读取 `data.kind`；不是本协议定义的错误时返回 `None`。
    pub fn kind(&self) -> Option<ErrorKind> {
        let kind = self.data.as_ref()?.get("kind")?;
        serde_json::from_value(kind.clone()).ok()
    }

    /// 转换为 [`ToolError`]；无法识别类别时归为 `HANDLER_ERROR`。
    pub fn to_tool_error(&self) -> ToolError {
        let details = self.data.as_ref().and_then(|d| {
            let mut d = d.clone();
            d.as_object_mut()?.remove("kind");
            if d.as_object()?.is_empty() { None } else { Some(d) }
        });
        ToolError {
            kind: self.kind().unwrap_or(ErrorKind::HandlerError),
            message: self.message.clone(),
            details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_error_roundtrip() {
        let e = ToolError::new(ErrorKind::ToolDisabled, "cart is empty")
            .with_details(json!({"hint": "add items first"}));
        let rpc: RpcError = e.clone().into();
        assert_eq!(rpc.code, -32002);
        assert_eq!(rpc.kind(), Some(ErrorKind::ToolDisabled));
        assert_eq!(rpc.to_tool_error(), e);
    }

    #[test]
    fn kind_serializes_screaming() {
        assert_eq!(serde_json::to_value(ErrorKind::AppNotResponding).unwrap(), json!("APP_NOT_RESPONDING"));
        assert_eq!(ErrorKind::AppNotResponding.as_str(), "APP_NOT_RESPONDING");
    }
}
