//! 连接级错误码与诊断上报（spec/protocol.md 第 10 节）。
//!
//! - [`ConnectionErrorCode`]：SDK 连接状态（`backoff` / `rejected` / `host-mismatch` / `blocked`）与 Host 启动失败
//!   共用的机器可读错误码。每个码有固定的类别、中文原因与修复建议（[`ConnectionErrorCode::reason`] /
//!   [`ConnectionErrorCode::hint`]），spec/protocol.md 的错误码表与此一一对应。
//! - [`ConnectionIssue`]：错误码 + 本次的具体说明。
//! - `app/diagnostic`（[`DiagnosticParams`]）：SDK 在连接恢复后上报此前遇到的连接问题（如浏览器拦截），
//!   Host 记录在状态中（`/status`、`app-mcp-host doctor`）。
//!
//! 线上传输的错误码一律是字符串（`HelloResult.code`、`PairingResultParams.code`、`DiagnosticParams.code`），
//! 新版本可能增加错误码；接收方遇到不认识的码时按 [`ConnectionErrorCode::parse`] 返回 `None` 处理，不报错。

use serde::{Deserialize, Serialize};

/// 错误码的类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueKind {
    /// 建立连接失败（Host 未运行、超时、权限）。
    Connect,
    /// 已建立的连接断开（对端关闭、连接中断、心跳超时）。
    Disconnect,
    /// 对端身份不符（不是 app-mcp、属于其他用户）。
    Identity,
    /// 握手被 Host 拒绝或超时。
    Handshake,
    /// 浏览器拦截（只出现在网页 SDK）。
    Browser,
    /// Host 侧启动失败（锁、端口、IPC 端点）。
    Host,
    /// SDK 本地初始化失败（与 Host 无关）。
    Sdk,
}

impl IssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueKind::Connect => "connect",
            IssueKind::Disconnect => "disconnect",
            IssueKind::Identity => "identity",
            IssueKind::Handshake => "handshake",
            IssueKind::Browser => "browser",
            IssueKind::Host => "host",
            IssueKind::Sdk => "sdk",
        }
    }
}

/// 连接级错误码（spec/protocol.md 10.1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionErrorCode {
    /// 端点上没有监听者（连接被拒绝、套接字文件或命名管道不存在）。
    HostNotRunning,
    /// 规定时间内没能建立连接。
    ConnectTimeout,
    /// 其他建立连接的错误。
    ConnectFailed,
    /// 本地 IPC 权限错误：监听方属于其他用户，或当前用户无权打开端点。
    IpcPermissionDenied,
    /// 已建立的连接被对端正常关闭（收到 Close 帧或连接结束，如 Host 停止 / 重启）。
    ConnectionClosed,
    /// 已建立的连接因 I/O 错误中断（连接被重置、管道断开等，未经关闭握手）。
    ConnectionLost,
    /// 心跳超时：规定时间内没有收到 `ping` 的响应，SDK 主动断开。
    HeartbeatTimeout,
    /// 对端不是 app-mcp Host（`service` 不符、不认识 `app/hello`、握手结果无法解析）。
    HostNotAppMcp,
    /// 对端是其他操作系统用户的 app-mcp Host。
    HostOtherUser,
    /// 握手超时（Host 没有及时回复 `app/hello`）。
    HandshakeTimeout,
    /// 协议版本不兼容。
    ProtocolIncompatible,
    /// 网页来源（Origin）不在 Host 的允许列表中。
    OriginNotAllowed,
    /// 握手参数不合法（appId 格式、保留名、与上游重名、instanceId 为空）。
    InvalidHello,
    /// 用户拒绝了配对请求。
    PairingRejected,
    /// 其他拒绝（Host 未给出错误码，如旧 Host）。
    Rejected,
    /// 浏览器本地网络访问（LNA）权限未授予。
    BlockedLocalNetworkAccess,
    /// 非安全上下文（HTTP 公网页面）不能连接本机。
    BlockedInsecureContext,
    /// 页面的内容安全策略（CSP `connect-src`）不允许连接 Host。
    BlockedCsp,
    /// Host：单实例锁已被持有（同一配置目录已有 Host 在运行）。
    LockHeld,
    /// Host：监听端口被占用。
    PortBusy,
    /// Host：本地 IPC 端点被占用。
    IpcEndpointBusy,
    /// 本地 IPC 端点超过系统上限（Unix `sockaddr_un.sun_path`、Windows 命名管道名 256 字符）。
    IpcPathTooLong,
    /// SDK 本地初始化失败（如网页 SDK 的 WASM 核心加载失败），没有连接 Host。
    SdkInitFailed,
}

impl ConnectionErrorCode {
    /// 全部错误码（文档与测试用）。
    pub const ALL: [ConnectionErrorCode; 23] = [
        Self::HostNotRunning,
        Self::ConnectTimeout,
        Self::ConnectFailed,
        Self::IpcPermissionDenied,
        Self::ConnectionClosed,
        Self::ConnectionLost,
        Self::HeartbeatTimeout,
        Self::HostNotAppMcp,
        Self::HostOtherUser,
        Self::HandshakeTimeout,
        Self::ProtocolIncompatible,
        Self::OriginNotAllowed,
        Self::InvalidHello,
        Self::PairingRejected,
        Self::Rejected,
        Self::BlockedLocalNetworkAccess,
        Self::BlockedInsecureContext,
        Self::BlockedCsp,
        Self::LockHeld,
        Self::PortBusy,
        Self::IpcEndpointBusy,
        Self::IpcPathTooLong,
        Self::SdkInitFailed,
    ];

    /// 线上的字符串形式（如 `"HOST_NOT_RUNNING"`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostNotRunning => "HOST_NOT_RUNNING",
            Self::ConnectTimeout => "CONNECT_TIMEOUT",
            Self::ConnectFailed => "CONNECT_FAILED",
            Self::IpcPermissionDenied => "IPC_PERMISSION_DENIED",
            Self::ConnectionClosed => "CONNECTION_CLOSED",
            Self::ConnectionLost => "CONNECTION_LOST",
            Self::HeartbeatTimeout => "HEARTBEAT_TIMEOUT",
            Self::HostNotAppMcp => "HOST_NOT_APP_MCP",
            Self::HostOtherUser => "HOST_OTHER_USER",
            Self::HandshakeTimeout => "HANDSHAKE_TIMEOUT",
            Self::ProtocolIncompatible => "PROTOCOL_INCOMPATIBLE",
            Self::OriginNotAllowed => "ORIGIN_NOT_ALLOWED",
            Self::InvalidHello => "INVALID_HELLO",
            Self::PairingRejected => "PAIRING_REJECTED",
            Self::Rejected => "REJECTED",
            Self::BlockedLocalNetworkAccess => "BLOCKED_LOCAL_NETWORK_ACCESS",
            Self::BlockedInsecureContext => "BLOCKED_INSECURE_CONTEXT",
            Self::BlockedCsp => "BLOCKED_CSP",
            Self::LockHeld => "LOCK_HELD",
            Self::PortBusy => "PORT_BUSY",
            Self::IpcEndpointBusy => "IPC_ENDPOINT_BUSY",
            Self::IpcPathTooLong => "IPC_PATH_TOO_LONG",
            Self::SdkInitFailed => "SDK_INIT_FAILED",
        }
    }

    /// 解析字符串形式；不认识的码（新版本增加的）返回 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }

    pub fn kind(self) -> IssueKind {
        match self {
            Self::HostNotRunning | Self::ConnectTimeout | Self::ConnectFailed | Self::IpcPermissionDenied => {
                IssueKind::Connect
            }
            Self::ConnectionClosed | Self::ConnectionLost | Self::HeartbeatTimeout => IssueKind::Disconnect,
            Self::HostNotAppMcp | Self::HostOtherUser => IssueKind::Identity,
            Self::HandshakeTimeout
            | Self::ProtocolIncompatible
            | Self::OriginNotAllowed
            | Self::InvalidHello
            | Self::PairingRejected
            | Self::Rejected => IssueKind::Handshake,
            Self::BlockedLocalNetworkAccess | Self::BlockedInsecureContext | Self::BlockedCsp => IssueKind::Browser,
            Self::LockHeld | Self::PortBusy | Self::IpcEndpointBusy | Self::IpcPathTooLong => IssueKind::Host,
            Self::SdkInitFailed => IssueKind::Sdk,
        }
    }

    /// 中文原因（通用说明；具体情况见 [`ConnectionIssue::message`]）。
    pub fn reason(self) -> &'static str {
        match self {
            Self::HostNotRunning => "Host 未运行：端点上没有监听者",
            Self::ConnectTimeout => "规定时间内没能建立连接",
            Self::ConnectFailed => "建立连接失败",
            Self::IpcPermissionDenied => "本地 IPC 端点属于其他用户，或当前用户无权访问",
            Self::ConnectionClosed => "Host 关闭了连接（Host 停止、重启或主动断开）",
            Self::ConnectionLost => "连接意外中断（连接被重置或管道断开）",
            Self::HeartbeatTimeout => "心跳超时：Host 没有及时响应 ping",
            Self::HostNotAppMcp => "对端不是 app-mcp Host（端口被其他程序占用）",
            Self::HostOtherUser => "对端是其他操作系统用户的 app-mcp Host",
            Self::HandshakeTimeout => "Host 没有及时回复握手",
            Self::ProtocolIncompatible => "SDK 与 Host 的协议版本不兼容",
            Self::OriginNotAllowed => "网页来源不在 Host 的允许列表中",
            Self::InvalidHello => "握手参数不合法（appId、instanceId 等）",
            Self::PairingRejected => "用户拒绝了配对请求",
            Self::Rejected => "Host 拒绝了连接",
            Self::BlockedLocalNetworkAccess => "浏览器的本地网络访问权限未授予",
            Self::BlockedInsecureContext => "非 HTTPS 的公网页面不能连接本机",
            Self::BlockedCsp => "页面的内容安全策略（connect-src）不允许连接 Host",
            Self::LockHeld => "同一配置目录已有 Host 在运行（单实例锁被持有）",
            Self::PortBusy => "Host 的监听端口被占用",
            Self::IpcEndpointBusy => "Host 的本地 IPC 端点被占用",
            Self::IpcPathTooLong => {
                "本地 IPC 端点超过系统上限（Unix 套接字路径：Linux 107 字节、macOS 103 字节；Windows 命名管道名：256 字符）"
            }
            Self::SdkInitFailed => "SDK 本地初始化失败（未连接 Host）",
        }
    }

    /// 中文修复建议。
    pub fn hint(self) -> &'static str {
        match self {
            Self::HostNotRunning => {
                "启动 Host：app-mcp-host serve（或 service install 登录自启）；运行 app-mcp-host doctor 查看端点"
            }
            Self::ConnectTimeout => "检查 Host 是否卡住（app-mcp-host doctor），以及防火墙 / 代理是否拦截回环连接",
            Self::ConnectFailed => "查看 SDK 日志中的系统错误，并运行 app-mcp-host doctor",
            Self::ConnectionClosed => {
                "SDK 会自动重连；若 Host 已停止，启动它（app-mcp-host serve / service start）；频繁出现时查看 Host 日志"
            }
            Self::ConnectionLost => {
                "SDK 会自动重连；频繁出现时检查 Host 是否崩溃（app-mcp-host doctor、Host 日志）以及代理 / 安全软件是否切断连接"
            }
            Self::HeartbeatTimeout => {
                "SDK 会自动重连；Host 可能卡住或过载：查看 Host 日志，必要时重启（app-mcp-host service stop / start）"
            }
            Self::IpcPermissionDenied => {
                "以同一用户运行 Host 与 App；确认套接字目录为 0700 且属于当前用户（app-mcp-host doctor 会检查）"
            }
            Self::HostNotAppMcp => {
                "停止占用端口的程序（app-mcp-host doctor 会给出进程），或用 APP_MCP_ENDPOINT / hostUrl 指定正确端点"
            }
            Self::HostOtherUser => "启动自己的 Host（app-mcp-host serve），或用 APP_MCP_ENDPOINT 指定自己的端点",
            Self::HandshakeTimeout => "Host 可能过载或卡住：查看 Host 日志，必要时重启（app-mcp-host service stop / start）",
            Self::ProtocolIncompatible => "升级 SDK 或 Host 到相同的协议版本",
            Self::OriginNotAllowed => "把页面来源加入允许列表：app-mcp-host serve --allow-origin <来源>",
            Self::InvalidHello => "检查 appId（[a-z][a-z0-9-]{0,62}，不能是保留名或上游名）与 instanceId",
            Self::PairingRejected => "在 Host 的配对提示中允许该 App 后重试（wake() / connectNow()）",
            Self::Rejected => "查看原因说明与 Host 日志",
            Self::BlockedLocalNetworkAccess => "在浏览器地址栏的站点设置中允许「本机上的应用」（本地网络访问），授权后自动重连",
            Self::BlockedInsecureContext => "改用 HTTPS 发布页面，或在 localhost 上打开",
            Self::BlockedCsp => "在 CSP 的 connect-src 中加入 ws://127.0.0.1:7717（及备选端口 7737、7757）后刷新页面",
            Self::LockHeld => "无需处理（已有实例在服务）；如需重启先 app-mcp-host service stop",
            Self::PortBusy => {
                "运行 app-mcp-host doctor 查看占用端口的进程并停止它，或用 --listen 指定其他端口"
            }
            Self::IpcEndpointBusy => "另一个配置目录的 Host 正在使用该端点：停止它，或用 --ipc-endpoint 指定其他端点",
            Self::IpcPathTooLong => {
                r"用 --ipc-endpoint unix:<较短的绝对路径> / pipe:\\.\pipe\<较短名称>（嵌入式 Hub 为 HubConfig.ipc_endpoint，SDK 为 APP_MCP_ENDPOINT / host_url）指定较短端点，或缩短 XDG_RUNTIME_DIR / --home 所在路径"
            }
            Self::SdkInitFailed => "检查 WASM 文件地址（wasmUrl）能否加载、页面 CSP 是否允许 WebAssembly（'wasm-unsafe-eval'），以及浏览器控制台中的错误",
        }
    }
}

impl std::fmt::Display for ConnectionErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一次连接问题：错误码 + 具体说明（中文）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionIssue {
    pub code: ConnectionErrorCode,
    pub message: String,
}

impl ConnectionIssue {
    pub fn new(code: ConnectionErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::error::Error for ConnectionIssue {}

impl std::fmt::Display for ConnectionIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// 按建立连接时的 I/O 错误归类错误码（原生 SDK 的驱动层使用）。
pub fn connect_error_code(kind: std::io::ErrorKind) -> ConnectionErrorCode {
    use std::io::ErrorKind as K;
    match kind {
        K::ConnectionRefused | K::NotFound | K::AddrNotAvailable => ConnectionErrorCode::HostNotRunning,
        K::TimedOut => ConnectionErrorCode::ConnectTimeout,
        K::PermissionDenied => ConnectionErrorCode::IpcPermissionDenied,
        _ => ConnectionErrorCode::ConnectFailed,
    }
}

/// `app/diagnostic`（SDK → Host，通知）的参数：此前遇到的连接问题，在连接恢复（握手成功）后上报。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticParams {
    /// 错误码（[`ConnectionErrorCode`] 的字符串形式；可能是接收方不认识的新码）。
    pub code: String,
    /// 中文说明（最近一次）。
    pub message: String,
    /// 自上次上报以来发生的次数（≥ 1）。
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for c in ConnectionErrorCode::ALL {
            assert_eq!(ConnectionErrorCode::parse(c.as_str()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), serde_json::json!(c.as_str()));
            assert!(!c.reason().is_empty() && !c.hint().is_empty());
        }
        assert_eq!(ConnectionErrorCode::parse("SOMETHING_NEW"), None);
        assert_eq!(ConnectionErrorCode::BlockedCsp.kind(), IssueKind::Browser);
        assert_eq!(ConnectionErrorCode::HeartbeatTimeout.kind(), IssueKind::Disconnect);
        assert_eq!(serde_json::to_value(IssueKind::Disconnect).unwrap(), serde_json::json!("disconnect"));
        assert_eq!(ConnectionErrorCode::parse("CONNECTION_CLOSED"), Some(ConnectionErrorCode::ConnectionClosed));
    }

    /// spec/protocol.md 的错误码表必须列出每个错误码。
    #[test]
    fn spec_lists_every_code() {
        let spec = include_str!("../../../spec/protocol.md");
        for c in ConnectionErrorCode::ALL {
            assert!(spec.contains(&format!("`{}`", c.as_str())), "spec/protocol.md 缺少错误码 {c}");
        }
    }

    #[test]
    fn io_classification() {
        use std::io::ErrorKind as K;
        assert_eq!(connect_error_code(K::ConnectionRefused), ConnectionErrorCode::HostNotRunning);
        assert_eq!(connect_error_code(K::NotFound), ConnectionErrorCode::HostNotRunning);
        assert_eq!(connect_error_code(K::PermissionDenied), ConnectionErrorCode::IpcPermissionDenied);
        assert_eq!(connect_error_code(K::TimedOut), ConnectionErrorCode::ConnectTimeout);
        assert_eq!(connect_error_code(K::BrokenPipe), ConnectionErrorCode::ConnectFailed);
    }

    #[test]
    fn diagnostic_params() {
        let p: DiagnosticParams =
            serde_json::from_value(serde_json::json!({"code": "BLOCKED_CSP", "message": "m"})).unwrap();
        assert_eq!(p.count, 1);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, serde_json::json!({"code": "BLOCKED_CSP", "message": "m", "count": 1}));
    }
}
