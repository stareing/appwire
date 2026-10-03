//! 事件（`Hub::events`）。

use app_mcp_protocol::Visibility;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HubEvent {
    AppConnected {
        app_id: String,
        instance_id: String,
    },
    AppDisconnected {
        app_id: String,
        instance_id: String,
    },
    /// 已按 `list_changed_debounce` 合并。
    ToolsChanged,
    /// 已按 `list_changed_debounce` 合并。
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
    /// 实例与 Hub 完成 `app/sleep` 握手，进入休眠（工具仍列出，可唤醒；不另发 `ToolsChanged`）。
    AppDormant {
        app_id: String,
        instance_id: String,
    },
    /// Hub 正在唤醒 App：`instance_id` 为被唤醒的休眠实例；`None` 表示 App 未运行，按清单冷启动。
    AppWaking {
        app_id: String,
        instance_id: Option<String>,
    },
    /// SDK 上报了此前遇到的连接问题（`app/diagnostic`，spec/protocol.md 10.2），如浏览器拦截。
    AppDiagnostic {
        app_id: String,
        instance_id: String,
        /// 错误码（spec/protocol.md 10.1；可能是本 Hub 不认识的新码）。
        code: String,
        message: String,
        count: u32,
    },
    /// App 发出的事件（`events/emit`，已去重与校验；spec/hub-api.md 3.17），不论有无订阅。
    AppEvent(crate::events::AppEvent),
}
