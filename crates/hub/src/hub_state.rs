//! Hub 自身状态作为只读 MCP 资源（第 16 项 P7；spec/hub-api.md 3.6「Hub 状态资源」）：Agent 用 `resources/read` 自查，
//! 不必由机主转述 `status`。
//!
//! - `app-mcp://apps/hub`（[`HubStateView`]）：App 概况与未到期的对象锁。
//! - `app-mcp://apps/self`（[`SelfStateView`]）：读取方自己的任务（含其任务句柄）、所持的锁、进行中的调用、记账用量与 Agent 配额余量。
//!
//! 读取时现算；不可订阅（状态随每次调用变化，变化通知会给 Hub 增加推送流量，Agent 需要时再读）。
//!
//! @security 不给任务 ID（句柄是凭据）、调用方键与会话号；锁的持有者只给记账主体（与 `LOCKED` 错误相同）；
//! 被 `hide` 的 App 及其上的锁不出现。

use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::model::{ReadResourceResult, ResourceContents};
use serde::{Deserialize, Serialize};

use crate::hub::{DEFAULT_MIME, HubShared, lock};
use crate::names::{BUILTIN_APP_ID, RESOURCE_HUB, RESOURCE_SELF};
use crate::object_lock::LockStatus;
use crate::task::CallerKey;
use crate::types::{AgentTaskStatus, AppKind, AppState, InstanceState, TaskLeaseStatus, TaskSelectionStatus};
use crate::usage::UsageStatus;

/// `app-mcp://apps/hub` 的内容。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubStateView {
    /// 按 appId 排序（含上游）。
    pub apps: Vec<AppStateView>,
    pub locks: Vec<LockView>,
}

/// 一个 App 的概况（实例详情用 `apps.list`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateView {
    pub app_id: String,
    pub name: String,
    pub kind: AppKind,
    pub state: AppState,
    /// 已连接的实例数。
    pub connected: usize,
    /// 休眠实例数（含正在唤醒的）。
    pub dormant: usize,
}

/// 一把未到期的锁。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockView {
    pub app_id: String,
    /// 命名锁的名字；App 锁为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// 持有者的记账主体（`agent:<名>` / `local` / `api`）。
    pub holder: String,
    pub expires_in_ms: u64,
}

/// `app-mcp://apps/self` 的内容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfStateView {
    /// 读取方的记账主体。
    pub subject: String,
    /// 已登记 Agent 的名字；本机用户与 Hub API 为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// 读取方的任务：自身任务在前，其后为其任务句柄。
    pub tasks: Vec<SelfTaskView>,
    /// 读取方（含其任务句柄）持有的锁。
    pub locks: Vec<LockView>,
    /// 读取方记账主体的累计用量（同一 Agent 的所有会话 / 句柄合计）；尚无记录时为 `None`。
    pub usage: Option<UsageStatus>,
    /// 每 Agent 配额（`limits.agent_rate`）；读取方不是已登记 Agent 或该级不限时为 `None`。
    pub quota: Option<QuotaView>,
    /// 读取方（含其任务句柄）进行中的调用（第 16 项 P5，[`crate::call_objects`]）。
    #[serde(default)]
    pub calls: Vec<crate::call_objects::CallStatus>,
}

/// 读取方的一个任务（不含任务 ID）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTaskView {
    /// 是否为 `apps.task.begin` 签发的任务句柄。
    pub handle: bool,
    pub selections: Vec<TaskSelectionStatus>,
    pub leases: Vec<TaskLeaseStatus>,
    pub inflight: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
}

/// 每 Agent 配额的余量。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaView {
    pub per_minute: u32,
    pub burst: u32,
    /// 此刻还可发起的调用数（令牌数向下取整）。
    pub available: u32,
}

/// `resources/list` 中的 Hub 状态资源。
#[cfg(feature = "mcp-server")]
pub(crate) fn hub_state_resources() -> Vec<rmcp::model::Resource> {
    [
        (RESOURCE_HUB, "Hub 状态：各 App 的连接状态、在线 / 休眠实例数与当前的对象锁（只读）"),
        (RESOURCE_SELF, "本调用方的状态：自己的任务、选择、租约、持有的锁、累计用量与配额余量（只读）"),
    ]
    .into_iter()
    .map(|(name, desc)| {
        rmcp::model::Resource::new(crate::hub::resource_uri(BUILTIN_APP_ID, name), format!("{BUILTIN_APP_ID}.{name}"))
            .with_description(desc)
            .with_mime_type(DEFAULT_MIME)
    })
    .collect()
}

/// 读取 Hub 状态资源 `app-mcp://apps/<name>`。
///
/// @error 未知的 `name` → `resource_not_found`。
pub(crate) fn read_hub_state(shared: &Arc<HubShared>, name: &str, uri: &str, caller: &CallerKey) -> Result<ReadResourceResult, McpError> {
    let text = match name {
        RESOURCE_HUB => serde_json::to_string(&shared.hub_state_view()),
        RESOURCE_SELF => serde_json::to_string(&shared.self_state_view(caller)),
        _ => return Err(McpError::resource_not_found(format!("资源「{uri}」不存在"), None)),
    }
    .map_err(|e| McpError::internal_error(format!("序列化 Hub 状态失败：{e}"), None))?;
    Ok(ReadResourceResult::new(vec![ResourceContents::text(text, uri).with_mime_type(DEFAULT_MIME)]))
}

/// `uri` 是否为 Hub 状态资源（不可订阅）。
pub(crate) fn is_hub_state_uri(uri: &str) -> bool {
    crate::hub::parse_resource_uri(uri).is_some_and(|(app_id, _)| app_id == BUILTIN_APP_ID)
}

fn lock_view(l: LockStatus) -> LockView {
    LockView { app_id: l.app_id, key: l.key, holder: l.holder, expires_in_ms: l.expires_in_ms }
}

/// `task_caller` 是否为 `owner` 本身或其名下的任务句柄（调用方键 `<owner>/<任务 ID>`，见 [`CallerKey::task_handle`]）。
pub(crate) fn owned_by(task_caller: &str, owner: &str) -> bool {
    task_caller.strip_prefix(owner).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn self_task_view(t: AgentTaskStatus, owner: &str) -> SelfTaskView {
    SelfTaskView {
        handle: t.caller != owner,
        selections: t.selections,
        leases: t.leases,
        inflight: t.inflight,
        idle_ms: t.idle_ms,
    }
}

impl HubShared {
    fn hub_state_view(&self) -> HubStateView {
        let status = self.status();
        let apps = status
            .apps
            .into_iter()
            .filter(|a| !self.app_hidden(&a.app_id))
            .map(|a| {
                let dormant = a.instances.iter().filter(|i| i.state != InstanceState::Connected).count();
                AppStateView {
                    connected: a.instances.len() - dormant,
                    dormant,
                    app_id: a.app_id,
                    name: a.name,
                    kind: a.kind,
                    state: a.state,
                }
            })
            .collect();
        let locks = status.locks.unwrap_or_default().into_iter().filter(|l| !self.app_hidden(&l.app_id)).map(lock_view).collect();
        HubStateView { apps, locks }
    }

    fn self_state_view(&self, caller: &CallerKey) -> SelfStateView {
        let owner = caller.as_str();
        let subject = caller.usage_subject();
        let mut tasks: Vec<AgentTaskStatus> = self.task_statuses().into_iter().filter(|t| owned_by(&t.caller, owner)).collect();
        // 自身任务在前（其调用方键是句柄键的前缀，按键排序后自然在前）。
        tasks.sort_by(|a, b| a.caller.cmp(&b.caller));
        let locks = self.lock_status().into_iter().filter(|l| owned_by(&l.caller, owner)).map(lock_view).collect();
        let usage = lock(&self.usage).status().into_iter().find(|u| u.subject == subject);
        let quota = caller.agent().and_then(|agent| {
            let limit = self.config.limits.agent_rate;
            let available = lock(&self.rates).agent_available(&self.config.limits, agent.as_str(), tokio::time::Instant::now())?;
            Some(QuotaView { per_minute: limit.per_minute, burst: limit.burst, available })
        });
        SelfStateView {
            subject,
            agent: caller.agent().map(|a| a.as_str().to_owned()),
            tasks: tasks.into_iter().map(|t| self_task_view(t, owner)).collect(),
            locks,
            usage,
            quota,
            calls: self.own_calls(caller, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_covers_handles_only() {
        assert!(owned_by("principal:local", "principal:local"));
        assert!(owned_by("principal:local/task-1", "principal:local"));
        assert!(!owned_by("principal:localhost", "principal:local"), "前缀相同的其他主体");
        assert!(!owned_by("principal:agent:claude", "principal:local"));
        assert!(!owned_by("mcp:12", "mcp:1"));
    }

    #[test]
    fn only_builtin_namespace_is_hub_state() {
        assert!(is_hub_state_uri("app-mcp://apps/self"));
        assert!(is_hub_state_uri("app-mcp://apps/other"));
        assert!(!is_hub_state_uri("app-mcp://shop/cart"));
        assert!(!is_hub_state_uri("app-mcp://apps"));
    }
}
