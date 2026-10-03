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
    /// Host 侧限流：对该（App, 工具）或该 App 的调用频率超出上限，调用未转发；`data.retryAfterMs` 给出建议等待时长。
    RateLimited,
    /// Host 侧大小上限：调用参数、调用结果或资源内容超过上限，未转发 / 未返回（不截断）。
    PayloadTooLarge,
    /// 调用被用户 / 厂商的策略规则拒绝（`deny`，spec/hub-api.md 3.13），未转发、未唤醒；`data.ruleId` 为命中规则的标识。
    PolicyDenied,
    /// 需要用户本人操作后才能继续（登录过期、系统权限未授予、需切到前台、需在 App 内确认等），由 App 的 handler 返回；
    /// `message` 面向用户，`data.reason` / `data.uri` 可选（见 [`ToolError::user_action_required`]）。
    UserActionRequired,
    /// 导航失败（第 4c 项，spec/protocol.md 3.4）：App 不支持导航、导航出错或超时，或导航后目标工具没有出现；
    /// `data.reason` 见 [`navigation_reason`]，`data.page` 为目标页面。
    NavigationFailed,
    /// 导航被拒绝：App 拒绝本次导航（如用户正在输入、页面需要登录），或清单声明该页面不可由 Agent 导航
    /// （`navigable: false`）；`data.reason` 见 [`navigation_reason`]。重试不会改变结果，应请用户自行打开。
    NavigationDenied,
    /// 对象锁冲突（第 16 项 N6，spec/hub-api.md 3.6「对象锁」，只由 Hub 产生）：App 正被其他 Agent 锁定，写调用未转发；
    /// 或要加的锁已被他人持有。`data.holder` 为持有者的记账主体，`data.retryAfterMs` 为锁的剩余有效期。
    Locked,
}

impl ErrorKind {
    /// 全部类别（唯一列表；按字符串解析时使用 [`ErrorKind::parse`]）。
    pub const ALL: [ErrorKind; 22] = [
        ErrorKind::ToolNotFound,
        ErrorKind::ToolDisabled,
        ErrorKind::InvalidInput,
        ErrorKind::UserRejected,
        ErrorKind::Timeout,
        ErrorKind::HandlerError,
        ErrorKind::Cancelled,
        ErrorKind::AppDisconnected,
        ErrorKind::AppNotInstalled,
        ErrorKind::LaunchFailed,
        ErrorKind::AppNotResponding,
        ErrorKind::InstanceFrozen,
        ErrorKind::ResourceNotFound,
        ErrorKind::Unauthorized,
        ErrorKind::UnsupportedProtocol,
        ErrorKind::RateLimited,
        ErrorKind::PayloadTooLarge,
        ErrorKind::PolicyDenied,
        ErrorKind::UserActionRequired,
        ErrorKind::NavigationFailed,
        ErrorKind::NavigationDenied,
        ErrorKind::Locked,
    ];

    /// 字符串形式（如 `"HANDLER_ERROR"`）→ 类别；未知值返回 `None`。
    pub fn parse(s: &str) -> Option<ErrorKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// 数字错误码 → 类别；不是本协议的错误码时返回 `None`。
    pub fn from_code(code: i64) -> Option<ErrorKind> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }

    /// JSON-RPC 错误码。
    ///
    /// @invariant 分区（spec/protocol.md 第 4 节）：-32001 ~ -32019 为既有类别（实现自定义区，保留不变）；
    /// -32020 ~ -32099 归 MCP 规范（`HeaderMismatch` -32020 等），本协议不再使用；新增类别从 -31001 起
    /// （JSON-RPC 保留区 -32768 ~ -32000 之外的应用定义区），避免与上游 MCP 服务器的错误码混淆。
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
            ErrorKind::RateLimited => -32016,
            ErrorKind::PayloadTooLarge => -32017,
            ErrorKind::PolicyDenied => -32018,
            ErrorKind::UserActionRequired => -32019,
            ErrorKind::NavigationFailed => -31001,
            ErrorKind::NavigationDenied => -31002,
            ErrorKind::Locked => -31003,
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
            ErrorKind::RateLimited => "RATE_LIMITED",
            ErrorKind::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            ErrorKind::PolicyDenied => "POLICY_DENIED",
            ErrorKind::UserActionRequired => "USER_ACTION_REQUIRED",
            ErrorKind::NavigationFailed => "NAVIGATION_FAILED",
            ErrorKind::NavigationDenied => "NAVIGATION_DENIED",
            ErrorKind::Locked => "LOCKED",
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

    /// `USER_ACTION_REQUIRED`（spec/protocol.md 第 4 节）：需要用户本人操作后才能继续。
    ///
    /// @input message 面向用户的说明（Agent 应转告用户），如"登录已过期，请在 App 内重新登录后重试"。
    /// @input reason 可选类别：[`user_action_reason`] 中的值或其他字符串。
    /// @input uri 可选的 App 内入口（深链接等），供 Agent / 用户打开。
    pub fn user_action_required(message: impl Into<String>, reason: Option<&str>, uri: Option<&str>) -> Self {
        let mut details = serde_json::Map::new();
        if let Some(r) = reason {
            details.insert("reason".into(), Value::String(r.to_owned()));
        }
        if let Some(u) = uri {
            details.insert("uri".into(), Value::String(u.to_owned()));
        }
        let err = Self::new(ErrorKind::UserActionRequired, message);
        if details.is_empty() { err } else { err.with_details(Value::Object(details)) }
    }
}

impl ToolError {
    /// `NAVIGATION_DENIED`（spec/protocol.md 3.4）：App 拒绝本次导航（`data.reason` = `app`）。各语言 SDK 的导航回调据此拒绝。
    ///
    /// @input message 面向模型 / 用户的说明，如"正在编辑草稿，请先保存后再切换页面"。
    pub fn navigation_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NavigationDenied, message).with_details(json!({ "reason": navigation_reason::APP }))
    }

    /// `NAVIGATION_FAILED`（spec/protocol.md 3.4）：导航没有完成。
    ///
    /// @input reason [`navigation_reason`] 中的值。
    pub fn navigation_failed(message: impl Into<String>, reason: &str) -> Self {
        Self::new(ErrorKind::NavigationFailed, message).with_details(json!({ "reason": reason }))
    }
}

/// `NAVIGATION_FAILED` / `NAVIGATION_DENIED` 的 `data.reason` 取值（spec/protocol.md 3.4）。
pub mod navigation_reason {
    /// App（实例）不支持导航：握手没有声明 `capabilities.navigate`，或没有设置导航回调。
    pub const UNSUPPORTED: &str = "unsupported";
    /// App 的导航回调出错（页面不存在、参数不合法等）。
    pub const ERROR: &str = "error";
    /// App 在时限内没有回复导航请求。
    pub const TIMEOUT: &str = "timeout";
    /// 导航完成，但时限内目标工具没有注册（页面没有提供该工具或界面未就绪）。
    pub const TOOL_NOT_REGISTERED: &str = "tool-not-registered";
    /// App 拒绝本次导航（`NAVIGATION_DENIED`）。
    pub const APP: &str = "app";
    /// 清单声明该页面不可由 Agent 导航（`navigable: false`，`NAVIGATION_DENIED`）。
    pub const NOT_NAVIGABLE: &str = "not-navigable";
}

/// `USER_ACTION_REQUIRED` 的 `data.reason` 建议取值（spec/protocol.md 第 4 节；接收方遇到其他值按原样展示）。
pub mod user_action_reason {
    /// 登录已过期 / 未登录。
    pub const LOGIN: &str = "login";
    /// 系统权限未授予（相机、位置、通知等）。
    pub const PERMISSION: &str = "permission";
    /// 需要把 App 切到前台。
    pub const FOREGROUND: &str = "foreground";
    /// 需要用户在 App 内确认。
    pub const CONFIRM: &str = "confirm";
    /// 操作系统阻止了 Hub 启动 / 绑定目标 App（关联启动、自启动管控），需用户在系统设置中放行；由 Hub 产生
    /// （spec/protocol.md 第 4 节，`data` 另带 `appId`、`packageName`、`appName`、`code`）。
    pub const OS_PERMISSION: &str = "os-permission";
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
        assert_eq!(serde_json::to_value(ErrorKind::RateLimited).unwrap(), json!("RATE_LIMITED"));
        assert_eq!((ErrorKind::RateLimited.code(), ErrorKind::PayloadTooLarge.code()), (-32016, -32017));
        assert_eq!(ErrorKind::PayloadTooLarge.as_str(), "PAYLOAD_TOO_LARGE");
        assert_eq!((ErrorKind::PolicyDenied.code(), ErrorKind::UserActionRequired.code()), (-32018, -32019));
        assert_eq!(serde_json::to_value(ErrorKind::PolicyDenied).unwrap(), json!("POLICY_DENIED"));
        assert_eq!(ErrorKind::UserActionRequired.as_str(), "USER_ACTION_REQUIRED");
        assert_eq!((ErrorKind::NavigationFailed.code(), ErrorKind::NavigationDenied.code()), (-31001, -31002));
        assert_eq!(serde_json::to_value(ErrorKind::NavigationDenied).unwrap(), json!("NAVIGATION_DENIED"));
        assert_eq!((ErrorKind::Locked.code(), ErrorKind::Locked.as_str()), (-31003, "LOCKED"));
        assert_eq!(ErrorKind::parse("LOCKED"), Some(ErrorKind::Locked));
    }

    #[test]
    fn all_kinds_unique_and_parse() {
        let mut codes: Vec<i64> = ErrorKind::ALL.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ErrorKind::ALL.len(), "错误码不重复");
        // MCP 规范占用的 -32020 ~ -32099 不使用
        assert!(ErrorKind::ALL.iter().all(|k| !(-32099..=-32020).contains(&k.code())));
        for k in ErrorKind::ALL {
            assert_eq!(ErrorKind::parse(k.as_str()), Some(k));
            assert_eq!(ErrorKind::from_code(k.code()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
        }
        assert_eq!(ErrorKind::parse("NOPE"), None);
        assert_eq!(ErrorKind::from_code(-32020), None);
    }

    #[test]
    fn navigation_errors() {
        let rpc: RpcError = ToolError::navigation_denied("正在编辑").into();
        assert_eq!(rpc.code, -31002);
        assert_eq!(rpc.data, Some(json!({"kind": "NAVIGATION_DENIED", "reason": "app"})));
        let rpc: RpcError = ToolError::navigation_failed("没有该页面", navigation_reason::ERROR).into();
        assert_eq!(rpc.data, Some(json!({"kind": "NAVIGATION_FAILED", "reason": "error"})));
        assert_eq!(rpc.to_tool_error().kind, ErrorKind::NavigationFailed);
    }

    #[test]
    fn user_action_required_details() {
        let e = ToolError::user_action_required("请先登录", Some(user_action_reason::LOGIN), Some("shop://login"));
        let rpc: RpcError = e.clone().into();
        assert_eq!(rpc.code, -32019);
        assert_eq!(rpc.data, Some(json!({"kind": "USER_ACTION_REQUIRED", "reason": "login", "uri": "shop://login"})));
        assert_eq!(rpc.to_tool_error(), e);
        let bare = ToolError::user_action_required("切到前台", None, None);
        assert_eq!(bare.details, None);
        let rpc: RpcError = bare.into();
        assert_eq!(rpc.data, Some(json!({"kind": "USER_ACTION_REQUIRED"})));
    }
}
