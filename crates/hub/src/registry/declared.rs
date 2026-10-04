//! 全量同步（`tools/sync` / `resources/sync`）的声明比较：与 Hub 此前已知的声明相同时，结果缓存不必清空
//! （spec/hub-api.md 3.20）。
//!
//! 「此前已知」按名称逐项取：本实例此前的声明（[`Instance::prior_tools`] / [`Instance::prior_resources`]，或已同步过的
//! 当前列表）→ 与缓存查定义同一来源（[`Registry::app_tool`] / [`Registry::app_resource`]：其他已连接实例 → 休眠快照 → 清单）。
//!
//! @why App 冷启动（被系统回收后再唤醒）握手都会全量同步；声明未变时保留缓存，数据新旧由条目 TTL 兜底（与休眠期间同理）。

use std::collections::BTreeMap;

use app_mcp_protocol::ResourceInfo;

use crate::tool_def::SharedTool;

use super::Registry;
use super::records::{DormantInstance, Instance};

/// 新声明与此前已知的是否不同：本实例此前声明过而新列表没有的名称，或新列表中有名称的定义与此前已知的不等
/// （此前未知也算不同）。
fn differs<T: PartialEq>(
    own: &BTreeMap<String, T>,
    new: &[(&str, &T)],
    known_elsewhere: impl Fn(&str) -> Option<T>,
) -> bool {
    let removed = own.keys().any(|name| !new.iter().any(|(n, _)| *n == name.as_str()));
    removed
        || new.iter().any(|(name, def)| match own.get(*name) {
            Some(prev) => prev != *def,
            None => known_elsewhere(name).as_ref() != Some(*def),
        })
}

impl Registry {
    fn own_instance(&self, app_id: &str, conn_id: u64) -> Option<&Instance> {
        self.apps.get(app_id)?.instances.iter().find(|i| i.conn.id == conn_id)
    }

    /// 回连实例未能快速恢复时：记住取出的休眠快照，作为本实例首次全量同步的比较基准。
    pub(crate) fn remember_prior(&mut self, app_id: &str, conn_id: u64, snapshot: DormantInstance) {
        if let Some(inst) = self.instance_mut(app_id, conn_id) {
            inst.prior_tools = Some(snapshot.tools);
            inst.prior_resources = Some(snapshot.resources);
        }
    }

    /// App 最近活跃的休眠记录（清除全部休眠记录前取出，作为新实例的比较基准）。
    pub(crate) fn latest_dormant(&self, app_id: &str) -> Option<DormantInstance> {
        self.apps.get(app_id)?.dormant_ordered(None, |_| true).first().map(|d| (*d).clone())
    }

    /// 本实例此前的工具声明（回连前的快照，或已同步过的当前列表）；实例未知时为空。
    fn own_prior_tools(&self, app_id: &str, conn_id: u64) -> Option<&BTreeMap<String, SharedTool>> {
        self.own_instance(app_id, conn_id).map(|i| i.prior_tools.as_ref().unwrap_or(&i.tools))
    }

    /// `tools/sync` 的声明（已经过 [`super::sanitize_tools`]）是否与此前已知的不同。须在同步写入注册表之前调用。
    pub(crate) fn tools_declaration_differs(&self, app_id: &str, conn_id: u64, tools: &[SharedTool]) -> bool {
        let empty = BTreeMap::new();
        let own = self.own_prior_tools(app_id, conn_id).unwrap_or(&empty);
        let new: Vec<(&str, &SharedTool)> = tools.iter().map(|t| (t.name.as_str(), t)).collect();
        differs(own, &new, |name| self.app_tool(app_id, name))
    }

    /// 新声明中定义变了的同名工具：`(此前已知, 新)`，基准同 [`Self::tools_declaration_differs`]（本实例此前的声明 →
    /// 其他已连接实例 → 休眠快照 → 清单）；此前未知的（新工具）与本实例删除的不列出（spec/hub-api.md 3.21）。
    /// 须在写入注册表之前调用（`tools/sync` 与 `tools/changed` 的 `upserted`）。
    pub(crate) fn redefined_tools(&self, app_id: &str, conn_id: u64, tools: &[SharedTool]) -> Vec<(SharedTool, SharedTool)> {
        let own = self.own_prior_tools(app_id, conn_id);
        tools
            .iter()
            .filter_map(|new| {
                let prev = own.and_then(|o| o.get(&new.name).cloned()).or_else(|| self.app_tool(app_id, &new.name))?;
                (prev != *new).then(|| (prev, new.clone()))
            })
            .collect()
    }

    /// `resources/sync` 的声明（已经过 [`super::sanitize_resources`]）是否与此前已知的不同。须在同步写入注册表之前调用。
    pub(crate) fn resources_declaration_differs(&self, app_id: &str, conn_id: u64, resources: &[ResourceInfo]) -> bool {
        let empty = BTreeMap::new();
        let own =
            self.own_instance(app_id, conn_id).map_or(&empty, |i| i.prior_resources.as_ref().unwrap_or(&i.resources));
        let new: Vec<(&str, &ResourceInfo)> = resources.iter().map(|r| (r.name.as_str(), r)).collect();
        differs(own, &new, |name| self.app_resource(app_id, name))
    }
}
