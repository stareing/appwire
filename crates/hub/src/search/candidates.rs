//! 检索候选（spec/hub-api.md 3.18「候选」）：调用方可见的 App 工具与上游工具（与 `tools/list` 同一过滤）∪ 各 App 页面目录中的
//! 工具。不含内置工具；同名只出现一次（先列出的优先：已注册 → 休眠快照 → 清单 → 页面目录）。
//!
//! 只读注册表、休眠快照、清单、上游工具缓存与页面目录；不唤醒、不连接。

use std::collections::{HashMap, HashSet};

use crate::call::app_hub_tool;
use crate::hub::HubShared;
use crate::types::{Availability, HubTool};

/// 一个候选工具与其上下文文本（App 名称、所在页面的标题与描述）。
pub(super) struct Candidate {
    pub tool: HubTool,
    pub context: Vec<String>,
}

/// 页面的标题与描述（作为上下文文本）。
type PageText = (Option<String>, Option<String>);

impl HubShared {
    /// `app`：只取该 appId（或上游名）的工具；`None` = 全部。
    pub(super) fn search_candidates(&self, app: Option<&str>) -> Vec<Candidate> {
        let wanted = |a: &str| app.is_none_or(|x| x == a);
        let mut seen: HashSet<String> = HashSet::new();
        let mut tools: Vec<HubTool> = self
            .visible_tools(false, wanted)
            .into_iter()
            .filter(|(t, builtin)| !builtin && seen.insert(t.name.clone()))
            .map(|(t, _)| t)
            .collect();
        let app_ids: Vec<String> = self.registry().app_ids().into_iter().filter(|a| wanted(a)).collect();
        let mut pages: HashMap<String, HashMap<String, PageText>> = HashMap::new();
        for app_id in app_ids {
            let catalog = self.page_catalog(&app_id);
            let reg = self.registry();
            for p in &catalog {
                for t in p.tools.values() {
                    if !seen.insert(format!("{app_id}.{}", t.name)) {
                        continue;
                    }
                    let availability =
                        if reg.tool_registered(&app_id, &t.name) { Availability::Available } else { Availability::NotRegistered };
                    let mut tool = app_hub_tool(&app_id, t, availability);
                    tool.page.get_or_insert_with(|| p.name.clone());
                    tools.push(tool);
                }
            }
            drop(reg);
            let texts = catalog.into_iter().map(|p| (p.name, (p.title, p.description))).collect();
            pages.insert(app_id, texts);
        }
        tools
            .into_iter()
            .map(|tool| {
                let page = tool.page.as_ref().and_then(|p| pages.get(&tool.app_id)?.get(p)).cloned();
                let mut context = vec![self.search_app_name(&tool.app_id)];
                if let Some((title, description)) = page {
                    context.extend(title);
                    context.extend(description);
                }
                Candidate { tool, context }
            })
            .collect()
    }

    /// App 的显示名（清单名 / 实例上报名），上游为服务器名；都没有时为 appId。
    fn search_app_name(&self, app_id: &str) -> String {
        if self.is_upstream(app_id) {
            return self.upstream_display_name(app_id);
        }
        self.registry().display_name(app_id).unwrap_or_else(|| app_id.to_owned())
    }
}
