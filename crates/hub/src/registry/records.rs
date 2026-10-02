//! 注册表登记的记录类型：新实例、已连接实例、休眠实例、唤醒计划、名字服务条目、列表项与路由目标。

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

use app_mcp_protocol::{
    AppOverview, ClientKind, ResourceInfo, ResourcesSyncParams, ToolsSyncParams, Visibility, WakeDescriptor, WakeKind,
};

use crate::connection::Connection;
use crate::routing::Candidate;
use crate::tool_def::SharedTool;

use super::Availability;

/// 握手成功后登记的新实例。
#[derive(Debug, Clone)]
pub struct NewInstance {
    pub instance_id: String,
    pub app_name: String,
    pub client_kind: ClientKind,
    pub app_version: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    /// `app/hello` 中携带的总览。
    pub overview: Option<AppOverview>,
    /// 对端进程号（本地 IPC 连接由操作系统提供；TCP 连接为 `None`）。
    pub pid: Option<u32>,
    /// 握手声明了 `capabilities.navigate`（spec/protocol.md 3.4）。
    pub navigate: bool,
    pub conn: Arc<Connection>,
    /// `app/hello` 中的唤醒描述（`none` 已滤掉）。
    pub wake: Option<WakeDescriptor>,
}

/// 一个已连接的实例（标签页 / 进程）。
#[derive(Debug)]
pub struct Instance {
    pub instance_id: String,
    pub app_name: String,
    pub client_kind: ClientKind,
    pub app_version: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub overview: Option<AppOverview>,
    /// 对端进程号（本地 IPC 连接）。
    pub pid: Option<u32>,
    /// 能处理 `app/navigate`（握手声明了 `capabilities.navigate`）。
    pub navigate: bool,
    pub conn: Arc<Connection>,
    /// `app/hello` 中的唤醒描述：有它的实例才持久化（spec/hub-api.md 3.5「持久化」）。
    pub wake: Option<WakeDescriptor>,
    /// 尚未收到 `app/visibility` 时为 `None`。
    pub visibility: Option<Visibility>,
    pub focused: bool,
    pub ready: bool,
    pub connected_at: SystemTime,
    pub connected_seq: u64,
    pub last_active_at: Option<SystemTime>,
    pub last_active_seq: Option<u64>,
    pub tools: BTreeMap<String, SharedTool>,
    pub resources: BTreeMap<String, ResourceInfo>,
    /// Host 已向该实例订阅的资源名。
    pub subscriptions: HashSet<String>,
}

impl Instance {
    /// 在线实例的持久化视图（spec/hub-api.md 3.5「持久化」）：Host 异常退出后按休眠实例读回、经唤醒描述唤醒。
    /// 没有恢复令牌（`resume_token` 为空，回连时总是完整同步）；`slept_at` 取写出时刻，保留期由此起算。
    pub(super) fn persisted(&self, now: SystemTime) -> Option<DormantInstance> {
        let wake = self.wake.clone()?;
        self.ready.then(|| DormantInstance {
            instance_id: self.instance_id.clone(),
            app_name: self.app_name.clone(),
            client_kind: self.client_kind,
            app_version: self.app_version.clone(),
            title: self.title.clone(),
            url: self.url.clone(),
            overview: self.overview.clone(),
            visibility: self.visibility,
            tools: self.tools.clone(),
            resources: self.resources.clone(),
            resume_token: String::new(),
            tools_hash: String::new(),
            wake: Some(wake),
            slept_at: now,
            connected_at: self.connected_at,
            last_active_at: self.last_active_at,
            recency: self.last_active_seq.unwrap_or(self.connected_seq),
        })
    }

    /// Host 是否订阅了本实例声明 `realtime` 的资源（spec/lifecycle.md 第 13 节 B3：只有这类订阅阻止休眠）。
    pub fn has_realtime_subscription(&self) -> bool {
        self.subscriptions.iter().any(|n| self.resources.get(n).is_some_and(|r| r.realtime))
    }

    pub(super) fn candidate(&self) -> Candidate<'_> {
        Candidate {
            instance_id: &self.instance_id,
            focused: self.focused,
            last_active: self.last_active_seq,
            connected: self.connected_seq,
        }
    }
}

/// 休眠中的实例（spec/lifecycle.md §9）：`app/sleep` 被接受后的工具 / 资源快照与恢复信息。
#[derive(Debug, Clone)]
pub struct DormantInstance {
    pub instance_id: String,
    pub app_name: String,
    pub client_kind: ClientKind,
    pub app_version: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub overview: Option<AppOverview>,
    pub visibility: Option<Visibility>,
    /// 与休眠前的实例共享同一份定义。
    pub tools: BTreeMap<String, SharedTool>,
    pub resources: BTreeMap<String, ResourceInfo>,
    /// 本 Hub 发给 SDK 的恢复令牌（一次性）。
    pub resume_token: String,
    /// SDK 休眠时上报的 `toolsHash`。
    pub tools_hash: String,
    /// SDK 休眠时上报的唤醒描述。
    pub wake: Option<WakeDescriptor>,
    pub slept_at: SystemTime,
    pub connected_at: SystemTime,
    pub last_active_at: Option<SystemTime>,
    /// 路由优先级用：休眠前最近活跃（或连接）的序号。
    pub recency: u64,
}

impl DormantInstance {
    /// 按快照重算的 `toolsHash`（spec/protocol.md 8.4）。
    pub fn snapshot_hash(&self) -> String {
        app_mcp_protocol::tools_hash(
            &ToolsSyncParams {
                tools: self.tools.values().map(|t| t.to_info()).collect(),
            },
            &ResourcesSyncParams {
                resources: self.resources.values().cloned().collect(),
            },
        )
    }

    /// 可用的唤醒描述（`none` 视为未上报）。
    pub fn wake_descriptor(&self) -> Option<&WakeDescriptor> {
        self.wake.as_ref().filter(|w| w.kind != WakeKind::None)
    }
}

/// 需要先唤醒才能派发的调用目标。
#[derive(Debug, Clone)]
pub struct WakePlan {
    pub app_id: String,
    /// 被唤醒的休眠实例；`None` = App 未运行，按清单冷启动。
    pub instance_id: Option<String>,
    /// 休眠实例上报的唤醒描述（`None` 时由调用方按清单解析）。
    pub descriptor: Option<WakeDescriptor>,
    /// 快照（或清单）中的定义，用于唤醒前的 schema 校验与审批。
    pub tool: Option<SharedTool>,
}

/// [`Registry::wake_target_presence`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeTargetPresence {
    /// 已连接并就绪（`app/ready`）：直接派发，不必唤醒。
    Ready(String),
    /// 已连接、握手未完成：等它的 `app/ready`，不再激活。
    Handshaking,
    /// 没有连接：需要激活。
    Absent,
}

/// 名字服务发现到的 App（spec/naming.md 5.4，`source: "name-service"` / 激活文件即 `install`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedEntry {
    /// 发现它的连接器（[`crate::connector::Connector::kind`]，如 `"dbus"`）。
    pub source: String,
    /// 连接器在 [`crate::HubConfig::connectors`] 中的位置。
    pub connector: usize,
    /// 平台名字（如 D-Bus 总线名）。
    pub detail: String,
    /// 可被系统激活（有激活文件）。
    pub activatable: bool,
    /// 名字的所有者正在运行。
    pub running: bool,
    pub first_seen: SystemTime,
    pub last_seen: SystemTime,
    /// 该 App 的静态清单来自连接器的安装元数据（[`crate::connector::Connector::manifest`]）：卸载事件时随记录一起移除。
    pub install_manifest: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListedTool {
    pub app_id: String,
    pub info: SharedTool,
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListedResource {
    pub app_id: String,
    pub info: ResourceInfo,
    pub available: bool,
}

/// 工具调用的目标。
#[derive(Debug, Clone)]
pub struct ToolTarget {
    pub instance_id: String,
    pub conn: Arc<Connection>,
    pub tool: SharedTool,
}

/// 资源读取的目标。
#[derive(Debug, Clone)]
pub struct ResourceTarget {
    pub instance_id: String,
    pub conn: Arc<Connection>,
    pub resource: ResourceInfo,
}
