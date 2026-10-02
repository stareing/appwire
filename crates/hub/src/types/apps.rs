//! App 与实例：来源、App 信息、实例信息。

use app_mcp_protocol::Visibility;
use serde::{Deserialize, Serialize};

/// App 的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    /// 通过 App 端 SDK 连接（或只有静态清单）的 App。
    App,
    /// Hub 以子进程启动的上游 MCP 服务器。
    Upstream,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    /// 总览中的一句话简介。
    pub summary: Option<String>,
    pub connected: bool,
    /// 按连接顺序排列；上游为空。
    pub instances: Vec<InstanceInfo>,
    /// [`crate::Hub::select_instance`] 选定且仍连接的实例。
    pub selected_instance: Option<String>,
    /// 休眠中的实例（spec/lifecycle.md §9）：已与 Hub 完成 `app/sleep` 握手后断开，保留工具快照，
    /// 调用其工具时按需唤醒。按休眠时间排列。
    #[serde(default)]
    pub dormant_instances: Vec<InstanceInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub instance_id: String,
    /// `web` / `native` / `hybrid`。
    pub client_kind: String,
    /// 尚未上报时为 `visible`。
    pub visibility: Visibility,
    pub focused: bool,
    /// 最近活跃时间（Unix 毫秒）；从未活跃时为连接时间。
    pub last_active_ms: u64,
    /// 实例标题（网页为 `document.title`）。spec 之外的补充字段。
    pub title: Option<String>,
    /// 实例进程号：经本地 IPC（Unix 域套接字 / 命名管道）连接时由操作系统提供；
    /// 回环 TCP 连接与休眠实例为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// 连接 ID（spec/protocol.md 10.3），与 Hub 日志的 `cid` 字段、SDK 日志中的连接 ID 相同；休眠实例为 `None`。
    /// spec 之外的补充字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
}
