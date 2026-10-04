//! 已连接实例的状态同步：工具 / 资源同步与增量变化、可见性、就绪、最近使用，以及 App 未连接时的错误。

use std::collections::BTreeMap;
use std::time::SystemTime;

use app_mcp_protocol::{ErrorKind, ResourceInfo, ToolError, ToolInfo, Visibility};
use serde_json::json;

use crate::tool_def::SharedTool;

use super::{AppEntry, Registry, sanitize_resources, sanitize_tools};
use super::records::Instance;

impl Registry {
    pub(super) fn instance_mut(&mut self, app_id: &str, conn_id: u64) -> Option<&mut Instance> {
        self.apps
            .get_mut(app_id)?
            .instances
            .iter_mut()
            .find(|i| i.conn.id == conn_id)
    }

    /// 某连接对应实例最近上报的可见性。
    pub fn visibility_of(&self, app_id: &str, conn_id: u64) -> Option<Visibility> {
        self.apps
            .get(app_id)?
            .instances
            .iter()
            .find(|i| i.conn.id == conn_id)?
            .visibility
    }

    pub fn instance(&self, app_id: &str, instance_id: &str) -> Option<&Instance> {
        self.apps
            .get(app_id)?
            .instances
            .iter()
            .find(|i| i.instance_id == instance_id)
    }

    pub fn has_app(&self, app_id: &str) -> bool {
        self.apps.contains_key(app_id)
    }

    /// 全量替换工具列表。返回列表是否变化。
    pub fn sync_tools(&mut self, app_id: &str, conn_id: u64, tools: Vec<ToolInfo>) -> bool {
        let tools = sanitize_tools(app_id, tools);
        self.sync_sanitized_tools(app_id, conn_id, tools)
    }

    /// [`Self::sync_tools`] 的后半段：`tools` 已经过 [`sanitize_tools`]。同步后不再保留本实例此前的声明。
    pub(crate) fn sync_sanitized_tools(&mut self, app_id: &str, conn_id: u64, tools: Vec<SharedTool>) -> bool {
        if self.instance_mut(app_id, conn_id).is_some() {
            self.learn_pages(app_id, tools.iter());
        }
        let Some(inst) = self.instance_mut(app_id, conn_id) else {
            return false;
        };
        let new: BTreeMap<String, SharedTool> =
            tools.into_iter().map(|t| (t.name.clone(), t)).collect();
        let changed = inst.tools != new;
        inst.tools = new;
        inst.prior_tools = None;
        changed
    }

    /// 增量变更工具列表。返回列表是否变化。
    pub fn change_tools(
        &mut self,
        app_id: &str,
        conn_id: u64,
        upserted: Vec<ToolInfo>,
        removed: Vec<String>,
    ) -> bool {
        self.change_sanitized_tools(app_id, conn_id, sanitize_tools(app_id, upserted), removed)
    }

    /// [`Self::change_tools`] 的后半段：`upserted` 已经过 [`sanitize_tools`]。
    pub(crate) fn change_sanitized_tools(
        &mut self,
        app_id: &str,
        conn_id: u64,
        upserted: Vec<SharedTool>,
        removed: Vec<String>,
    ) -> bool {
        if self.instance_mut(app_id, conn_id).is_some() {
            self.learn_pages(app_id, upserted.iter());
        }
        let Some(inst) = self.instance_mut(app_id, conn_id) else {
            return false;
        };
        let mut changed = false;
        for name in removed {
            changed |= inst.tools.remove(&name).is_some();
        }
        for t in upserted {
            changed |= inst.tools.get(&t.name) != Some(&t);
            inst.tools.insert(t.name.clone(), t);
        }
        changed
    }

    /// 全量替换资源列表。返回列表是否变化。
    pub fn sync_resources(
        &mut self,
        app_id: &str,
        conn_id: u64,
        resources: Vec<ResourceInfo>,
    ) -> bool {
        let resources = sanitize_resources(app_id, resources);
        self.sync_sanitized_resources(app_id, conn_id, resources)
    }

    /// [`Self::sync_resources`] 的后半段：`resources` 已经过 [`sanitize_resources`]。同步后不再保留本实例此前的声明。
    pub(crate) fn sync_sanitized_resources(
        &mut self,
        app_id: &str,
        conn_id: u64,
        resources: Vec<ResourceInfo>,
    ) -> bool {
        let Some(inst) = self.instance_mut(app_id, conn_id) else {
            return false;
        };
        let new: BTreeMap<String, ResourceInfo> =
            resources.into_iter().map(|r| (r.name.clone(), r)).collect();
        let changed = inst.resources != new;
        inst.resources = new;
        inst.prior_resources = None;
        let resources = &inst.resources;
        inst.subscriptions.retain(|n| resources.contains_key(n));
        changed
    }

    /// 增量变更资源列表。返回列表是否变化。
    pub fn change_resources(
        &mut self,
        app_id: &str,
        conn_id: u64,
        upserted: Vec<ResourceInfo>,
        removed: Vec<String>,
    ) -> bool {
        let upserted = sanitize_resources(app_id, upserted);
        let Some(inst) = self.instance_mut(app_id, conn_id) else {
            return false;
        };
        let mut changed = false;
        for name in removed {
            changed |= inst.resources.remove(&name).is_some();
            inst.subscriptions.remove(&name);
        }
        for r in upserted {
            changed |= inst.resources.get(&r.name) != Some(&r);
            inst.resources.insert(r.name.clone(), r);
        }
        changed
    }

    /// 更新可见性。变为 visible 或获得焦点时记为一次“活跃”。
    pub fn set_visibility(
        &mut self,
        app_id: &str,
        conn_id: u64,
        visibility: Visibility,
        focused: bool,
    ) {
        let seq = self.next_seq();
        let Some(inst) = self.instance_mut(app_id, conn_id) else {
            return;
        };
        let became_visible =
            visibility == Visibility::Visible && inst.visibility != Some(Visibility::Visible);
        let became_focused = focused && !inst.focused;
        inst.visibility = Some(visibility);
        inst.focused = focused;
        if became_visible || became_focused {
            inst.last_active_seq = Some(seq);
            inst.last_active_at = Some(SystemTime::now());
        }
    }

    /// 标记实例就绪。返回该实例是否要持久化（声明了唤醒描述）。
    pub fn set_ready(&mut self, app_id: &str, conn_id: u64) -> bool {
        self.instance_mut(app_id, conn_id).is_some_and(|inst| {
            inst.ready = true;
            inst.wake.is_some()
        })
    }

    /// 记一次活跃（完成一次调用后）。
    pub fn touch(&mut self, app_id: &str, instance_id: &str) {
        let seq = self.next_seq();
        if let Some(inst) = self.apps.get_mut(app_id).and_then(|e| {
            e.instances
                .iter_mut()
                .find(|i| i.instance_id == instance_id)
        }) {
            inst.last_active_seq = Some(seq);
            inst.last_active_at = Some(SystemTime::now());
        }
    }

    pub(super) fn app_label(&self, app_id: &str) -> String {
        match self.apps.get(app_id).and_then(AppEntry::display_name) {
            Some(name) if name != app_id => format!("「{name}」（{app_id}）"),
            _ => format!("「{app_id}」"),
        }
    }

    pub(crate) fn disconnected_error(&self, app_id: &str) -> ToolError {
        let url = self.manifest(app_id).and_then(|m| m.meta().web_url());
        let label = self.app_label(app_id);
        let message = match url {
            Some(url) => {
                format!("App{label}当前未连接。请让用户在浏览器中打开 {url}，待 App 连接后重试。")
            }
            None => format!("App{label}当前未连接。请让用户先启动该 App，待其连接后重试。"),
        };
        let mut details = json!({ "appId": app_id });
        if let Some(url) = url {
            details["launchUrl"] = json!(url);
        }
        ToolError::new(ErrorKind::AppDisconnected, message).with_details(details)
    }

}
