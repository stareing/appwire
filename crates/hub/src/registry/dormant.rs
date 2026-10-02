//! 休眠（spec/lifecycle.md §9）：休眠记录的建立、恢复、过期，以及唤醒计划。

use std::time::SystemTime;

use app_mcp_protocol::{ResourcesSyncParams, ToolsSyncParams, WakeDescriptor};

use crate::tool_def::SharedTool;

use super::Registry;
use super::records::{DormantInstance, WakePlan, WakeTargetPresence};

impl Registry {
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

    pub(super) fn plan_for(&self, app_id: &str, d: &DormantInstance, tool: Option<SharedTool>) -> WakePlan {
        WakePlan {
            app_id: app_id.to_owned(),
            instance_id: Some(d.instance_id.clone()),
            descriptor: d.wake_descriptor().cloned(),
            tool,
        }
    }

}
