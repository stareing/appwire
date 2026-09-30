//! MCP 出口：手动实现 rmcp 的 [`ServerHandler`]。
//!
//! 每个 MCP 连接一个 [`McpSession`]：`apps.select` 的选择、“已附带的总览版本”与渐进暴露已展开的 App
//! 按会话保存（会话键 `mcp:<id>`）。工具调用与资源读取都交给 [`crate::call`]，与 Hub API 共用一份逻辑。
//!
//! 注意：rmcp 3.5 在协议 2026-07-28 中去掉了 `initialize` 与 `resources/subscribe`，
//! 改用每请求 `_meta` 与 `subscriptions/listen`。本 Hub 只声明支持到 2025-11-25，
//! 从而始终使用会话模式（`initialize` + `notifications/initialized` + 旧式订阅）。

use std::borrow::Cow;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    Resource, ServerCapabilities, ServerConfig, SubscribeRequestParams, UnsubscribeRequestParams,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::Value;

pub use crate::call::{
    TOOL_APPS_LIST, TOOL_APPS_OVERVIEW, TOOL_APPS_SELECT, TOOL_APPS_TOOLS, UNAVAILABLE_PREFIX,
};
use crate::call::{self, CallCtx, to_mcp_error};
use crate::hub::{DEFAULT_MIME, HubShared, parse_resource_uri, resource_uri};
use crate::overview;

/// 一个 MCP 会话。
pub struct McpSession {
    shared: Arc<HubShared>,
    id: u64,
    key: String,
}

impl McpSession {
    pub(crate) fn new(shared: Arc<HubShared>) -> Self {
        let id = shared.next_id();
        Self {
            shared,
            id,
            key: format!("mcp:{id}"),
        }
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        self.shared.remove_session(self.id);
        self.shared.drop_session_state(&self.key);
    }
}

#[allow(deprecated)] // subscribe / unsubscribe 在 rmcp 中标记为仅旧协议可用，本 Hub 只使用旧协议（见模块文档）
impl ServerHandler for McpSession {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_resources_subscribe()
            .build();
        let summaries = self.shared.summaries();
        ServerConfig::new(capabilities)
            .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
            .with_server_info(Implementation::new(
                "app-mcp-host",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(overview::instructions(&summaries, self.shared.progressive()))
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(
            &ProtocolVersion::LATEST_WITH_INITIALIZE,
        ))
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        tracing::info!(session = self.id, "MCP 客户端已初始化");
        self.shared.register_session(self.id, context.peer);
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.shared.mcp_tools(&self.key)))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let ctx = CallCtx {
            name: request.name.to_string(),
            arguments: Value::Object(request.arguments.unwrap_or_default()),
            session_key: self.key.clone(),
            session: Some(self.key.clone()),
            instance_id: None,
            timeout: None,
            call_id: None,
            mcp_session: Some(self.id),
        };
        let ct = context.ct.clone();
        let inv = self
            .shared
            .call(ctx, async move { ct.cancelled().await })
            .await;
        inv.to_mcp().map(Into::into)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = self
            .shared
            .registry()
            .resources()
            .into_iter()
            .map(|r| {
                let desc = if r.available {
                    r.info.description.clone()
                } else {
                    format!("{UNAVAILABLE_PREFIX}{}（App 未连接）", r.info.description)
                };
                Resource::new(
                    resource_uri(&r.app_id, &r.info.name),
                    format!("{}.{}", r.app_id, r.info.name),
                )
                .with_description(desc)
                .with_mime_type(
                    r.info
                        .mime_type
                        .clone()
                        .unwrap_or_else(|| DEFAULT_MIME.to_owned()),
                )
            })
            .chain(self.shared.upstream_resources())
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        call::read_resource(&self.shared, &request.uri, &self.key)
            .await
            .map(Into::into)
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if let Some((app_id, _)) = parse_resource_uri(&request.uri)
            && self.shared.is_upstream(app_id)
        {
            return Err(McpError::invalid_params(
                "上游 MCP 服务器的资源暂不支持订阅，请直接读取。",
                None,
            ));
        }
        self.shared
            .subscribe(self.id, &request.uri)
            .map_err(|e| to_mcp_error(&e))
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.shared.unsubscribe(self.id, &request.uri);
        Ok(())
    }
}
