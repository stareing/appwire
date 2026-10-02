//! 上游 MCP 服务器的状态维护：连接 / 断开、工具与资源、总览。

#[cfg(any(feature = "mcp-server", feature = "upstream"))]
use rmcp::model::Resource;
use rmcp::model::Tool;
use rmcp::{Peer, RoleClient};

use crate::overview::{Overview, OverviewSource};
use crate::types::HubEvent;
use crate::upstream::UpstreamState;
#[cfg(feature = "mcp-server")]
use crate::upstream::encode_uri_component;

#[cfg(feature = "mcp-server")]
use super::resource_uri;
use super::{HubShared, lock};

impl HubShared {
    // ------------------------------------------------------------------
    // 上游 MCP 服务器
    // ------------------------------------------------------------------

    pub(crate) fn is_upstream(&self, name: &str) -> bool {
        lock(&self.upstreams).contains_key(name)
    }

    /// 上游的 peer：不是上游时为 `None`，未连接时为 `Some(None)`。
    pub(crate) fn upstream_peer(&self, name: &str) -> Option<Option<Peer<RoleClient>>> {
        lock(&self.upstreams).get(name).map(|s| s.peer.clone())
    }

    /// 上游工具的原始定义（名称不含前缀）。
    pub(crate) fn upstream_tool(&self, name: &str, tool: &str) -> Option<Tool> {
        lock(&self.upstreams)
            .get(name)?
            .tools
            .iter()
            .find(|t| t.name == tool)
            .cloned()
    }

    pub(crate) fn upstream_display_name(&self, name: &str) -> String {
        lock(&self.upstreams)
            .get(name)
            .and_then(|s| s.server_name.clone())
            .unwrap_or_else(|| name.to_owned())
    }

    #[cfg(feature = "upstream")]
    pub(crate) fn upstream_connected(
        &self,
        name: &str,
        peer: Peer<RoleClient>,
        tools: Vec<Tool>,
        resources: Vec<Resource>,
        instructions: Option<String>,
        server_name: Option<String>,
    ) {
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            st.peer = Some(peer);
            st.tools = tools;
            st.resources = resources;
            st.instructions = instructions;
            st.server_name = server_name;
            st.last_error = None;
        }
        self.emit(HubEvent::UpstreamState {
            name: name.to_owned(),
            connected: true,
            error: None,
        });
        self.mark_tools_changed();
        self.mark_resources_changed();
    }

    pub(crate) fn upstream_disconnected(&self, name: &str, error: Option<String>) {
        let mut last_error = None;
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            let was_connected = st.connected();
            st.peer = None;
            st.tools.clear();
            st.resources.clear();
            st.restarts += 1;
            if error.is_some() {
                st.last_error = error;
            } else if was_connected {
                st.last_error = Some("进程已退出".into());
            }
            last_error = st.last_error.clone();
        }
        self.emit(HubEvent::UpstreamState {
            name: name.to_owned(),
            connected: false,
            error: last_error,
        });
        self.mark_tools_changed();
        self.mark_resources_changed();
    }

    #[cfg(feature = "upstream")]
    pub(crate) fn set_upstream_tools(&self, name: &str, tools: Vec<Tool>) {
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            st.tools = tools;
        }
        self.mark_tools_changed();
    }

    #[cfg(feature = "upstream")]
    pub(crate) fn set_upstream_resources(&self, name: &str, resources: Vec<Resource>) {
        if let Some(st) = lock(&self.upstreams).get_mut(name) {
            st.resources = resources;
        }
        self.mark_resources_changed();
    }

    /// 已连接上游的资源，URI 改为 `app-mcp://<name>/<编码后的上游 URI>`。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn upstream_resources(&self) -> Vec<Resource> {
        let policy = self.policy();
        let ups = lock(&self.upstreams);
        let mut out = Vec::new();
        for (name, st) in ups.iter().filter(|(name, _)| policy.app_hidden(name).is_none()) {
            for r in &st.resources {
                let mut r = r.clone();
                r.uri = resource_uri(name, &encode_uri_component(&r.uri));
                r.name = format!("{name}.{}", r.name);
                out.push(r);
            }
        }
        out
    }

}

/// 上游的总览：取 `instructions` 的前 100 个字符为简介，其余（≤ 2000）为正文。
pub(super) fn upstream_overview(name: &str, st: &UpstreamState) -> Option<Overview> {
    let text = st.instructions.as_deref()?.trim();
    let summary: String = text
        .chars()
        .take(app_mcp_protocol::OVERVIEW_SUMMARY_MAX_CHARS)
        .collect();
    let rest: String = text
        .chars()
        .skip(app_mcp_protocol::OVERVIEW_SUMMARY_MAX_CHARS)
        .collect();
    let raw = app_mcp_protocol::AppOverview {
        summary,
        body: Some(rest.trim().to_owned()).filter(|b| !b.is_empty()),
        locale: None,
    };
    let display = st.server_name.as_deref().unwrap_or(name);
    Overview::new(name, display, &raw, OverviewSource::Upstream)
}
