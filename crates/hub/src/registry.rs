//! 注册表：appId → 静态清单 + 已连接实例（工具、资源、可见性、订阅）。
//!
//! 注册表是纯数据结构（不做 I/O），由 `HubShared` 用互斥锁保护；
//! 持锁期间不 `await`。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app_mcp_manifest::Manifest;
use app_mcp_protocol::{
    AppOverview, ClientKind, ErrorKind, ResourceInfo, ResourcesSyncParams, ToolError, ToolInfo,
    ToolsSyncParams, Visibility, WakeDescriptor, WakeKind, is_valid_name,
};
use serde_json::{Value, json};

use crate::connection::Connection;
use crate::overview::{AppSummary, Overview, OverviewSource};
use crate::pages::{self, LearnedPages, PageEntry};
use crate::routing::{self, Candidate};
use crate::tool_def::{SharedTool, StaticManifest, ToolDef};
use crate::types::{AppInfo, AppKind, InstanceInfo};

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
    fn persisted(&self, now: SystemTime) -> Option<DormantInstance> {
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

    fn candidate(&self) -> Candidate<'_> {
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

    // ------------------------------------------------------------------
    // 休眠（spec/lifecycle.md §9）
    // ------------------------------------------------------------------

    /// 把某连接对应的实例转为休眠记录（同一 instanceId 的旧休眠记录被替换）。返回 instanceId。
    pub fn make_dormant(
        &mut self,
        app_id: &str,
        conn_id: u64,
        resume_token: String,
        tools_hash: String,
        wake: Option<WakeDescriptor>,
    ) -> Option<String> {
        let entry = self.apps.get_mut(app_id)?;
        let pos = entry.instances.iter().position(|i| i.conn.id == conn_id)?;
        let inst = entry.instances.remove(pos);
        entry.dormant.retain(|d| d.instance_id != inst.instance_id);
        let id = inst.instance_id.clone();
        entry.dormant.push(DormantInstance {
            instance_id: inst.instance_id,
            app_name: inst.app_name,
            client_kind: inst.client_kind,
            app_version: inst.app_version,
            title: inst.title,
            url: inst.url,
            overview: inst.overview,
            visibility: inst.visibility,
            tools: inst.tools,
            resources: inst.resources,
            resume_token,
            tools_hash,
            wake,
            slept_at: SystemTime::now(),
            connected_at: inst.connected_at,
            last_active_at: inst.last_active_at,
            recency: inst.last_active_seq.unwrap_or(inst.connected_seq),
        });
        Some(id)
    }

    /// Hub 关闭按名拨入的通道（或对端死亡）：实例转为休眠快照（spec/naming.md 第 3、7.5 节）。恢复令牌由 Hub 生成但
    /// 不下发（SDK 下次握手不带令牌，按完整同步），`toolsHash` 取快照摘要。返回 instanceId。
    pub fn make_dormant_by_hub(&mut self, app_id: &str, conn_id: u64, resume_token: String) -> Option<String> {
        let entry = self.apps.get(app_id)?;
        let inst = entry.instances.iter().find(|i| i.conn.id == conn_id)?;
        let wake = inst.wake.clone();
        let hash = app_mcp_protocol::tools_hash(
            &ToolsSyncParams { tools: inst.tools.values().map(|t| t.to_info()).collect() },
            &ResourcesSyncParams { resources: inst.resources.values().cloned().collect() },
        );
        self.make_dormant(app_id, conn_id, resume_token, hash, wake)
    }

    pub fn dormant(&self, app_id: &str, instance_id: &str) -> Option<&DormantInstance> {
        self.apps
            .get(app_id)?
            .dormant
            .iter()
            .find(|d| d.instance_id == instance_id)
    }

    /// 取出（移除）某个休眠记录。
    pub fn take_dormant(&mut self, app_id: &str, instance_id: &str) -> Option<DormantInstance> {
        let entry = self.apps.get_mut(app_id)?;
        let pos = entry.dormant.iter().position(|d| d.instance_id == instance_id)?;
        Some(entry.dormant.remove(pos))
    }

    /// 移除某 App 的全部休眠记录（App 以新实例 ID 连接时），返回被移除的 instanceId。
    pub fn clear_dormant(&mut self, app_id: &str) -> Vec<String> {
        let Some(entry) = self.apps.get_mut(app_id) else {
            return Vec::new();
        };
        entry.dormant.drain(..).map(|d| d.instance_id).collect()
    }

    /// 移除 `before` 之前休眠的记录，返回 `(appId, instanceId)`。
    pub fn expire_dormant(&mut self, before: SystemTime) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (app_id, entry) in self.apps.iter_mut() {
            entry.dormant.retain(|d| {
                let keep = d.slept_at >= before;
                if !keep {
                    out.push((app_id.clone(), d.instance_id.clone()));
                }
                keep
            });
        }
        self.apps.retain(|_, e| !e.is_empty());
        out
    }

    /// 某 App 要持久化的实例与页面目录中 SDK 上报过的页面工具（spec/hub-api.md 3.5「持久化」）：休眠记录，
    /// 加上已就绪且声明了唤醒描述的在线实例（Host 异常退出时它们来不及转为休眠）。工具定义共享，不深拷贝。
    pub(crate) fn dormant_view(&self, app_id: &str) -> (Vec<DormantInstance>, Vec<SharedTool>) {
        let Some(e) = self.apps.get(app_id) else {
            return (Vec::new(), Vec::new());
        };
        let now = SystemTime::now();
        let mut out = e.dormant.clone();
        out.extend(e.instances.iter().filter_map(|i| i.persisted(now)));
        if out.is_empty() {
            return (Vec::new(), Vec::new());
        }
        (out, e.learned_pages.tools().cloned().collect())
    }

    /// 登记从文件读回的休眠记录（Hub 启动时）：按读回顺序（旧 → 新）重新分配路由优先级，已有同 ID 的记录不覆盖；
    /// 页面工具记入页面目录。返回登记的实例数。
    pub(crate) fn restore_dormant(
        &mut self,
        app_id: &str,
        instances: Vec<DormantInstance>,
        page_tools: &[SharedTool],
    ) -> usize {
        let mut n = 0;
        for mut d in instances {
            d.recency = self.next_seq();
            let entry = self.apps.entry(app_id.to_owned()).or_default();
            if entry.dormant.iter().any(|x| x.instance_id == d.instance_id)
                || entry.instances.iter().any(|x| x.instance_id == d.instance_id)
            {
                continue;
            }
            entry.learned_pages.learn(app_id, d.tools.values());
            entry.dormant.push(d);
            n += 1;
        }
        if let Some(entry) = self.apps.get_mut(app_id) {
            entry.learned_pages.learn(app_id, page_tools);
        }
        n
    }

    /// 快速恢复：把休眠快照作为新连接实例的工具 / 资源（SDK 跳过了 `tools/sync`）。
    pub fn restore_snapshot(&mut self, app_id: &str, conn_id: u64, snapshot: &DormantInstance) {
        self.learn_pages(app_id, snapshot.tools.values());
        if let Some(inst) = self.instance_mut(app_id, conn_id) {
            inst.tools = snapshot.tools.clone();
            inst.resources = snapshot.resources.clone();
            if inst.visibility.is_none() {
                inst.visibility = snapshot.visibility;
            }
        }
    }

    /// 调用工具前是否需要先唤醒：没有已连接实例注册该工具，而休眠实例的快照中有
    /// （或 App 未运行、清单声明了该静态工具）时返回计划。
    ///
    /// `strict` 为真时 `selected` 是调用方严格指定的实例。
    pub fn wake_plan_tool(
        &self,
        app_id: &str,
        tool: &str,
        selected: Option<&str>,
        strict: bool,
    ) -> Option<WakePlan> {
        let entry = self.apps.get(app_id)?;
        if strict {
            let id = selected?;
            if entry.instances.iter().any(|i| i.instance_id == id) {
                return None;
            }
            let d = entry.dormant.iter().find(|d| d.instance_id == id)?;
            let t = d.tools.get(tool)?;
            return Some(self.plan_for(app_id, d, Some(t.clone())));
        }
        if entry.instances.iter().any(|i| i.tools.contains_key(tool)) {
            return None;
        }
        if let Some(d) = entry.dormant_ordered(selected, |d| d.tools.contains_key(tool)).first() {
            return Some(self.plan_for(app_id, d, d.tools.get(tool).cloned()));
        }
        // App 未运行：静态工具按清单冷启动。
        if entry.instances.is_empty() {
            let t = entry.manifest.as_ref()?.tool(tool)?;
            return Some(WakePlan {
                app_id: app_id.to_owned(),
                instance_id: None,
                descriptor: None,
                tool: Some(t.clone()),
            });
        }
        None
    }

    /// 读取资源前是否需要先唤醒（只考虑休眠实例的快照）。
    pub fn wake_plan_resource(&self, app_id: &str, name: &str, selected: Option<&str>) -> Option<WakePlan> {
        let entry = self.apps.get(app_id)?;
        if entry.instances.iter().any(|i| i.resources.contains_key(name)) {
            return None;
        }
        let d = *entry.dormant_ordered(selected, |d| d.resources.contains_key(name)).first()?;
        Some(self.plan_for(app_id, d, None))
    }

    /// 唤醒目标当前是否已有连接（spec/lifecycle.md §9 唤醒去重）。
    ///
    /// `instance_id = None`（冷启动）时该 App 的任一已连接实例都算。
    pub(crate) fn wake_target_presence(&self, app_id: &str, instance_id: Option<&str>) -> WakeTargetPresence {
        let Some(entry) = self.apps.get(app_id) else {
            return WakeTargetPresence::Absent;
        };
        let mut matching = entry
            .instances
            .iter()
            .filter(|i| instance_id.is_none_or(|id| i.instance_id == id))
            .peekable();
        if matching.peek().is_none() {
            return WakeTargetPresence::Absent;
        }
        match matching.find(|i| i.ready) {
            Some(i) => WakeTargetPresence::Ready(i.instance_id.clone()),
            None => WakeTargetPresence::Handshaking,
        }
    }

    /// 实例是否仍有记录（已连接或休眠）。
    pub(crate) fn instance_known(&self, app_id: &str, instance_id: &str) -> bool {
        self.apps.get(app_id).is_some_and(|e| {
            e.instances.iter().any(|i| i.instance_id == instance_id)
                || e.dormant.iter().any(|d| d.instance_id == instance_id)
        })
    }

    fn plan_for(&self, app_id: &str, d: &DormantInstance, tool: Option<SharedTool>) -> WakePlan {
        WakePlan {
            app_id: app_id.to_owned(),
            instance_id: Some(d.instance_id.clone()),
            descriptor: d.wake_descriptor().cloned(),
            tool,
        }
    }

    fn instance_mut(&mut self, app_id: &str, conn_id: u64) -> Option<&mut Instance> {
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
        let upserted = sanitize_tools(app_id, upserted);
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
        let Some(inst) = self.instance_mut(app_id, conn_id) else {
            return false;
        };
        let new: BTreeMap<String, ResourceInfo> =
            resources.into_iter().map(|r| (r.name.clone(), r)).collect();
        let changed = inst.resources != new;
        inst.resources = new;
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

    fn app_label(&self, app_id: &str) -> String {
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

    // ------------------------------------------------------------------
    // 页面目录与导航（第 4c 项，spec/hub-api.md 3.14）
    // ------------------------------------------------------------------

    /// 记下 SDK 上报的页面工具。
    fn learn_pages<'a>(&mut self, app_id: &str, tools: impl IntoIterator<Item = &'a SharedTool>) {
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

    /// 按路由规则选出调用 `tool_name` 的目标实例。
    pub fn route_tool(
        &self,
        app_id: &str,
        tool_name: &str,
        selected: Option<&str>,
    ) -> Result<ToolTarget, ToolError> {
        let Some(entry) = self.apps.get(app_id) else {
            return Err(ToolError::new(
                ErrorKind::ToolNotFound,
                format!("没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"),
            ));
        };
        let static_tool = entry.manifest.as_ref().and_then(|m| m.tool(tool_name));
        let label = self.app_label(app_id);
        if entry.instances.is_empty() {
            return Err(if static_tool.is_some() {
                self.disconnected_error(app_id)
            } else {
                ToolError::new(
                    ErrorKind::ToolNotFound,
                    format!("App{label}没有名为「{tool_name}」的工具。"),
                )
            });
        }
        let Some(inst) = entry
            .ordered(selected, |i| i.tools.contains_key(tool_name))
            .into_iter()
            .next()
        else {
            let message = if static_tool.is_some() {
                format!(
                    "工具「{tool_name}」当前不可用：App{label}已连接，但没有实例注册该工具，\
                     通常需要先在 App 中打开对应界面后再调用。"
                )
            } else {
                format!(
                    "App{label}没有名为「{tool_name}」的工具。可调用 apps.list 查看各实例注册的工具。"
                )
            };
            return Err(ToolError::new(ErrorKind::ToolNotFound, message));
        };
        if inst.visibility == Some(Visibility::Frozen) {
            let title = inst.title.as_deref().unwrap_or(&inst.instance_id);
            return Err(ToolError::new(
                ErrorKind::InstanceFrozen,
                format!(
                    "目标实例「{title}」的页面已被浏览器冻结（后台标签页）。请让用户切换到该页面后重试，\
                     或用 apps.select 选择其他实例。"
                ),
            )
            .with_details(json!({ "appId": app_id, "instanceId": inst.instance_id })));
        }
        let tool = inst.tools.get(tool_name).cloned().ok_or_else(|| {
            ToolError::new(
                ErrorKind::ToolNotFound,
                format!("工具「{tool_name}」不存在"),
            )
        })?;
        Ok(ToolTarget {
            instance_id: inst.instance_id.clone(),
            conn: inst.conn.clone(),
            tool,
        })
    }

    /// 按路由规则选出读取资源 `name` 的目标实例。
    pub fn route_resource(
        &self,
        app_id: &str,
        name: &str,
        selected: Option<&str>,
    ) -> Result<ResourceTarget, ToolError> {
        let Some(entry) = self.apps.get(app_id) else {
            return Err(ToolError::new(
                ErrorKind::ResourceNotFound,
                format!("没有 appId 为「{app_id}」的 App。"),
            ));
        };
        if entry.instances.is_empty() {
            return Err(self.disconnected_error(app_id));
        }
        let Some(inst) = entry
            .ordered(selected, |i| i.resources.contains_key(name))
            .into_iter()
            .next()
        else {
            return Err(ToolError::new(
                ErrorKind::ResourceNotFound,
                format!(
                    "App{}当前没有实例提供资源「{name}」。",
                    self.app_label(app_id)
                ),
            ));
        };
        let resource = inst.resources.get(name).cloned().ok_or_else(|| {
            ToolError::new(ErrorKind::ResourceNotFound, format!("资源「{name}」不存在"))
        })?;
        Ok(ResourceTarget {
            instance_id: inst.instance_id.clone(),
            conn: inst.conn.clone(),
            resource,
        })
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::Connection;

    fn tool(name: &str) -> ToolInfo {
        serde_json::from_value(json!({"name": name, "description": format!("{name} 工具"), "inputSchema": {"type": "object"}}))
            .unwrap()
    }

    fn res(name: &str) -> ResourceInfo {
        ResourceInfo {
            name: name.into(),
            description: "r".into(),
            mime_type: None,
            realtime: false,
            annotations: None,
        }
    }

    fn manifest() -> Manifest {
        app_mcp_manifest::parse(
            &json!({
                "manifestVersion": 1, "appId": "shop", "name": "示例商城",
                "launch": {"web": [{"type": "url", "href": "http://localhost:5173/"}]},
                "tools": [
                    {"name": "orders.search", "description": "搜索", "inputSchema": {"type": "object"}},
                    {"name": "cart.add", "description": "加购（静态）", "inputSchema": {"type": "object"}}
                ],
                "resources": [{"name": "cart.state", "description": "购物车"}]
            })
            .to_string(),
        )
        .unwrap()
    }

    fn add(
        reg: &mut Registry,
        app: &str,
        id: &str,
        conn_id: u64,
    ) -> (Arc<Connection>, Option<Arc<Connection>>) {
        let (conn, rx) = Connection::new(conn_id, format!("t-{conn_id}"));
        std::mem::forget(rx); // 保持通道打开
        let old = reg.add_instance(
            app,
            NewInstance {
                instance_id: id.into(),
                app_name: "Shop".into(),
                client_kind: ClientKind::Web,
                app_version: None,
                title: Some(format!("tab {id}")),
                url: None,
                overview: None,
                pid: None,
                navigate: false,
                conn: conn.clone(),
                wake: None,
            },
        );
        (conn, old)
    }

    #[test]
    fn overview_runtime_beats_manifest() {
        let mut reg = Registry::new();
        let mut m = manifest();
        m.overview = Some(AppOverview {
            summary: "静态简介".into(),
            body: None,
            locale: None,
        });
        reg.set_manifest(m);
        let o = reg.overview("shop").unwrap();
        assert_eq!(
            (o.summary.as_str(), o.source),
            ("静态简介", OverviewSource::Manifest)
        );
        assert_eq!(reg.summaries()[0].summary.as_deref(), Some("静态简介"));

        let (conn, rx) = Connection::new(9, "t-9");
        std::mem::forget(rx);
        reg.add_instance(
            "shop",
            NewInstance {
                instance_id: "a".into(),
                app_name: "Shop".into(),
                client_kind: ClientKind::Web,
                app_version: None,
                title: None,
                url: None,
                overview: Some(AppOverview {
                    summary: "运行时简介".into(),
                    body: Some("正文".into()),
                    locale: None,
                }),
                pid: None,
                navigate: false,
                conn: conn.clone(),
                wake: None,
            },
        );
        let o = reg.overview("shop").unwrap();
        assert_eq!(
            (o.summary.as_str(), o.source),
            ("运行时简介", OverviewSource::Runtime)
        );
        assert_eq!(o.app_name, "示例商城");
        assert_eq!(
            reg.apps_json(&HashMap::new())["apps"][0]["summary"],
            "运行时简介"
        );
        reg.remove_instance("shop", conn.id);
        assert_eq!(
            reg.overview("shop").unwrap().source,
            OverviewSource::Manifest
        );
        assert!(reg.overview("nope").is_none());
    }

    #[test]
    fn static_tools_when_disconnected() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let tools = reg.tools();
        assert_eq!(tools.len(), 2);
        assert!(
            tools
                .iter()
                .all(|t| t.availability == Availability::Disconnected)
        );
        let err = reg.route_tool("shop", "orders.search", None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::AppDisconnected);
        assert!(err.message.contains("http://localhost:5173/"));
        assert_eq!(
            reg.route_tool("shop", "nope", None).unwrap_err().kind,
            ErrorKind::ToolNotFound
        );
        assert_eq!(
            reg.route_tool("other", "x", None).unwrap_err().kind,
            ErrorKind::ToolNotFound
        );
        let resources = reg.resources();
        assert_eq!(resources.len(), 1);
        assert!(!resources[0].available);
    }

    #[test]
    fn connected_union_and_not_registered() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let (a, _) = add(&mut reg, "shop", "a", 1);
        let (b, _) = add(&mut reg, "shop", "b", 2);
        assert!(reg.sync_tools("shop", a.id, vec![tool("orders.search"), tool("x")]));
        assert!(!reg.sync_tools("shop", a.id, vec![tool("orders.search"), tool("x")]));
        assert!(reg.sync_tools("shop", b.id, vec![tool("x"), tool("y")]));
        let tools = reg.tools();
        let names: Vec<_> = tools
            .iter()
            .map(|t| (t.info.name.as_str(), t.availability))
            .collect();
        assert_eq!(
            names,
            vec![
                ("orders.search", Availability::Available),
                ("x", Availability::Available),
                ("y", Availability::Available),
                ("cart.add", Availability::NotRegistered),
            ]
        );
        let err = reg.route_tool("shop", "cart.add", None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ToolNotFound);
        assert!(err.message.contains("打开对应界面"));
        // 只在注册了工具的实例中选择
        assert_eq!(
            reg.route_tool("shop", "y", Some("a")).unwrap().instance_id,
            "b"
        );
        assert_eq!(
            reg.route_tool("shop", "orders.search", Some("b"))
                .unwrap()
                .instance_id,
            "a"
        );
    }

    #[test]
    fn invalid_tools_are_dropped() {
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        let mut bad = tool("ok");
        bad.input_schema = json!({"type": "array"});
        bad.name = "bad".into();
        reg.sync_tools("app", a.id, vec![tool("ok"), bad, tool("has space")]);
        assert_eq!(reg.tools().len(), 1);
    }

    #[test]
    fn change_tools_incremental() {
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        reg.sync_tools("app", a.id, vec![tool("x")]);
        assert!(reg.change_tools("app", a.id, vec![tool("y")], vec!["x".into()]));
        assert!(!reg.change_tools("app", a.id, vec![], vec!["zzz".into()]));
        let names: Vec<_> = reg.tools().into_iter().map(|t| t.info.name.clone()).collect();
        assert_eq!(names, vec!["y"]);
    }

    #[test]
    fn routing_focus_recency_and_connect_order() {
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        let (b, _) = add(&mut reg, "app", "b", 2);
        for c in [&a, &b] {
            reg.sync_tools("app", c.id, vec![tool("t")]);
        }
        // 都没有活跃记录：最早连接
        assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "a");
        // b 变为可见：最近活跃
        reg.set_visibility("app", b.id, Visibility::Visible, false);
        assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "b");
        // a 变为可见：a 更近
        reg.set_visibility("app", a.id, Visibility::Visible, false);
        assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "a");
        // b 聚焦
        reg.set_visibility("app", b.id, Visibility::Visible, true);
        assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "b");
        // b 失焦，a 完成一次调用后最近活跃
        reg.set_visibility("app", b.id, Visibility::Hidden, false);
        reg.touch("app", "a");
        assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "a");
        // 选择优先
        assert_eq!(
            reg.route_tool("app", "t", Some("b")).unwrap().instance_id,
            "b"
        );
    }

    #[test]
    fn view_tools_listed_from_primary_instance_and_pages_learned() {
        let view = |name: &str, page: &str| ToolInfo {
            surface: app_mcp_protocol::ToolSurface::View,
            page: Some(page.into()),
            ..tool(name)
        };
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        let (b, _) = add(&mut reg, "app", "b", 2);
        reg.sync_tools("app", a.id, vec![tool("bg"), view("a.view", "pa")]);
        reg.sync_tools("app", b.id, vec![view("b.view", "pb")]);
        reg.set_visibility("app", b.id, Visibility::Visible, true);
        let names = |reg: &Registry| reg.tools().into_iter().map(|t| t.info.name.clone()).collect::<Vec<_>>();
        // b 聚焦：只列 b 的 view 工具，a 的 app 工具照常
        assert_eq!(names(&reg), ["b.view", "bg"]);
        reg.set_visibility("app", a.id, Visibility::Visible, true);
        reg.set_visibility("app", b.id, Visibility::Hidden, false);
        assert_eq!(names(&reg), ["a.view", "bg"]);
        // 页面目录记下两个实例上报的页面；工具注销后仍在目录中
        reg.sync_tools("app", b.id, vec![]);
        let pages: Vec<String> = reg.pages("app").into_iter().map(|p| p.name).collect();
        assert_eq!(pages, ["pa", "pb"]);
        assert!(!reg.tool_registered("app", "b.view") && reg.tool_registered("app", "a.view"));
        assert!(reg.instance_has_tool("app", "a", "a.view") && !reg.instance_has_tool("app", "b", "a.view"));
        // 休眠快照中的 view 工具不列出
        reg.make_dormant("app", a.id, "r".into(), "h".into(), None);
        assert_eq!(names(&reg), ["bg"]);
        // 导航目标：只选声明了导航能力且已就绪的实例
        assert!(reg.navigation_target("app", None).is_none());
        assert!(reg.wake_plan_app("app", None).is_none(), "仍有已连接实例");
        reg.remove_instance("app", b.id);
        let plan = reg.wake_plan_app("app", None).expect("休眠实例");
        assert_eq!((plan.instance_id.as_deref(), plan.tool.is_none()), (Some("a"), true));
    }

    #[test]
    fn navigation_target_prefers_ready_capable_instances() {
        let mut reg = Registry::new();
        let (conn, rx) = Connection::new(9, "t-9");
        std::mem::forget(rx);
        let new = |id: &str, navigate: bool, conn: Arc<Connection>| NewInstance {
            instance_id: id.into(),
            app_name: "Shop".into(),
            client_kind: ClientKind::Native,
            app_version: None,
            title: None,
            url: None,
            overview: None,
            pid: None,
            navigate,
            conn,
            wake: None,
        };
        reg.add_instance("app", new("x", true, conn.clone()));
        assert!(reg.navigation_target("app", None).is_none(), "未就绪");
        reg.set_ready("app", conn.id);
        assert_eq!(reg.navigation_target("app", None).map(|(id, _)| id).as_deref(), Some("x"));
        let (c2, rx) = Connection::new(10, "t-10");
        std::mem::forget(rx);
        reg.add_instance("app", new("y", false, c2.clone()));
        reg.set_ready("app", c2.id);
        assert_eq!(reg.navigation_target("app", Some("y")).map(|(id, _)| id).as_deref(), Some("x"), "y 不支持导航");
        reg.set_visibility("app", conn.id, Visibility::Frozen, false);
        assert!(reg.navigation_target("app", None).is_none(), "冻结的实例不导航");
    }

    #[test]
    fn repeated_visible_does_not_bump() {
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        let (b, _) = add(&mut reg, "app", "b", 2);
        for c in [&a, &b] {
            reg.sync_tools("app", c.id, vec![tool("t")]);
        }
        reg.set_visibility("app", a.id, Visibility::Visible, false);
        reg.set_visibility("app", b.id, Visibility::Visible, false);
        reg.set_visibility("app", a.id, Visibility::Visible, false);
        assert_eq!(reg.route_tool("app", "t", None).unwrap().instance_id, "b");
    }

    #[test]
    fn frozen_target_errors() {
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        reg.sync_tools("app", a.id, vec![tool("t")]);
        reg.set_visibility("app", a.id, Visibility::Frozen, false);
        assert_eq!(
            reg.route_tool("app", "t", None).unwrap_err().kind,
            ErrorKind::InstanceFrozen
        );
    }

    #[test]
    fn replace_same_instance_id() {
        let mut reg = Registry::new();
        let (a1, _) = add(&mut reg, "app", "a", 1);
        reg.sync_tools("app", a1.id, vec![tool("t")]);
        let (a2, old) = add(&mut reg, "app", "a", 2);
        assert_eq!(old.map(|c| c.id), Some(1));
        // 旧连接断开不影响新实例
        assert!(reg.remove_instance("app", a1.id).is_none());
        assert!(reg.instance("app", "a").is_some());
        assert!(reg.tools().is_empty());
        assert!(reg.remove_instance("app", a2.id).is_some());
        assert!(!reg.has_app("app"));
    }

    #[test]
    fn remove_keeps_manifest_entry() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let (a, _) = add(&mut reg, "shop", "a", 1);
        reg.remove_instance("shop", a.id);
        assert!(reg.has_app("shop"));
        assert_eq!(reg.tools()[0].availability, Availability::Disconnected);
    }

    #[test]
    fn resources_and_subscriptions() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let (a, _) = add(&mut reg, "shop", "a", 1);
        assert!(reg.sync_resources("shop", a.id, vec![res("cart.state")]));
        assert_eq!(reg.resources().len(), 1);
        assert!(reg.resources()[0].available);
        assert_eq!(
            reg.route_resource("shop", "cart.state", None)
                .unwrap()
                .instance_id,
            "a"
        );
        assert_eq!(
            reg.route_resource("shop", "x", None).unwrap_err().kind,
            ErrorKind::ResourceNotFound
        );
        reg.mark_subscribed("shop", a.id, "cart.state", true);
        assert!(reg.resource_holders("shop", "cart.state")[0].1);
        assert!(reg.change_resources("shop", a.id, vec![], vec!["cart.state".into()]));
        assert!(reg.resource_holders("shop", "cart.state").is_empty());
        assert!(reg.instance("shop", "a").unwrap().subscriptions.is_empty());
    }

    #[test]
    fn apps_json_shape() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let (a, _) = add(&mut reg, "shop", "a", 1);
        let (_b, _) = add(&mut reg, "shop", "b", 2);
        reg.sync_tools("shop", a.id, vec![tool("x")]);
        let mut sel = HashMap::new();
        sel.insert("shop".to_string(), "b".to_string());
        let v = reg.apps_json(&sel);
        let app = &v["apps"][0];
        assert_eq!(app["appId"], "shop");
        assert_eq!(app["name"], "示例商城");
        assert_eq!(app["connected"], true);
        assert_eq!(app["staticToolCount"], 2);
        assert_eq!(app["selectedInstanceId"], "b");
        assert_eq!(app["defaultInstanceId"], "b");
        assert_eq!(app["instances"].as_array().unwrap().len(), 2);
        assert_eq!(app["instances"][0]["tools"], json!(["x"]));
        sel.insert("shop".to_string(), "gone".to_string());
        assert_eq!(
            reg.apps_json(&sel)["apps"][0]["selectedInstanceId"],
            Value::Null
        );
    }

    #[test]
    fn dormant_snapshot_listing_and_plans() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let (a, _) = add(&mut reg, "shop", "a", 1);
        reg.sync_tools("shop", a.id, vec![tool("x"), tool("orders.search")]);
        reg.sync_resources("shop", a.id, vec![res("cart.state")]);
        let hash = app_mcp_protocol::tools_hash(
            &ToolsSyncParams { tools: vec![tool("x"), tool("orders.search")] },
            &ResourcesSyncParams { resources: vec![res("cart.state")] },
        );
        let wake = WakeDescriptor { kind: WakeKind::Uri, target: Some("shop".into()), background: true };
        assert_eq!(
            reg.make_dormant("shop", a.id, "rt".into(), hash.clone(), Some(wake.clone())).as_deref(),
            Some("a")
        );
        assert!(reg.make_dormant("shop", a.id, "rt".into(), hash.clone(), None).is_none());
        let d = reg.dormant("shop", "a").unwrap();
        assert_eq!(d.snapshot_hash(), hash);
        // 工具仍列出，标记 Dormant；未注册的静态工具仍为 Disconnected
        let listed: Vec<_> = reg.tools().into_iter().map(|t| (t.info.name.clone(), t.availability)).collect();
        assert_eq!(
            listed,
            vec![
                ("orders.search".to_string(), Availability::Dormant),
                ("x".to_string(), Availability::Dormant),
                ("cart.add".to_string(), Availability::Disconnected),
            ]
        );
        assert!(reg.resources().iter().all(|r| r.available));
        let info = &reg.app_infos(&HashMap::new())[0];
        assert!(!info.connected);
        assert_eq!(info.dormant_instances[0].instance_id, "a");
        assert_eq!(reg.apps_json(&HashMap::new())["apps"][0]["dormant"], true);

        let p = reg.wake_plan_tool("shop", "x", None, false).unwrap();
        assert_eq!((p.instance_id.as_deref(), p.descriptor.as_ref()), (Some("a"), Some(&wake)));
        // 只在清单中的工具：冷启动计划（实例为 None）
        let p = reg.wake_plan_tool("shop", "cart.add", None, false).unwrap();
        assert!(p.instance_id.is_none());
        assert!(reg.wake_plan_tool("shop", "nope", None, false).is_none());
        assert!(reg.wake_plan_tool("shop", "x", Some("zzz"), true).is_none());
        assert!(reg.wake_plan_resource("shop", "cart.state", None).is_some());

        // 回连：已连接实例注册了工具时不需要唤醒
        let snap = reg.take_dormant("shop", "a").unwrap();
        let (a2, _) = add(&mut reg, "shop", "a", 2);
        reg.restore_snapshot("shop", a2.id, &snap);
        assert!(reg.wake_plan_tool("shop", "x", None, false).is_none());
        assert_eq!(reg.route_tool("shop", "x", None).unwrap().instance_id, "a");
    }

    #[test]
    fn dormant_expiry_and_clear() {
        let mut reg = Registry::new();
        let (a, _) = add(&mut reg, "app", "a", 1);
        reg.make_dormant("app", a.id, "r".into(), String::new(), None);
        assert!(reg.has_app("app"));
        assert!(reg.expire_dormant(SystemTime::UNIX_EPOCH).is_empty());
        let later = SystemTime::now() + std::time::Duration::from_secs(1);
        assert_eq!(reg.expire_dormant(later), vec![("app".to_string(), "a".to_string())]);
        assert!(!reg.has_app("app"));
        let (b, _) = add(&mut reg, "app", "b", 2);
        reg.make_dormant("app", b.id, "r".into(), String::new(), None);
        assert_eq!(reg.clear_dormant("app"), vec!["b".to_string()]);
    }

    #[test]
    fn wake_target_presence_and_known() {
        let mut reg = Registry::new();
        assert_eq!(reg.wake_target_presence("shop", None), WakeTargetPresence::Absent);
        let (a, _) = add(&mut reg, "shop", "a", 1);
        // 已连接、未就绪：握手中
        assert_eq!(reg.wake_target_presence("shop", Some("a")), WakeTargetPresence::Handshaking);
        assert_eq!(reg.wake_target_presence("shop", None), WakeTargetPresence::Handshaking);
        assert_eq!(reg.wake_target_presence("shop", Some("b")), WakeTargetPresence::Absent);
        reg.set_ready("shop", a.id);
        assert_eq!(reg.wake_target_presence("shop", Some("a")), WakeTargetPresence::Ready("a".into()));
        assert_eq!(reg.wake_target_presence("shop", None), WakeTargetPresence::Ready("a".into()));
        assert!(reg.instance_known("shop", "a"));
        // 休眠：仍有记录，但不算已连接
        reg.make_dormant("shop", a.id, "rt".into(), String::new(), None);
        assert_eq!(reg.wake_target_presence("shop", Some("a")), WakeTargetPresence::Absent);
        assert!(reg.instance_known("shop", "a"));
        reg.clear_dormant("shop");
        assert!(!reg.instance_known("shop", "a"));
        assert!(!reg.instance_known("other", "a"));
    }

    /// 回归（第 4f 项 d）：同一工具定义在实例、休眠快照、页面目录与列表之间共享（`Arc`），列出 / 计数不深拷贝。
    #[test]
    fn tool_definitions_are_shared_not_copied() {
        let mut reg = Registry::new();
        reg.set_manifest(manifest());
        let (a, _) = add(&mut reg, "shop", "a", 1);
        let mut page_tool = tool("orders.view");
        page_tool.page = Some("orders".into());
        reg.sync_tools("shop", a.id, vec![tool("orders.search"), page_tool]);
        let live = reg.instance("shop", "a").unwrap().tools["orders.view"].clone();
        let catalog = reg.pages("shop");
        let (_, in_catalog) = pages::find_tool(&catalog, "orders.view").unwrap();
        assert!(Arc::ptr_eq(&live, in_catalog), "页面目录与实例共享定义");
        let listed = reg.tools();
        let listed_view = listed.iter().find(|t| t.info.name == "orders.view").unwrap();
        assert!(Arc::ptr_eq(&live, &listed_view.info), "列表不复制定义");
        assert_eq!(reg.tool_count(), listed.len());
        assert_eq!(reg.tools_of(|app| app == "other").len(), 0);
        assert_eq!(reg.tools_of(|app| app == "shop").len(), listed.len());

        reg.make_dormant("shop", a.id, "rt".into(), String::new(), None);
        let snap = reg.dormant("shop", "a").unwrap();
        assert!(Arc::ptr_eq(&live, &snap.tools["orders.view"]), "休眠快照沿用同一份定义");
        let (b, _) = add(&mut reg, "shop", "a", 2);
        let snap = reg.take_dormant("shop", "a").unwrap();
        reg.restore_snapshot("shop", b.id, &snap);
        assert!(Arc::ptr_eq(&live, &reg.instance("shop", "a").unwrap().tools["orders.view"]), "快速恢复不复制定义");
        // 静态清单的工具也只有一份：列表与路由取到的是同一个。
        let static_listed = reg.tools().into_iter().find(|t| t.info.name == "cart.add").unwrap();
        assert!(Arc::ptr_eq(&static_listed.info, reg.manifest("shop").unwrap().tool("cart.add").unwrap()));
    }
}
