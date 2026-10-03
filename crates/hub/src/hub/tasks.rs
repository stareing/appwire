//! Agent 任务（调用方的跨请求状态，`crate::task`）：实例选择、总览附带、任务结束。

use std::collections::HashMap;
use std::sync::MutexGuard;
use std::time::Duration;

use crate::overview::Overview;
use crate::task::{CallerKey, TaskTable};

use super::{HubShared, lock};

impl HubShared {
    // ------------------------------------------------------------------
    // Agent 任务（调用方的跨请求状态，crate::task）
    // ------------------------------------------------------------------

    /// 结束调用方的任务：收回其全部租约、删除其租约统计与状态、按调用方键归属的事件订阅与信箱（MCP 会话结束、
    /// `Hub::reset_session`、空闲回收、`apps.task.end`）。
    pub(crate) fn end_task(&self, key: &CallerKey) {
        self.release_leases(key);
        lock(&self.agent_tasks).remove(key);
        self.events_end_owner(key);
    }

    pub(crate) fn agent_tasks(&self) -> MutexGuard<'_, TaskTable> {
        lock(&self.agent_tasks)
    }

    /// 调用方 `apps.select` 选择的空闲有效期：只有无会话主体本身（主体级选择）有（[`HubConfig::principal_select_ttl`]）。
    /// 任务句柄的选择只属于该任务，随任务结束 / 空闲回收清除，不另设有效期。
    pub(crate) fn selection_ttl(&self, key: &CallerKey) -> Option<Duration> {
        let ttl = self.config.principal_select_ttl;
        (key.can_own_handles() && !ttl.is_zero()).then_some(ttl)
    }

    /// 该调用方是否可签发 / 列出任务句柄（`apps.task.*` 与 `taskId` 参数）：无会话主体本身，且 [`HubConfig::max_task_handles`] > 0。
    pub(crate) fn task_handles_for(&self, key: &CallerKey) -> bool {
        key.can_own_handles() && self.config.max_task_handles > 0
    }

    /// 路由用：调用方的 `apps.select` 优先（无会话调用方的选择过期即移除、未过期则续期），其次 [`Hub::select_instance`]。
    pub(crate) fn selected_for(&self, key: &CallerKey, app_id: &str) -> Option<String> {
        let ttl = self.selection_ttl(key);
        let now = tokio::time::Instant::now();
        lock(&self.agent_tasks)
            .get_mut(key)
            .and_then(|s| s.use_selection(app_id, ttl, now))
            .or_else(|| lock(&self.global_selected).get(app_id).cloned())
    }

    /// 列出用（`apps.list`）：全局选择被调用方未过期的选择覆盖；不续期。
    pub(crate) fn merged_selection(&self, key: &CallerKey) -> HashMap<String, String> {
        let mut out = lock(&self.global_selected).clone();
        let now = tokio::time::Instant::now();
        if let Some(s) = lock(&self.agent_tasks).get(key) {
            out.extend(s.live_selections(self.selection_ttl(key), now).map(|(a, i, _)| (a.to_owned(), i.to_owned())));
        }
        out
    }

    pub(crate) fn select_for_caller(&self, key: &CallerKey, app_id: &str, instance_id: &str) {
        lock(&self.agent_tasks).entry(key).select(app_id, instance_id, tokio::time::Instant::now());
    }

    /// 记下调用方已看过某 App 的总览（`apps.overview`）。无会话调用方不记：其总览不按"首次附带"送达（S5）。
    pub(crate) fn mark_delivered(&self, key: &CallerKey, app_id: &str, version: &str) {
        if key.is_stateless() {
            return;
        }
        lock(&self.agent_tasks)
            .entry(key)
            .delivered
            .insert(app_id.to_owned(), version.to_owned());
    }

    /// 该调用方首次接触某 App（或其总览版本变化）时返回总览，并记为已附带。
    ///
    /// 无会话调用方恒为 `None`：没有会话可去重（按主体去重会让同一主体下的新对话永远拿不到总览），总览改经
    /// `server/discover` 的 `instructions`、`apps.tools` 与 `apps.overview` 送达（docs/plans/12-mcp-stateless.md 3.2 H7）。
    pub(crate) fn attach_overview(&self, key: &CallerKey, app_id: &str) -> Option<Overview> {
        if key.is_stateless() {
            return None;
        }
        let ov = self.overview(app_id)?;
        let mut st = lock(&self.agent_tasks);
        let s = st.entry(key);
        if s.delivered.get(app_id) == Some(&ov.version) {
            return None;
        }
        s.delivered.insert(app_id.to_owned(), ov.version.clone());
        Some(ov)
    }

}
