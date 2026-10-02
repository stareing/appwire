//! 注册表：appId → 静态清单 + 已连接实例（工具、资源、可见性、订阅）。
//!
//! 注册表是纯数据结构（不做 I/O），由 `HubShared` 用互斥锁保护；
//! 持锁期间不 `await`。

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app_mcp_manifest::Manifest;
use app_mcp_protocol::{ClientKind, ResourceInfo, ToolInfo, Visibility, is_valid_name};
use serde_json::{Value, json};

use crate::connection::Connection;
use crate::pages::LearnedPages;
use crate::routing::{self, Candidate};
use crate::tool_def::{SharedTool, StaticManifest, ToolDef};

mod dormant;
mod instance_state;
mod listing;
mod navigation;
mod records;
mod route_targets;
#[cfg(test)]
mod tests;

pub use records::{
    DormantInstance, Instance, ListedResource, ListedTool, NamedEntry, NewInstance, ResourceTarget, ToolTarget,
    WakePlan,
};
pub(crate) use records::WakeTargetPresence;

#[derive(Debug, Default)]
struct AppEntry {
    manifest: Option<StaticManifest>,
    /// 按连接顺序排列。
    instances: Vec<Instance>,
    /// 按休眠顺序排列。
    dormant: Vec<DormantInstance>,
    /// SDK 上报过的页面工具（页面目录的运行时部分，随 App 记录保留）。
    learned_pages: LearnedPages,
    /// 名字服务发现记录（spec/naming.md 第 5 节）：有记录时 Hub 可按名拨号。
    named: Option<NamedEntry>,
}

impl AppEntry {
    fn display_name(&self) -> Option<&str> {
        self.manifest
            .as_ref()
            .map(|m| m.meta().name.as_str())
            .or_else(|| self.instances.last().map(|i| i.app_name.as_str()))
            .or_else(|| self.dormant.last().map(|i| i.app_name.as_str()))
    }

    fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.manifest.is_none() && self.dormant.is_empty() && self.named.is_none()
    }

    /// 休眠实例按优先级排列：选定的优先，其次最近活跃。
    fn dormant_ordered(&self, selected: Option<&str>, filter: impl Fn(&DormantInstance) -> bool) -> Vec<&DormantInstance> {
        let mut v: Vec<&DormantInstance> = self.dormant.iter().filter(|d| filter(d)).collect();
        v.sort_by_key(|d| (Some(d.instance_id.as_str()) != selected, std::cmp::Reverse(d.recency)));
        v
    }

    /// 按默认路由优先级排列的实例（`filter` 为真的才参与）。
    fn ordered<'a>(
        &'a self,
        selected: Option<&str>,
        filter: impl Fn(&Instance) -> bool,
    ) -> Vec<&'a Instance> {
        let cands: Vec<Candidate<'_>> = self
            .instances
            .iter()
            .filter(|i| filter(i))
            .map(Instance::candidate)
            .collect();
        routing::order(&cands, selected)
            .into_iter()
            .filter_map(|id| self.instances.iter().find(|i| i.instance_id == id))
            .collect()
    }
}

pub use crate::types::Availability;

#[derive(Debug, Default)]
pub struct Registry {
    apps: BTreeMap<String, AppEntry>,
    seq: u64,
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn client_kind_str(k: ClientKind) -> &'static str {
    match k {
        ClientKind::Web => "web",
        ClientKind::Native => "native",
        ClientKind::Hybrid => "hybrid",
    }
}

fn visibility_str(v: Option<Visibility>) -> Value {
    match v {
        Some(Visibility::Visible) => json!("visible"),
        Some(Visibility::Hidden) => json!("hidden"),
        Some(Visibility::Frozen) => json!("frozen"),
        None => Value::Null,
    }
}

/// 过滤 SDK 发来（或从休眠记录文件读回）的非法工具条目，合法的转为共享的紧凑定义。
pub(crate) fn sanitize_tools(app_id: &str, tools: Vec<ToolInfo>) -> Vec<SharedTool> {
    tools
        .into_iter()
        .filter(|t| {
            let ok = is_valid_name(&t.name)
                && t.input_schema.get("type").and_then(Value::as_str) == Some("object");
            if !ok {
                tracing::warn!(app_id, tool = %t.name, "忽略非法的工具定义（名称不合法或 inputSchema.type 不是 object）");
            }
            ok
        })
        .map(|t| Arc::new(ToolDef::from_info(t)))
        .collect()
}

pub(crate) fn sanitize_resources(app_id: &str, resources: Vec<ResourceInfo>) -> Vec<ResourceInfo> {
    resources
        .into_iter()
        .filter(|r| {
            let ok = is_valid_name(&r.name);
            if !ok {
                tracing::warn!(app_id, resource = %r.name, "忽略名称不合法的资源");
            }
            ok
        })
        .collect()
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记静态清单（工具转为紧凑定义，清单只保存这一份）；同一 appId 已有清单时替换，返回是否替换了旧清单。
    pub fn set_manifest(&mut self, manifest: Manifest) -> bool {
        self.set_static_manifest(StaticManifest::new(manifest))
    }

    /// 登记已转换的静态清单；同一 appId 已有清单时替换，返回是否替换了旧清单。
    pub fn set_static_manifest(&mut self, manifest: StaticManifest) -> bool {
        let entry = self.apps.entry(manifest.meta().app_id.clone()).or_default();
        entry.manifest.replace(manifest).is_some()
    }

    /// 移除静态清单（卸载事件，spec/naming.md 5.4）。返回是否有清单被移除。
    pub fn clear_manifest(&mut self, app_id: &str) -> bool {
        let Some(entry) = self.apps.get_mut(app_id) else { return false };
        let removed = entry.manifest.take().is_some();
        if entry.is_empty() {
            self.apps.remove(app_id);
        }
        removed
    }

    pub fn manifest(&self, app_id: &str) -> Option<&StaticManifest> {
        self.apps.get(app_id)?.manifest.as_ref()
    }

    /// 名字服务发现记录。
    pub fn named(&self, app_id: &str) -> Option<&NamedEntry> {
        self.apps.get(app_id)?.named.as_ref()
    }

    /// 记下（`Some`）或移除（`None`）名字服务发现记录；首次见到时间沿用旧记录。返回列表是否因此变化。
    pub fn set_named(&mut self, app_id: &str, named: Option<NamedEntry>) -> bool {
        match named {
            Some(mut n) => {
                let entry = self.apps.entry(app_id.to_owned()).or_default();
                if let Some(old) = &entry.named {
                    n.first_seen = old.first_seen;
                }
                let changed = entry.named.as_ref().is_none_or(|o| (o.activatable, o.running) != (n.activatable, n.running));
                entry.named = Some(n);
                changed
            }
            None => {
                let Some(entry) = self.apps.get_mut(app_id) else { return false };
                let changed = entry.named.take().is_some();
                if entry.is_empty() {
                    self.apps.remove(app_id);
                }
                changed
            }
        }
    }

    /// 名字的运行状态变化（`NameOwnerChanged`）。名字消失不删除记录（spec/naming.md 5.4）；没有记录时按需新建
    /// （运行中的、没有激活文件的 App）。返回列表是否因此变化。
    pub fn set_named_running(&mut self, app_id: &str, running: bool, make: impl FnOnce() -> NamedEntry) -> bool {
        let entry = self.apps.entry(app_id.to_owned()).or_default();
        match &mut entry.named {
            Some(n) => {
                let changed = n.running != running;
                n.running = running;
                if running {
                    n.last_seen = SystemTime::now();
                }
                changed
            }
            None if running => {
                entry.named = Some(make());
                true
            }
            None => {
                if entry.is_empty() {
                    self.apps.remove(app_id);
                }
                false
            }
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// 登记新实例。同一 instanceId 已存在时替换，返回被替换的旧连接（调用方负责关闭）。
    pub fn add_instance(&mut self, app_id: &str, new: NewInstance) -> Option<Arc<Connection>> {
        let seq = self.next_seq();
        let entry = self.apps.entry(app_id.to_owned()).or_default();
        let old = entry
            .instances
            .iter()
            .position(|i| i.instance_id == new.instance_id)
            .map(|pos| entry.instances.remove(pos).conn);
        entry.instances.push(Instance {
            instance_id: new.instance_id,
            app_name: new.app_name,
            client_kind: new.client_kind,
            app_version: new.app_version,
            title: new.title,
            url: new.url,
            overview: new.overview,
            pid: new.pid,
            navigate: new.navigate,
            conn: new.conn,
            wake: new.wake,
            visibility: None,
            focused: false,
            ready: false,
            connected_at: SystemTime::now(),
            connected_seq: seq,
            last_active_at: None,
            last_active_seq: None,
            tools: BTreeMap::new(),
            resources: BTreeMap::new(),
            subscriptions: HashSet::new(),
        });
        old
    }

    /// 移除某连接对应的实例。连接已被替换时不做任何事，返回 `None`。
    pub fn remove_instance(&mut self, app_id: &str, conn_id: u64) -> Option<Instance> {
        let entry = self.apps.get_mut(app_id)?;
        let pos = entry.instances.iter().position(|i| i.conn.id == conn_id)?;
        let inst = entry.instances.remove(pos);
        if entry.is_empty() {
            self.apps.remove(app_id);
        }
        Some(inst)
    }

}
