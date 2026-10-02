//! App 与实例信息。

use app_mcp_hub as hub;

use super::{AppKind, Visibility};

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct InstanceInfo {
    pub instance_id: String,
    /// `web` / `native` / `hybrid`。
    pub client_kind: String,
    pub visibility: Visibility,
    pub focused: bool,
    /// 最近活跃时间（Unix 毫秒）。
    pub last_active_ms: u64,
    pub title: Option<String>,
    /// 实例进程号（经本地 IPC 连接时由操作系统提供；否则为空）。
    pub pid: Option<u32>,
    /// Hub 分配的连接 ID（spec/protocol.md 10.3），与 Hub 日志的 `cid`、SDK 日志中的连接 ID 相同；休眠实例为空。
    pub connection_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppInfo {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    pub summary: Option<String>,
    pub connected: bool,
    pub instances: Vec<InstanceInfo>,
    pub selected_instance: Option<String>,
    /// 休眠中的实例（按休眠时间排列）；调用其工具时 Hub 先唤醒。`connected` 只看已连接实例。
    pub dormant_instances: Vec<InstanceInfo>,
}

impl From<hub::InstanceInfo> for InstanceInfo {
    fn from(i: hub::InstanceInfo) -> Self {
        InstanceInfo {
            instance_id: i.instance_id,
            client_kind: i.client_kind,
            visibility: i.visibility.into(),
            focused: i.focused,
            last_active_ms: i.last_active_ms,
            title: i.title,
            pid: i.pid,
            connection_id: i.connection_id,
        }
    }
}

impl From<hub::AppInfo> for AppInfo {
    fn from(a: hub::AppInfo) -> Self {
        AppInfo {
            app_id: a.app_id,
            name: a.name,
            kind: a.kind.into(),
            summary: a.summary,
            connected: a.connected,
            instances: a.instances.into_iter().map(Into::into).collect(),
            selected_instance: a.selected_instance,
            dormant_instances: a.dormant_instances.into_iter().map(Into::into).collect(),
        }
    }
}
