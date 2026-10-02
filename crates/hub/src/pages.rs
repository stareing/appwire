//! 页面目录（第 4c 项，spec/hub-api.md 3.14）：清单 `pages` 与 SDK 上报的 `ToolInfo.page` 合成的"页面 → 工具"表。
//!
//! 纯数据，不做 I/O；由 [`crate::registry::Registry`] 按 App 持有。页面目录只用于渐进披露（`apps.tools` 的页面摘要、
//! `apps.page`）与"调用不在当前页面的工具时先导航"，不进入 MCP `tools/list`。

use std::collections::BTreeMap;

use app_mcp_protocol::is_valid_name;
use serde_json::Value;

use crate::tool_def::{SharedTool, StaticManifest};

/// 每个 App 从 SDK 上报中记下的页面数上限（spec/hub-api.md 3.11 的资源保护同类：防止失控的 App 撑大 Hub 内存）。
pub const MAX_LEARNED_PAGES: usize = 64;
/// 每个页面记下的工具数上限。
pub const MAX_LEARNED_PAGE_TOOLS: usize = 128;

/// SDK 上报过的页面工具（`ToolInfo.page` 非空）。工具从实例注销后仍保留，Hub 据此在工具不在当前页面时导航。
/// 与实例共享同一份定义（[`SharedTool`]）。
///
/// @invariant 同一工具名只出现在一个页面中（工具名在 App 内唯一；上报的页面变化时移到新页面）。
#[derive(Debug, Default)]
pub(crate) struct LearnedPages {
    pages: BTreeMap<String, BTreeMap<String, SharedTool>>,
}

impl LearnedPages {
    /// 记下带 `page` 的工具（运行时定义覆盖之前记下的）；超出上限的忽略并记 warn 日志。返回目录是否变化。
    pub fn learn<'a>(&mut self, app_id: &str, tools: impl IntoIterator<Item = &'a SharedTool>) -> bool {
        let mut changed = false;
        for t in tools {
            let Some(page) = t.page.as_deref().filter(|p| is_valid_name(p)) else {
                continue;
            };
            if self.pages.get(page).and_then(|p| p.get(&t.name)) == Some(t) {
                continue;
            }
            for (name, tools) in self.pages.iter_mut().filter(|(name, _)| name.as_str() != page) {
                if tools.remove(&t.name).is_some() {
                    tracing::debug!(app_id, tool = %t.name, from = %name, to = page, "工具所在页面变化");
                }
            }
            self.pages.retain(|_, tools| !tools.is_empty());
            if !self.pages.contains_key(page) && self.pages.len() >= MAX_LEARNED_PAGES {
                tracing::warn!(app_id, page, limit = MAX_LEARNED_PAGES, "上报的页面数超出上限，忽略");
                continue;
            }
            let entry = self.pages.entry(page.to_owned()).or_default();
            if !entry.contains_key(&t.name) && entry.len() >= MAX_LEARNED_PAGE_TOOLS {
                tracing::warn!(app_id, page, limit = MAX_LEARNED_PAGE_TOOLS, "页面上报的工具数超出上限，忽略");
                continue;
            }
            entry.insert(t.name.clone(), t.clone());
            changed = true;
        }
        changed
    }
}

/// 页面目录中的一个页面（清单声明与运行时上报合并后）。
#[derive(Debug, Clone, PartialEq)]
pub struct PageEntry {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub route: Option<String>,
    /// 导航参数的 JSON Schema（清单 `pages[].params`）。
    pub params: Option<Value>,
    /// 清单声明 `navigable: false` 时为 false（Hub 不导航，返回 `NAVIGATION_DENIED`）。
    pub navigable: bool,
    /// 清单中声明了该页面（否则只来自工具上的 `page` 字段）。
    pub declared: bool,
    /// 页面上的工具：清单定义，运行时上报的同名定义优先。
    pub tools: BTreeMap<String, SharedTool>,
}

impl PageEntry {
    fn undeclared(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            title: None,
            description: None,
            route: None,
            params: None,
            navigable: true,
            declared: false,
            tools: BTreeMap::new(),
        }
    }
}

/// 合成某个 App 的页面目录：清单 `pages`（含其 `tools`）→ 清单顶层带 `page` 的工具 → SDK 上报的页面工具（覆盖同名）。
/// 按页面名排序。
pub(crate) fn catalog(manifest: Option<&StaticManifest>, learned: &LearnedPages) -> Vec<PageEntry> {
    let mut pages: BTreeMap<String, PageEntry> = BTreeMap::new();
    if let Some(m) = manifest {
        for (p, page_tools) in m.pages() {
            let tools = page_tools.iter().map(|t| (t.name.clone(), t.clone())).collect();
            pages.insert(
                p.name.clone(),
                PageEntry {
                    name: p.name.clone(),
                    title: p.title.clone(),
                    description: p.description.clone(),
                    route: p.route.clone(),
                    params: p.params.clone(),
                    navigable: p.navigable,
                    declared: true,
                    tools,
                },
            );
        }
        for t in m.tools() {
            if let Some(page) = t.page.as_deref().filter(|p| is_valid_name(p)) {
                pages.entry(page.to_owned()).or_insert_with(|| PageEntry::undeclared(page)).tools.insert(t.name.clone(), t.clone());
            }
        }
    }
    for (page, tools) in &learned.pages {
        for t in tools.values() {
            for (name, entry) in pages.iter_mut().filter(|(name, _)| *name != page) {
                if entry.tools.remove(&t.name).is_some() {
                    tracing::trace!(tool = %t.name, from = %name, to = %page, "运行时上报的页面覆盖清单");
                }
            }
            pages.entry(page.clone()).or_insert_with(|| PageEntry::undeclared(page)).tools.insert(t.name.clone(), t.clone());
        }
    }
    pages.into_values().collect()
}

/// 页面目录中声明了 `tool` 的页面与该工具的定义。
pub(crate) fn find_tool<'a>(pages: &'a [PageEntry], tool: &str) -> Option<(&'a PageEntry, &'a SharedTool)> {
    pages.iter().find_map(|p| p.tools.get(tool).map(|t| (p, t)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_def::ToolDef;
    use app_mcp_protocol::ToolInfo;
    use serde_json::json;
    use std::sync::Arc;

    fn tool(name: &str, page: Option<&str>) -> SharedTool {
        tool_described(name, page, "d")
    }

    fn tool_described(name: &str, page: Option<&str>, description: &str) -> SharedTool {
        let mut t: ToolInfo =
            serde_json::from_value(json!({"name": name, "description": description, "inputSchema": {"type": "object"}}))
                .expect("tool");
        t.page = page.map(str::to_owned);
        Arc::new(ToolDef::from_info(t))
    }

    fn manifest() -> StaticManifest {
        StaticManifest::new(app_mcp_manifest::parse(
            &json!({
                "manifestVersion": 1, "appId": "shop", "name": "商城",
                "tools": [
                    {"name": "info", "description": "d", "inputSchema": {"type": "object"}},
                    {"name": "orders.export", "description": "d", "inputSchema": {"type": "object"}, "page": "orders"}
                ],
                "pages": [
                    {"name": "cart", "title": "购物车", "tools": [
                        {"name": "cart.checkout", "description": "结算", "inputSchema": {"type": "object"}, "surface": "view"}
                    ]},
                    {"name": "admin", "navigable": false, "tools": [
                        {"name": "admin.reset", "description": "d", "inputSchema": {"type": "object"}}
                    ]}
                ]
            })
            .to_string(),
        )
        .expect("manifest"))
    }

    #[test]
    fn catalog_merges_manifest_and_learned() {
        let mut learned = LearnedPages::default();
        let checkout = tool_described("cart.checkout", Some("cart"), "运行时结算");
        assert!(learned.learn("shop", [&checkout, &tool("orders.cancel", Some("orders")), &tool("plain", None)]));
        assert!(!learned.learn("shop", [&checkout]), "相同定义不算变化");
        let pages = catalog(Some(&manifest()), &learned);
        let names: Vec<&str> = pages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["admin", "cart", "orders"]);
        let cart = &pages[1];
        assert_eq!((cart.title.as_deref(), cart.declared, cart.navigable), (Some("购物车"), true, true));
        assert_eq!(cart.tools["cart.checkout"].description, "运行时结算", "运行时定义优先");
        assert!(!pages[0].navigable);
        let orders = &pages[2];
        assert!(!orders.declared);
        assert_eq!(orders.tools.keys().collect::<Vec<_>>(), ["orders.cancel", "orders.export"]);
        let (page, t) = find_tool(&pages, "admin.reset").expect("admin.reset");
        assert_eq!((page.name.as_str(), t.page.as_deref()), ("admin", Some("admin")), "清单页面工具补上 page");
        assert!(find_tool(&pages, "info").is_none(), "顶层无 page 的工具不在目录中");
    }

    #[test]
    fn learned_page_moves_and_limits() {
        let mut learned = LearnedPages::default();
        learned.learn("a", [&tool("x", Some("p1"))]);
        learned.learn("a", [&tool("x", Some("p2"))]);
        let pages = catalog(None, &learned);
        assert_eq!(pages.len(), 1, "工具移到新页面，旧页面为空时移除");
        assert_eq!(pages[0].name, "p2");
        // 清单中的页面被运行时上报移走
        let mut learned = LearnedPages::default();
        learned.learn("shop", [&tool("cart.checkout", Some("checkout"))]);
        let pages = catalog(Some(&manifest()), &learned);
        let cart = pages.iter().find(|p| p.name == "cart").expect("cart");
        assert!(cart.tools.is_empty());
        assert!(find_tool(&pages, "cart.checkout").is_some_and(|(p, _)| p.name == "checkout"));

        let mut learned = LearnedPages::default();
        let many: Vec<SharedTool> = (0..MAX_LEARNED_PAGES + 3).map(|i| tool(&format!("t{i}"), Some(&format!("p{i}")))).collect();
        learned.learn("a", many.iter());
        assert_eq!(catalog(None, &learned).len(), MAX_LEARNED_PAGES);
        let mut learned = LearnedPages::default();
        let many: Vec<SharedTool> = (0..MAX_LEARNED_PAGE_TOOLS + 3).map(|i| tool(&format!("t{i}"), Some("p"))).collect();
        learned.learn("a", many.iter());
        assert_eq!(catalog(None, &learned)[0].tools.len(), MAX_LEARNED_PAGE_TOOLS);
        // 非法页面名忽略
        let mut learned = LearnedPages::default();
        assert!(!learned.learn("a", [&tool("t", Some("bad page"))]));
    }
}
