//! 事件，以及审批 / 配对 / 唤醒请求。

use app_mcp_hub as hub;
use serde_json::Value;

use super::{Risk, ToolAnnotations, Visibility, WakeKind};

#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum HubEvent {
    AppConnected {
        app_id: String,
        instance_id: String,
    },
    AppDisconnected {
        app_id: String,
        instance_id: String,
    },
    /// 已合并；收到后应重新列工具 / 导出。
    ToolsChanged,
    ResourcesChanged,
    ResourceUpdated {
        uri: String,
    },
    VisibilityChanged {
        app_id: String,
        instance_id: String,
        visibility: Visibility,
    },
    UpstreamState {
        name: String,
        connected: bool,
        error: Option<String>,
    },
    /// 实例进入休眠：工具仍列出（`Availability::Dormant`），不另发 `ToolsChanged`。
    AppDormant {
        app_id: String,
        instance_id: String,
    },
    /// Hub 正在唤醒 App；`instance_id` 为空表示 App 未运行、按清单冷启动。
    AppWaking {
        app_id: String,
        instance_id: Option<String>,
    },
    /// SDK 上报了此前遇到的连接问题（`app/diagnostic`，spec/protocol.md 10.2），如浏览器拦截。
    /// `code` 为错误码（10.1；可能是本 Hub 不认识的新码），`count` 为合并的次数。
    AppDiagnostic {
        app_id: String,
        instance_id: String,
        code: String,
        message: String,
        count: u32,
    },
    /// 本绑定尚未单独映射的 Hub 事件（兜底，兼容未来新增）。`kind` 为事件类型名，
    /// `json` 为事件的完整 JSON 文本。
    Other {
        kind: String,
        json: String,
    },
}

impl From<hub::HubEvent> for HubEvent {
    fn from(e: hub::HubEvent) -> Self {
        use hub::HubEvent as H;
        match e {
            H::AppConnected {
                app_id,
                instance_id,
            } => HubEvent::AppConnected {
                app_id,
                instance_id,
            },
            H::AppDisconnected {
                app_id,
                instance_id,
            } => HubEvent::AppDisconnected {
                app_id,
                instance_id,
            },
            H::ToolsChanged => HubEvent::ToolsChanged,
            H::ResourcesChanged => HubEvent::ResourcesChanged,
            H::ResourceUpdated { uri } => HubEvent::ResourceUpdated { uri },
            H::VisibilityChanged {
                app_id,
                instance_id,
                visibility,
            } => HubEvent::VisibilityChanged {
                app_id,
                instance_id,
                visibility: visibility.into(),
            },
            H::UpstreamState {
                name,
                connected,
                error,
            } => HubEvent::UpstreamState {
                name,
                connected,
                error,
            },
            H::AppDormant {
                app_id,
                instance_id,
            } => HubEvent::AppDormant {
                app_id,
                instance_id,
            },
            H::AppWaking {
                app_id,
                instance_id,
            } => HubEvent::AppWaking {
                app_id,
                instance_id,
            },
            H::AppDiagnostic {
                app_id,
                instance_id,
                code,
                message,
                count,
            } => HubEvent::AppDiagnostic {
                app_id,
                instance_id,
                code,
                message,
                count,
            },
            #[allow(unreachable_patterns)]
            other => other_event(&other),
        }
    }
}

/// 兜底映射：取 serde 标签 `type` 作为类别，整段 JSON 原样透传。
#[allow(dead_code)]
pub(super) fn other_event(e: &hub::HubEvent) -> HubEvent {
    let value = serde_json::to_value(e).unwrap_or(Value::Null);
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    HubEvent::Other {
        kind,
        json: value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 审批 / 配对请求
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub app_id: String,
    pub app_name: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    pub risk: Risk,
    /// 参数 JSON 文本（已按 schema 校验）。
    pub arguments_json: String,
    pub session: Option<String>,
    /// 工具的 MCP 注解（与 `HubTool.annotations` 相同），供厂商按声明决定是否确认。
    pub annotations: ToolAnnotations,
    /// MCP 出口：发起调用的认证主体（取自传输层凭据，现在恒为 `local`）；经 `call_tool` 发起时为空。
    #[uniffi(default = None)]
    pub principal: Option<String>,
    /// MCP 出口：客户端自报的 `clientInfo.name`；经 `call_tool` 发起时为空。
    /// 自报、不可信，**仅供显示**，不得据此做授权决定。
    #[uniffi(default = None)]
    pub client_name: Option<String>,
}

impl From<hub::ApprovalRequest> for ApprovalRequest {
    fn from(r: hub::ApprovalRequest) -> Self {
        ApprovalRequest {
            call_id: r.call_id,
            app_id: r.app_id,
            app_name: r.app_name,
            tool: r.tool,
            title: r.title,
            description: r.description,
            risk: r.risk.into(),
            arguments_json: r.arguments.to_string(),
            session: r.session,
            annotations: r.annotations.into(),
            principal: r.principal,
            client_name: r.client_name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct PairingRequest {
    pub app_id: String,
    pub app_name: String,
    pub origin: Option<String>,
    pub client_kind: String,
    pub instance_id: String,
}

impl From<hub::PairingRequest> for PairingRequest {
    fn from(r: hub::PairingRequest) -> Self {
        PairingRequest {
            app_id: r.app_id,
            app_name: r.app_name,
            origin: r.origin,
            client_kind: r.client_kind,
            instance_id: r.instance_id,
        }
    }
}

// ---------------------------------------------------------------------------
// 唤醒请求
// ---------------------------------------------------------------------------

/// 唤醒描述。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WakeDescriptor {
    pub kind: WakeKind,
    /// scheme、AUMID、bundle id、D-Bus 名称、组件名或 URL。
    pub target: Option<String>,
    /// 能否不把窗口带到前台就唤醒。
    pub background: bool,
}

/// 交给 `HubWaker` 的唤醒请求（spec/hub-api.md 3.5）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WakeRequest {
    pub app_id: String,
    /// 被唤醒的休眠实例；为空表示 App 未运行，按清单冷启动。
    pub instance_id: Option<String>,
    pub descriptor: WakeDescriptor,
    /// 一次性唤醒令牌（32 位十六进制）。
    pub token: String,
    /// 通用激活参数 `app-mcp-wake:<token>`，App 端 SDK 的 `handleWake` 可识别。
    pub activation_arg: String,
}

impl From<hub::WakeRequest> for WakeRequest {
    fn from(r: hub::WakeRequest) -> Self {
        WakeRequest {
            app_id: r.app_id,
            instance_id: r.instance_id,
            descriptor: WakeDescriptor {
                kind: r.descriptor.kind.into(),
                target: r.descriptor.target,
                background: r.descriptor.background,
            },
            token: r.token,
            activation_arg: r.activation_arg,
        }
    }
}
