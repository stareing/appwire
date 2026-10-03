//! 渐进暴露（spec/hub-api.md 3.7）：按调用方决定直接列出哪些 App 的工具。

use std::collections::HashSet;
use std::sync::Arc;

#[cfg(feature = "mcp-server")]
use rmcp::model::Tool;

use crate::call;
use crate::task::CallerKey;
use crate::types::{HubTool, ToolExposure};

use super::{HubShared, lock};

impl HubShared {
    // ------------------------------------------------------------------
    // 渐进暴露（spec/hub-api.md 3.7）
    // ------------------------------------------------------------------

    /// App 与上游工具（不含内置工具）的总数。
    ///
    /// 只数不复制（第 4f 项 d：每次 `tools/list` 都会调用，之前经 [`Registry::tools`] 深拷贝全部定义）。
    fn tool_count(&self) -> usize {
        let policy = self.policy();
        if !policy.has_hide() {
            let apps = self.registry().tool_count();
            return apps + lock(&self.upstreams).values().map(|s| s.tools.len()).sum::<usize>();
        }
        let mut n = 0;
        self.registry().visit_tools(
            |_| true,
            |app_id, t, _| n += usize::from(policy.tool_hidden(app_id, &t.name, Some(&t.effective_annotations())).is_none()),
        );
        for (name, st) in lock(&self.upstreams).iter() {
            n += st
                .tools
                .iter()
                .filter(|t| policy.tool_hidden(name, &t.name, Some(&call::upstream_annotations(t))).is_none())
                .count();
        }
        n
    }

    /// 按 `mode` 是否列为渐进暴露。
    fn exposure_active(&self, mode: ToolExposure) -> bool {
        match mode {
            ToolExposure::All => false,
            ToolExposure::Progressive => true,
            ToolExposure::Auto => self.tool_count() > self.config.tool_exposure_threshold,
        }
    }

    /// legacy 会话与 Hub API 当前是否按渐进暴露列出工具（[`HubConfig::tool_exposure`]）。
    pub(crate) fn progressive(&self) -> bool {
        self.exposure_active(self.config.tool_exposure)
    }

    /// 无会话请求当前是否按渐进暴露列出工具（[`HubConfig::stateless_tool_exposure`]）。
    pub(crate) fn stateless_progressive(&self) -> bool {
        self.exposure_active(self.config.stateless_tool_exposure)
    }

    /// 该调用方当前是否按渐进暴露列出工具。
    pub(crate) fn progressive_for(&self, key: &CallerKey) -> bool {
        if key.is_stateless() { self.stateless_progressive() } else { self.progressive() }
    }

    /// 调用方直接列出工具的 App；渐进暴露未生效时返回 `None`（全部列出）。
    ///
    /// - legacy 会话与 Hub API：展开过 / 调用过的，以及选定了实例的（调用方 `apps.select` 与全局选择）。
    /// - 无会话调用方：只有全局选择（[`Hub::select_instance`]，服务器状态）。
    ///
    /// @invariant 无会话调用方的结果只取决于服务器状态与配置，不读其任务（展开记录、`apps.select`）：MCP 2026-07-28 要求列表
    /// 不随其他请求的副作用变化（SEP-2567，docs/plans/12-mcp-stateless.md 3.3）。
    pub(crate) fn exposed_apps(&self, key: &CallerKey) -> Option<HashSet<String>> {
        if !self.progressive_for(key) {
            return None;
        }
        if key.is_stateless() {
            return Some(lock(&self.global_selected).keys().cloned().collect());
        }
        let mut out: HashSet<String> = self.merged_selection(key).into_keys().collect();
        if let Some(s) = lock(&self.agent_tasks).get(key) {
            out.extend(s.exposed.iter().cloned());
        }
        Some(out)
    }

    /// 把 App 记为调用方已展开。渐进暴露生效且此前未列出时返回 `true`（调用方的工具列表因此变化）。
    /// 无会话调用方不记录（其列表不随调用变化），恒为 `false`。
    pub(crate) fn expose_app(&self, key: &CallerKey, app_id: &str) -> bool {
        if key.is_stateless() {
            return false;
        }
        let selected = self.merged_selection(key).contains_key(app_id);
        let inserted = lock(&self.agent_tasks)
            .entry(key)
            .exposed
            .insert(app_id.to_owned());
        inserted && !selected && self.progressive()
    }

    /// 只通知一个 MCP 会话工具列表已变化（渐进暴露下该会话展开了新的 App）。
    pub(crate) fn notify_session_tools_changed(self: &Arc<Self>, session: u64) {
        let Some(peer) = lock(&self.subscribers).session(session) else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let shared = self.clone();
        rt.spawn(async move {
            if peer.notify_tool_list_changed().await.is_err() {
                tracing::debug!(session, "MCP 会话已关闭，移除");
                shared.remove_subscriber(session);
            }
        });
    }

    /// MCP `tools/list`：内置工具 + （渐进暴露时只含已展开 App 的）App 工具 + 上游工具。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn mcp_tools(&self, key: &CallerKey) -> Vec<Tool> {
        let exposed = self.exposed_apps(key);
        let listed = |app_id: &str| exposed.as_ref().is_none_or(|e| e.contains(app_id));
        let policy = self.policy();
        let mut tools = call::builtin_tools(call::BuiltinSet {
            apps_tools: exposed.is_some(),
            apps_page: self.has_pages(),
            tasks: self.task_handles_for(key),
            locks: self.locks_enabled(),
        });
        self.registry().visit_tools(listed, |app_id, t, availability| {
            if policy.tool_hidden(app_id, &t.name, Some(&t.effective_annotations())).is_none() {
                tools.push(call::to_mcp_tool(app_id, t, availability));
            }
        });
        let ups = lock(&self.upstreams);
        for (name, st) in ups.iter().filter(|(name, _)| listed(name)) {
            for t in st.tools.iter().filter(|t| policy.tool_hidden(name, &t.name, Some(&call::upstream_annotations(t))).is_none()) {
                let mut t = t.clone();
                t.name = format!("{name}.{}", t.name).into();
                tools.push(t);
            }
        }
        tools
    }

    /// `apps.tools` 的结果：某个 App（或上游）的全部工具定义。
    pub(crate) fn app_tools(&self, app_id: &str) -> Vec<HubTool> {
        self.visible_tools(false, |a| a == app_id)
            .into_iter()
            .filter(|(_, builtin)| !builtin)
            .map(|(t, _)| t)
            .collect()
    }

}
