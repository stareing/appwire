//! 列表（`apps.list`、Hub API、格式导出）：App 与工具清单、总览、导出名映射。

use std::collections::HashMap;

use app_mcp_protocol::ToolAnnotations;
use serde_json::{Value, json};

use crate::call;
use crate::format::NameCodec;
use crate::policy::PolicyConfig;
#[cfg(feature = "mcp-server")]
use crate::overview::AppSummary;
use crate::overview::Overview;
use crate::types::HubTool;

use super::{HubShared, lock};
use super::upstreams::upstream_overview;

impl HubShared {
    pub(crate) fn apps_json(&self, selected: &HashMap<String, String>) -> Value {
        let mut v = self.registry().apps_json(selected);
        let policy = self.policy();
        if policy.has_hide() {
            self.hide_in_apps_json(&policy, &mut v);
        }
        let ups: Vec<Value> = lock(&self.upstreams)
            .iter()
            .filter(|(name, _)| policy.app_hidden(name).is_none())
            .map(|(name, st)| {
                json!({
                    "appId": name,
                    "kind": "upstream",
                    "name": st.server_name.clone().unwrap_or_else(|| name.clone()),
                    "summary": upstream_overview(name, st).map(|o| o.summary),
                    "connected": st.connected(),
                    "command": st.config.command,
                    "tools": st
                        .tools
                        .iter()
                        .filter(|t| policy.tool_hidden(name, &t.name, Some(&call::upstream_hub_tool(name, t).annotations)).is_none())
                        .map(|t| t.name.to_string())
                        .collect::<Vec<_>>(),
                    "resourceCount": st.resources.len(),
                    "restarts": st.restarts,
                    "lastError": st.last_error,
                })
            })
            .collect();
        if let Some(apps) = v["apps"].as_array_mut() {
            apps.extend(ups);
        }
        v
    }

    /// `apps.list` 中去掉被 `hide` 隐藏的 App，并从各实例的 `tools` 与 `staticToolCount` 中去掉被隐藏的工具。
    fn hide_in_apps_json(&self, policy: &PolicyConfig, v: &mut Value) {
        let annotations: HashMap<(String, String), ToolAnnotations> = self
            .registry()
            .tools()
            .into_iter()
            .map(|t| ((t.app_id, t.info.name.clone()), t.info.effective_annotations()))
            .collect();
        let visible = |app_id: &str, tool: &str| {
            let a = annotations.get(&(app_id.to_owned(), tool.to_owned()));
            policy.tool_hidden(app_id, tool, a).is_none()
        };
        let Some(apps) = v.get_mut("apps").and_then(Value::as_array_mut) else {
            return;
        };
        apps.retain(|a| a["appId"].as_str().is_some_and(|id| policy.app_hidden(id).is_none()));
        for app in apps.iter_mut() {
            let app_id = app["appId"].as_str().unwrap_or_default().to_owned();
            for key in ["instances", "dormantInstances"] {
                for inst in app.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
                    if let Some(tools) = inst.get_mut("tools").and_then(Value::as_array_mut) {
                        tools.retain(|t| t.as_str().is_some_and(|t| visible(&app_id, t)));
                    }
                }
            }
            let static_count = self
                .registry()
                .manifest(&app_id)
                .map_or(0, |m| m.tools().iter().filter(|t| visible(&app_id, &t.name)).count());
            app["staticToolCount"] = json!(static_count);
            app["pageCount"] = json!(self.page_catalog(&app_id).len());
        }
    }

    /// App 或上游当前生效的总览。
    pub(crate) fn overview(&self, app_id: &str) -> Option<Overview> {
        if self.app_hidden(app_id) {
            return None;
        }
        if let Some(st) = lock(&self.upstreams).get(app_id) {
            return upstream_overview(app_id, st);
        }
        self.registry().overview(app_id)
    }

    /// 所有已知 App 与上游的一句话简介（按 appId 排序）。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn summaries(&self) -> Vec<AppSummary> {
        let mut out = self.registry().summaries();
        out.extend(lock(&self.upstreams).iter().map(|(name, st)| AppSummary {
            app_id: name.clone(),
            name: st.server_name.clone().unwrap_or_else(|| name.clone()),
            summary: upstream_overview(name, st).map(|o| o.summary),
        }));
        let policy = self.policy();
        out.retain(|s| policy.app_hidden(&s.app_id).is_none());
        out.sort_by(|a, b| a.app_id.cmp(&b.app_id));
        out
    }

    // ------------------------------------------------------------------
    // 列表（Hub API / 格式导出）
    // ------------------------------------------------------------------

    /// 全部工具：`(工具, 是否内置)`，顺序为内置、App（按 appId）、上游。
    /// `with_apps_tools`：内置工具是否包含 `apps.tools`（渐进暴露生效时才列出）。
    /// `app`：只取 appId（或上游名）满足条件的 App / 上游工具（先过滤再构造，构造会解析 schema）；内置工具总是包含。
    pub(crate) fn all_tools(&self, with_apps_tools: bool, with_apps_page: bool, app: impl Fn(&str) -> bool) -> Vec<(HubTool, bool)> {
        let set = call::BuiltinSet { apps_tools: with_apps_tools, apps_page: with_apps_page, tasks: false, locks: self.locks_enabled() };
        let mut out: Vec<(HubTool, bool)> = call::builtin_hub_tools(set)
            .into_iter()
            .map(|t| (t, true))
            .collect();
        self.registry().visit_tools(&app, |app_id, t, availability| {
            out.push((call::app_hub_tool(app_id, t, availability), false));
        });
        let ups = lock(&self.upstreams);
        for (name, st) in ups.iter().filter(|(name, _)| app(name)) {
            for t in &st.tools {
                out.push((call::upstream_hub_tool(name, t), false));
            }
        }
        out
    }

    /// [`Self::all_tools`] 去掉被 `hide` 规则隐藏的 App 工具与上游工具（Agent 可见的列表）。
    pub(crate) fn visible_tools(&self, with_apps_tools: bool, app: impl Fn(&str) -> bool) -> Vec<(HubTool, bool)> {
        let policy = self.policy();
        let mut tools = self.all_tools(with_apps_tools, self.has_pages(), app);
        if policy.has_hide() {
            tools.retain(|(t, builtin)| *builtin || policy.tool_hidden(&t.app_id, &t.tool, Some(&t.annotations)).is_none());
        }
        tools
    }

    /// 全部工具的全名（顺序同 [`Self::all_tools`]，只取名称，不构造定义）。
    fn all_tool_names(&self) -> Vec<String> {
        let set = call::BuiltinSet { apps_tools: true, apps_page: true, tasks: false, locks: true };
        let mut out: Vec<String> = call::builtin_hub_tools(set).into_iter().map(|t| t.name).collect();
        self.registry().visit_tools(|_| true, |app_id, t, _| out.push(format!("{app_id}.{}", t.name)));
        for (name, st) in lock(&self.upstreams).iter() {
            out.extend(st.tools.iter().map(|t| format!("{name}.{}", t.name)));
        }
        out
    }

    /// 按当前全部工具计算导出名，并并入历史映射。
    pub(crate) fn name_codec(&self) -> NameCodec {
        // 导出名按全部工具（含 apps.tools）计算，与渐进暴露无关，保证名称稳定。
        let names = self.all_tool_names();
        let codec = NameCodec::new(names.iter().map(String::as_str));
        lock(&self.export_names).extend(codec.pairs().map(|(full, export)| (export.to_owned(), full.to_owned())));
        codec
    }

    /// 把导出名解析为全名；不认识的名称原样返回（视为全名）。
    pub(crate) fn resolve_export_name(&self, name: &str) -> String {
        if let Some(full) = lock(&self.export_names).get(name) {
            return full.clone();
        }
        let codec = self.name_codec();
        codec.full_name(name).unwrap_or(name).to_owned()
    }
}
