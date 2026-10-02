//! 列表：工具、资源、订阅状态、App 信息、总览与 `apps.list` 的 JSON。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::connection::Connection;
use crate::overview::{AppSummary, Overview, OverviewSource};
use crate::pages;
use crate::tool_def::SharedTool;
use crate::types::{AppInfo, AppKind, InstanceInfo};

use super::{AppEntry, Availability, Registry, client_kind_str, unix_ms, visibility_str};
use super::records::{ListedResource, ListedTool};

impl Registry {
    /// MCP 工具列表（不含内置工具）。
    ///
    /// 已连接的 App：所有实例工具的并集（同名工具取默认路由优先级最高的实例的定义），
    /// 再加上未注册的静态工具（标记为 [`Availability::NotRegistered`]）。
    /// 未连接的 App：静态工具（[`Availability::Disconnected`]）。
    pub fn tools(&self) -> Vec<ListedTool> {
        self.tools_of(|_| true)
    }

    /// [`Self::tools`] 中 `app` 为真的 App 的部分（先按 App 过滤再取定义，定义经 `Arc` 共享，不深拷贝）。
    pub fn tools_of(&self, app: impl Fn(&str) -> bool) -> Vec<ListedTool> {
        let mut out = Vec::new();
        self.visit_tools(app, |app_id, info, availability| {
            out.push(ListedTool { app_id: app_id.to_owned(), info: info.clone(), availability });
        });
        out
    }

    /// [`Self::tools`] 的条数（不复制任何定义）。
    pub fn tool_count(&self) -> usize {
        let mut n = 0;
        self.visit_tools(|_| true, |_, _, _| n += 1);
        n
    }

    /// 按 [`Self::tools`] 的规则逐个访问工具（`app` 为真的 App）。
    pub fn visit_tools(&self, app: impl Fn(&str) -> bool, mut f: impl FnMut(&str, &SharedTool, Availability)) {
        for (app_id, entry) in self.apps.iter().filter(|(id, _)| app(id)) {
            let mut seen: HashSet<&str> = HashSet::new();
            if !entry.instances.is_empty() {
                // `view` 工具只列首选实例（焦点 / 最近活跃）当前界面上的（spec/hub-api.md 3.14 L1）。
                for (rank, inst) in entry.ordered(None, |_| true).into_iter().enumerate() {
                    for (name, info) in &inst.tools {
                        if (rank == 0 || info.surface.is_app()) && seen.insert(name) {
                            f(app_id, info, Availability::Available);
                        }
                    }
                }
            }
            for d in entry.dormant_ordered(None, |_| true) {
                for (name, info) in d.tools.iter().filter(|(_, t)| t.surface.is_app()) {
                    if seen.insert(name) {
                        f(app_id, info, Availability::Dormant);
                    }
                }
            }
            if let Some(m) = &entry.manifest {
                let availability = if entry.instances.is_empty() {
                    Availability::Disconnected
                } else {
                    Availability::NotRegistered
                };
                for t in m.tools().iter().filter(|t| !seen.contains(t.name.as_str())) {
                    f(app_id, t, availability);
                }
            }
        }
    }

    /// MCP 资源列表：已连接 App 的资源并集；未连接 App 的静态资源（`available = false`）。
    pub fn resources(&self) -> Vec<ListedResource> {
        let mut out = Vec::new();
        for (app_id, entry) in &self.apps {
            let mut seen: HashSet<&str> = HashSet::new();
            for inst in entry.ordered(None, |_| true) {
                for (name, info) in &inst.resources {
                    if seen.insert(name) {
                        out.push(ListedResource {
                            app_id: app_id.clone(),
                            info: info.clone(),
                            available: true,
                        });
                    }
                }
            }
            // 休眠实例的资源：读取时按需唤醒，视为可用。
            for d in entry.dormant_ordered(None, |_| true) {
                for (name, info) in &d.resources {
                    if seen.insert(name) {
                        out.push(ListedResource {
                            app_id: app_id.clone(),
                            info: info.clone(),
                            available: true,
                        });
                    }
                }
            }
            if entry.instances.is_empty()
                && let Some(m) = &entry.manifest
            {
                for r in m.meta().resources.iter().filter(|r| !seen.contains(r.name.as_str())) {
                    out.push(ListedResource {
                        app_id: app_id.clone(),
                        info: r.clone(),
                        available: false,
                    });
                }
            }
        }
        out
    }

    /// 提供资源 `name` 的所有实例：`(连接, 是否已订阅)`。
    pub fn resource_holders(&self, app_id: &str, name: &str) -> Vec<(Arc<Connection>, bool)> {
        self.apps
            .get(app_id)
            .map(|e| {
                e.instances
                    .iter()
                    .filter(|i| i.resources.contains_key(name))
                    .map(|i| (i.conn.clone(), i.subscriptions.contains(name)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 记录 Host 对某实例的订阅状态。
    pub fn mark_subscribed(&mut self, app_id: &str, conn_id: u64, name: &str, subscribed: bool) {
        if let Some(inst) = self.instance_mut(app_id, conn_id) {
            if subscribed {
                inst.subscriptions.insert(name.to_owned());
            } else {
                inst.subscriptions.remove(name);
            }
        }
    }

    /// App 的显示名（清单名优先，其次最近连接实例上报的名称）。
    pub fn display_name(&self, app_id: &str) -> Option<String> {
        self.apps
            .get(app_id)
            .and_then(AppEntry::display_name)
            .map(str::to_owned)
    }

    pub fn has_manifest(&self, app_id: &str) -> bool {
        self.manifest(app_id).is_some()
    }

    /// 所有实例的连接（关闭 Hub 时使用）。
    pub fn all_connections(&self) -> Vec<Arc<Connection>> {
        self.apps
            .values()
            .flat_map(|e| e.instances.iter().map(|i| i.conn.clone()))
            .collect()
    }

    /// [`crate::Hub::apps`] 的 App 部分。`selected` 为 appId → instanceId 选择。
    pub fn app_infos(&self, selected: &HashMap<String, String>) -> Vec<AppInfo> {
        self.apps
            .iter()
            .map(|(app_id, entry)| AppInfo {
                app_id: app_id.clone(),
                name: entry.display_name().unwrap_or(app_id).to_owned(),
                kind: AppKind::App,
                summary: self.overview(app_id).map(|o| o.summary),
                connected: !entry.instances.is_empty(),
                instances: entry
                    .instances
                    .iter()
                    .map(|i| InstanceInfo {
                        instance_id: i.instance_id.clone(),
                        client_kind: client_kind_str(i.client_kind).to_owned(),
                        visibility: i.visibility.unwrap_or_default(),
                        focused: i.focused,
                        last_active_ms: unix_ms(i.last_active_at.unwrap_or(i.connected_at)),
                        title: i.title.clone(),
                        pid: i.pid,
                        connection_id: Some(i.conn.cid.clone()),
                    })
                    .collect(),
                selected_instance: selected
                    .get(app_id)
                    .filter(|id| entry.instances.iter().any(|i| &i.instance_id == *id))
                    .cloned(),
                dormant_instances: entry
                    .dormant
                    .iter()
                    .map(|d| InstanceInfo {
                        instance_id: d.instance_id.clone(),
                        client_kind: client_kind_str(d.client_kind).to_owned(),
                        visibility: d.visibility.unwrap_or_default(),
                        focused: false,
                        last_active_ms: unix_ms(d.last_active_at.unwrap_or(d.connected_at)),
                        title: d.title.clone(),
                        pid: None,
                        connection_id: None,
                    })
                    .collect(),
            })
            .collect()
    }

    /// App 当前生效的总览：运行时（按默认路由优先级第一个带总览的实例）优先，其次静态清单。
    pub fn overview(&self, app_id: &str) -> Option<Overview> {
        let entry = self.apps.get(app_id)?;
        let name = entry.display_name().unwrap_or(app_id);
        let runtime = entry
            .ordered(None, |i| i.overview.is_some())
            .into_iter()
            .find_map(|i| {
                Overview::new(app_id, name, i.overview.as_ref()?, OverviewSource::Runtime)
            });
        let dormant = || {
            entry
                .dormant_ordered(None, |d| d.overview.is_some())
                .into_iter()
                .find_map(|d| Overview::new(app_id, name, d.overview.as_ref()?, OverviewSource::Runtime))
        };
        runtime.or_else(dormant).or_else(|| {
            let m = entry.manifest.as_ref()?.meta();
            Overview::new(app_id, name, m.overview.as_ref()?, OverviewSource::Manifest)
        })
    }

    /// 所有已知 App 的一句话简介（用于 MCP `instructions`）。
    pub fn summaries(&self) -> Vec<AppSummary> {
        self.apps
            .iter()
            .map(|(app_id, entry)| AppSummary {
                app_id: app_id.clone(),
                name: entry.display_name().unwrap_or(app_id).to_owned(),
                summary: self.overview(app_id).map(|o| o.summary),
            })
            .collect()
    }

    /// `apps.list` 的结果。`selected` 为当前会话的 appId → instanceId 选择。
    pub fn apps_json(&self, selected: &HashMap<String, String>) -> Value {
        let apps: Vec<Value> = self
            .apps
            .iter()
            .map(|(app_id, entry)| {
                let sel = selected
                    .get(app_id)
                    .filter(|id| entry.instances.iter().any(|i| &i.instance_id == *id))
                    .cloned();
                let default_target = entry
                    .ordered(sel.as_deref(), |_| true)
                    .first()
                    .map(|i| i.instance_id.clone());
                let instances: Vec<Value> = entry
                    .instances
                    .iter()
                    .map(|i| {
                        json!({
                            "instanceId": i.instance_id,
                            "clientKind": i.client_kind,
                            "title": i.title,
                            "url": i.url,
                            "appVersion": i.app_version,
                            "visibility": visibility_str(i.visibility),
                            "focused": i.focused,
                            "ready": i.ready,
                            "connectedAt": unix_ms(i.connected_at),
                            "lastActiveAt": i.last_active_at.map(unix_ms),
                            "tools": i.tools.keys().collect::<Vec<_>>(),
                            "resources": i.resources.keys().collect::<Vec<_>>(),
                            "navigation": i.navigate,
                        })
                    })
                    .collect();
                let dormant: Vec<Value> = entry
                    .dormant
                    .iter()
                    .map(|d| {
                        json!({
                            "instanceId": d.instance_id,
                            "clientKind": d.client_kind,
                            "title": d.title,
                            "url": d.url,
                            "appVersion": d.app_version,
                            "sleptAt": unix_ms(d.slept_at),
                            "lastActiveAt": d.last_active_at.map(unix_ms),
                            "wake": d.wake_descriptor().map(|w| crate::wake::kind_str(w.kind)),
                            "tools": d.tools.keys().collect::<Vec<_>>(),
                            "resources": d.resources.keys().collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                let m = entry.manifest.as_ref();
                json!({
                    "appId": app_id,
                    "kind": "app",
                    "name": entry.display_name().unwrap_or(app_id),
                    "description": m.and_then(|m| m.meta().description.clone()),
                    "summary": self.overview(app_id).map(|o| o.summary),
                    "connected": !entry.instances.is_empty(),
                    "dormant": entry.instances.is_empty() && !entry.dormant.is_empty(),
                    "instances": instances,
                    "dormantInstances": dormant,
                    "selectedInstanceId": sel,
                    "defaultInstanceId": default_target,
                    "staticToolCount": m.map(|m| m.tools().len()).unwrap_or(0),
                    "pageCount": pages::catalog(m, &entry.learned_pages).len(),
                    "launchUrl": m.and_then(|m| m.meta().web_url()),
                    "nameService": entry.named.as_ref().map(|n| json!({
                        "source": n.source,
                        "name": n.detail,
                        "activatable": n.activatable,
                        "running": n.running,
                        "firstSeenAt": unix_ms(n.first_seen),
                        "lastSeenAt": unix_ms(n.last_seen),
                    })),
                })
            })
            .collect();
        json!({ "apps": apps })
    }
}
