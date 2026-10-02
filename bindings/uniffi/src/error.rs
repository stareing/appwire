//! 绑定层统一错误 [`AppMcpError`] 与错误类别字符串（[`error_kinds`]）。

use app_mcp_native as native;
use native::ErrorKind;

/// 绑定层的统一错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error, uniffi::Error)]
pub enum AppMcpError {
    #[error("名称不合法：{detail}")]
    InvalidName { detail: String },
    #[error("inputSchema 不合法：{detail}")]
    InvalidSchema { detail: String },
    #[error("名为 {name:?} 的工具或资源已注册")]
    DuplicateName { name: String },
    #[error("JSON 不合法：{detail}")]
    InvalidJson { detail: String },
    #[error("配置不合法：{detail}")]
    InvalidConfig { detail: String },
    #[error("未知的错误类别：{kind:?}")]
    UnknownErrorKind { kind: String },
    #[error("调用或读取已完成或已取消")]
    AlreadyCompleted,
    #[error("句柄已释放")]
    Disposed,
    #[error("客户端已停止")]
    Stopped,
    #[error("内部错误：{detail}")]
    Internal { detail: String },
}

impl From<native::NativeError> for AppMcpError {
    fn from(e: native::NativeError) -> Self {
        use native::NativeError as N;
        match e {
            N::InvalidName(detail) => AppMcpError::InvalidName { detail },
            N::InvalidSchema(detail) => AppMcpError::InvalidSchema { detail },
            N::DuplicateName(name) => AppMcpError::DuplicateName { name },
            N::InvalidJson(detail) => AppMcpError::InvalidJson { detail },
            N::InvalidConfig(detail) => AppMcpError::InvalidConfig { detail },
            N::AlreadyCompleted => AppMcpError::AlreadyCompleted,
            N::Disposed => AppMcpError::Disposed,
            N::Stopped => AppMcpError::Stopped,
            N::Internal(detail) => AppMcpError::Internal { detail },
        }
    }
}

pub(crate) fn parse_error_kind(kind: &str) -> Result<ErrorKind, AppMcpError> {
    ErrorKind::parse(kind)
        .ok_or_else(|| AppMcpError::UnknownErrorKind {
            kind: kind.to_owned(),
        })
}

/// 返回所有合法的错误类别字符串（如 `"HANDLER_ERROR"`）。
#[uniffi::export]
pub fn error_kinds() -> Vec<String> {
    ErrorKind::ALL
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect()
}
