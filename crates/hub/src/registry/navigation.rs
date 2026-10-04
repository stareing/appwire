//! 页面目录与导航（第 4c 项，spec/hub-api.md 3.14），以及按工具名查找定义与 App 级唤醒计划。

use std::sync::Arc;

use app_mcp_protocol::Visibility;

use crate::connection::Connection;
use crate::pages::{self, PageEntry};
use crate::tool_def::SharedTool;

use super::Registry;
use super::records::WakePlan;

impl Registry {
    // ------------------------------------------------------------------
    // 页面目录与导航（第 4c 项，spec/hub-api.md 3.14）
    // ------------------------------------------------------------------

    /// 记下 SDK 上报的页面工具。
    pub(super) fn learn_pages<'a>(&mut self, app_id: &str, tools: impl IntoIterator<Item = &'a SharedTool>) {
        if let Some(entry) = self.apps.get_mut(app_id) {
            entry.learned_pages.learn(app_id, tools);
        }
    }

    /// 某 App 的页面目录（清单 + 运行时上报），按页面名排序；App 未知时为空。
    pub fn pages(&self, app_id: &str) -> Vec<PageEntry> {
        self.apps
            .get(app_id)
            .map(|e| pages::catalog(e.manifest.as_ref(), &e.learned_pages))
            .unwrap_or_default()
    }

    /// 是否有已连接实例注册了该工具（即工具在某个实例的当前界面上）。
    pub fn tool_registered(&self, app_id: &str, tool: &str) -> bool {
        self.apps.get(app_id).is_some_and(|e| e.instances.iter().any(|i| i.tools.contains_key(tool)))
    }

    /// 工具是否注册在可见或有焦点的已连接实例上（"当前界面"，spec/hub-api.md 3.18 排序加成）。
    pub fn tool_on_current_surface(&self, app_id: &str, tool: &str) -> bool {
        self.apps.get(app_id).is_some_and(|e| {
            e.instances
                .iter()
                .any(|i| i.tools.contains_key(tool) && (i.focused || i.visibility == Some(Visibility::Visible)))
        })
    }

    /// 指定实例是否注册了该工具。
    pub fn instance_has_tool(&self, app_id: &str, instance_id: &str, tool: &str) -> bool {
        self.instance(app_id, instance_id).is_some_and(|i| i.tools.contains_key(tool))
    }

    /// 所有已知 appId（静态清单、已连接与休眠实例）。
    pub fn app_ids(&self) -> Vec<String> {
        self.apps.keys().cloned().collect()
    }

    /// 是否有已连接实例。
    pub fn has_connected(&self, app_id: &str) -> bool {
        self.apps.get(app_id).is_some_and(|e| !e.instances.is_empty())
    }

    /// App 的已连接实例中按路由优先级（`prefer` 优先，其次焦点 / 最近活跃）排第一的已就绪实例。
    pub fn preferred_instance(&self, app_id: &str, prefer: Option<&str>) -> Option<(String, Arc<Connection>)> {
        let entry = self.apps.get(app_id)?;
        entry.ordered(prefer, |i| i.ready).first().map(|i| (i.instance_id.clone(), i.conn.clone()))
    }

    /// App 各已连接实例的连接 ID（`apps.release` 据此收回租约）。
    pub fn connection_ids(&self, app_id: &str) -> std::collections::HashSet<u64> {
        self.apps.get(app_id).map(|e| e.instances.iter().map(|i| i.conn.id).collect()).unwrap_or_default()
    }

    /// 导航目标：已就绪且声明了导航能力的实例，按路由优先级（`prefer` 优先，其次焦点 / 最近活跃）。
    pub fn navigation_target(&self, app_id: &str, prefer: Option<&str>) -> Option<(String, Arc<Connection>)> {
        let entry = self.apps.get(app_id)?;
        entry
            .ordered(prefer, |i| i.navigate && i.ready && i.visibility != Some(Visibility::Frozen))
            .first()
            .map(|i| (i.instance_id.clone(), i.conn.clone()))
    }

    /// 导航目标（[`Registry::navigation_target`] 选出的实例；`strict` 时只能是 `prefer`）是否在前台：可见，或尚未上报可见性。
    /// 没有可导航的实例时为 `false`（spec/hub-api.md 3.14 后台替代）。
    pub fn navigation_target_in_foreground(&self, app_id: &str, prefer: Option<&str>, strict: bool) -> bool {
        let Some(entry) = self.apps.get(app_id) else { return false };
        entry
            .ordered(prefer, |i| i.navigate && i.ready && i.visibility != Some(Visibility::Frozen))
            .into_iter()
            .next()
            .filter(|i| !strict || Some(i.instance_id.as_str()) == prefer)
            .is_some_and(|i| i.visibility != Some(Visibility::Hidden))
    }

    /// App 的某个工具的已知定义：已连接实例注册的（按路由优先级）→ 休眠实例快照 → 清单。不含页面目录。
    pub fn app_tool(&self, app_id: &str, name: &str) -> Option<SharedTool> {
        let entry = self.apps.get(app_id)?;
        let registered = entry.ordered(None, |i| i.tools.contains_key(name)).into_iter().next().and_then(|i| i.tools.get(name));
        let dormant =
            || entry.dormant_ordered(None, |d| d.tools.contains_key(name)).into_iter().next().and_then(|d| d.tools.get(name));
        registered.or_else(dormant).or_else(|| entry.manifest.as_ref()?.tool(name)).cloned()
    }

    /// App 没有已连接实例时的唤醒计划（不针对具体工具）：最近活跃（或选定）的休眠实例，否则按清单冷启动。
    /// 已有连接时为 `None`。
    pub fn wake_plan_app(&self, app_id: &str, selected: Option<&str>) -> Option<WakePlan> {
        let entry = self.apps.get(app_id)?;
        if !entry.instances.is_empty() {
            return None;
        }
        if let Some(d) = entry.dormant_ordered(selected, |_| true).first() {
            return Some(self.plan_for(app_id, d, None));
        }
        if entry.manifest.is_none() && entry.named.is_none() {
            return None;
        }
        Some(WakePlan { app_id: app_id.to_owned(), instance_id: None, descriptor: None, tool: None })
    }

}
