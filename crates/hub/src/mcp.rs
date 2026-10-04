//! MCP 出口：手动实现 rmcp 的 [`ServerHandler`]。
//!
//! 每个请求按**调用方**处理（docs/plans/12-mcp-stateless.md 3.1，S4）；调用方的跨请求状态（`apps.select` 的选择、
//! “已附带的总览版本”、租约、渐进暴露已展开的 App）记在其 Agent 任务上（[`crate::task`]）：
//!
//! - **legacy**：本 [`McpSession`] 处理过 `initialize`（stdio / IPC 流的一条连接，或 HTTP 的一个 `Mcp-Session-Id`）
//!   → 调用方键 `mcp:<id>`，会话结束（Drop）时结束任务、移除 peer 与订阅。行为与拆分前相同。
//! - **无会话（stateless）**：请求不经 `initialize`、自带协议 `_meta`（rmcp 的 `server/discover` 生命周期，HTTP / IPC / stdio
//!   一律如此；或请求 `_meta` 声明 2026-07-28 及以后的版本）→ 调用方键 `principal:<主体>`，不登记 peer、不建会话状态、
//!   Drop 无副作用；任务按请求流空闲回收（`HubConfig::task_idle_ttl`）。rmcp 的无状态 HTTP 路径每请求调用一次工厂，
//!   因此构造与 Drop 都不能有副作用（12 迁移计划 F9）。
//!
//! 工具调用与资源读取都交给 [`crate::call`]，与 Hub API 共用一份逻辑。
//!
//! 任务句柄（第 12 项 S8，spec/hub-api.md 3.6「任务句柄」）：`tools/call` 请求 `_meta` 的 `dev.appwire/taskId` 经 `CallCtx::task_id`
//! 交给 `crate::task_handle`，与参数 `taskId` 一并解析为调用方。
//!
//! 无会话请求的列表与总览（S5，spec/hub-api.md 3.7）：`tools/list` 只取决于服务器状态、主体与配置（不读任务上的展开记录与
//! `apps.select`），默认全部列出（`HubConfig::stateless_tool_exposure`）；列表结果带 `ttlMs` / `cacheScope: private`；
//! 总览不在调用结果中首次附带，改经 `server/discover` 的 `instructions`、`apps.tools` 与 `apps.overview`。
//!
//! 协议版本（S7，spec/hub-api.md 3.6「协议版本」）：默认（[`McpProtocolMode::Auto`]）声明到 2026-07-28——`initialize` 仍只协商
//! 带握手的版本（至多 2025-11-25，rmcp `LATEST_WITH_INITIALIZE`），每请求自带 `_meta` 的客户端可用 2026-07-28；
//! [`McpProtocolMode::LegacyOnly`] 只声明到 2025-11-25 并关闭 `subscriptions/listen`（S7 之前的行为）。
//!
//! 通知（S7）：legacy 会话照旧经会话 peer 收 `list_changed` 与 `resources/subscribe` 订阅的变化；无会话请求经
//! `subscriptions/listen` 流订阅（[`crate::subscribers`]），流的寿命即订阅寿命，每主体流数有上限。modern 请求的 JSON-RPC
//! 错误不发 AppWire 的 -32000…-32019 码（[`error_for_version`]）。

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use rmcp::model::{
    CacheScope, CallToolRequestParams, ErrorCode, SubscriptionFilter, CallToolResponse, DiscoverResult, Implementation, InitializeRequestParams, InitializeResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, Resource, ServerCapabilities, ServerConfig, SubscribeRequestParams,
    UnsubscribeRequestParams,
};
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::{NotificationContext, RequestContext, SubscriptionContext};
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler};
use serde_json::Value;

pub use crate::call::{
    TOOL_APPS_LIST, TOOL_APPS_OVERVIEW, TOOL_APPS_SELECT, TOOL_APPS_TOOLS, UNAVAILABLE_PREFIX,
};
use crate::call::{self, CallCtx, to_mcp_error};
use crate::hub::{DEFAULT_MIME, HubShared, parse_resource_uri, resource_uri};
use crate::overview;
use crate::progress::ProgressUpdate;
use crate::request_meta;
use crate::task::{CallerKey, Principal};
use crate::types::McpProtocolMode;
use app_mcp_protocol::ErrorKind;

/// 一个 MCP 连接（或 rmcp 无状态 HTTP 路径的一个请求）的处理器。
pub struct McpSession {
    shared: Arc<HubShared>,
    id: u64,
    /// 会话 ID（日志 `cid` 字段，spec/protocol.md 10.3）：`mcp-<序号>`。
    cid: String,
    /// 处理过 `initialize`：之后的请求（未声明 2026-07-28 及以后版本时）属于 legacy 会话 `mcp:<id>`。
    ///
    /// @why 在 `initialize` 而不是 `notifications/initialized` 时置位：rmcp 不等 `initialized` 就处理后续请求
    /// （HTTP 上两者是独立的 POST，无先后保证），按后者判定会把握手后的首批请求误判为无会话。
    handshake: AtomicBool,
    /// legacy 会话建立（`initialize`）时请求的认证主体：会话内的请求沿用它（第 16 项 N5）。
    session_principal: OnceLock<Principal>,
}

/// 一个请求的调用方。
struct RequestCaller {
    key: CallerKey,
    /// 认证主体（`ApprovalRequest::principal`）。
    principal: Principal,
    /// legacy 会话号（只向该会话发 `list_changed`）；无会话请求为 `None`。
    mcp_session: Option<u64>,
}

impl McpSession {
    pub(crate) fn new(shared: Arc<HubShared>) -> Self {
        let id = shared.next_id();
        Self {
            shared,
            id,
            cid: format!("mcp-{id}"),
            handshake: AtomicBool::new(false),
            session_principal: OnceLock::new(),
        }
    }

    fn is_legacy(&self) -> bool {
        self.handshake.load(Ordering::SeqCst)
    }

    /// 无会话请求的列表结果的 `ttlMs`（SEP-2549，12 迁移计划 m5；`cacheScope` 恒为 `private`：列表含本机 App 信息，
    /// 不应被共享中间层缓存）。legacy 会话为 `None`：结果不带这两个字段，线上格式不变。
    fn list_ttl_ms(&self, caller: &RequestCaller) -> Option<u64> {
        caller.key.is_stateless().then(|| millis(self.shared.config.stateless_list_ttl))
    }

    /// 服务器信息（`initialize` 与 `server/discover` 共用），`instructions` 由调用方按两代给出。
    fn server_config(&self, instructions: String) -> ServerConfig {
        #[cfg_attr(not(feature = "upstream"), allow(unused_mut))]
        let mut capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_resources_subscribe()
            .build();
        // MCP Apps：有已连接上游声明该扩展时才声明（Hub 只透传上游界面，自身不提供，spec/hub-api.md 3.22）
        #[cfg(feature = "upstream")]
        if self.shared.upstreams_declare_mcp_apps() {
            capabilities.extensions = Some(crate::upstream::ui::mcp_apps_extension());
        }
        ServerConfig::new(capabilities)
            .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE)
            .with_server_info(Implementation::new(
                "app-mcp-host",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(instructions)
    }

    /// 本 legacy 会话的调用方键（带建立会话时的 Agent 身份）。
    fn session_key(&self) -> CallerKey {
        CallerKey::mcp_session(self.id, self.session_principal.get().and_then(Principal::agent).cloned())
    }

    /// 按请求判定调用方（模块文档）。
    ///
    /// @security 主体只取自传输层（[`request_principal`]）；不读 `clientInfo`（自报、不可信）。
    fn caller(&self, context: &RequestContext<RoleServer>) -> RequestCaller {
        let declares_modern = context.meta.protocol_version().is_some_and(|v| !v.has_initialize());
        if self.is_legacy() && !declares_modern {
            let principal = self.session_principal.get().cloned().unwrap_or(Principal::Local);
            RequestCaller { key: self.session_key(), principal, mcp_session: Some(self.id) }
        } else {
            let principal = request_principal(context);
            RequestCaller { key: CallerKey::principal(&principal), principal, mcp_session: None }
        }
    }
}

/// 请求的认证主体：`/mcp` 核对令牌后放进 HTTP 请求扩展（`http_server`，rmcp 把 `http::request::Parts` 放进请求上下文）；
/// 出示已登记 Agent 令牌 → [`Principal::Agent`]。其余——本机令牌 / 回环 / IPC 同用户 / stdio 与流传输（没有 HTTP 部分）——
/// 都是 [`Principal::Local`]。
fn request_principal(context: &RequestContext<RoleServer>) -> Principal {
    context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Principal>())
        .cloned()
        .unwrap_or(Principal::Local)
}

/// MCP 2026-07-28 及以后的请求：AppWire 自定义的 -32000…-32019 码改为规范码，类别仍在 `data.kind`
/// （docs/plans/12-mcp-stateless.md 第 5 节 S3：该区为 legacy，新实现不应使用，接收方不得假定含义）。
/// 输入 / 资源不存在类 → `-32602`，其余 → `-32603`。更早的版本（legacy 会话、以旧版本经 `server/discover` 到达）原样返回。
///
/// @compat legacy 线上错误码不变（E-06）；rmcp 自己把 modern 的 `-32002` 改为 `-32602`，这里与之一致。
fn error_for_version(mut e: McpError, version: Option<&ProtocolVersion>) -> McpError {
    const LEGACY_RANGE: std::ops::RangeInclusive<i32> = -32019..=-32000;
    if version.is_none_or(ProtocolVersion::has_initialize) || !LEGACY_RANGE.contains(&e.code.0) {
        return e;
    }
    let kind = e.data.as_ref().and_then(|d| d.get("kind")).and_then(|k| serde_json::from_value::<ErrorKind>(k.clone()).ok());
    e.code = match kind {
        Some(ErrorKind::InvalidInput | ErrorKind::ResourceNotFound) => ErrorCode::INVALID_PARAMS,
        _ => ErrorCode::INTERNAL_ERROR,
    };
    e
}

/// [`McpProtocolMode::Auto`] 声明的最高协议版本（第 12 项 S7 适配并验证过的版本）。
const MAX_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// App 声明的缓存范围 → MCP `cacheScope`（spec/hub-api.md 3.20）：shared → public，private → private。
fn mcp_cache_scope(scope: app_mcp_protocol::CacheScope) -> CacheScope {
    match scope {
        app_mcp_protocol::CacheScope::Shared => CacheScope::Public,
        app_mcp_protocol::CacheScope::Private => CacheScope::Private,
    }
}

fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl McpSession {
    /// listen 流可订阅的资源（[`ServerHandler::accepted_subscription_filter`]）：与 legacy `resources/subscribe` 的规则相同。
    fn listen_subscribable(&self, uri: &str) -> bool {
        if uri == crate::names::RESOURCE_APPS_EVENTS_URI {
            return true;
        }
        !crate::hub_state::is_hub_state_uri(uri)
            && parse_resource_uri(uri).is_some_and(|(app_id, _)| !self.shared.is_upstream(app_id) && !self.shared.app_hidden(app_id))
    }
}

impl Drop for McpSession {
    /// 只有 legacy 会话有结束信号；无会话请求的处理器析构无副作用（任务按空闲回收）。
    fn drop(&mut self) {
        if !self.is_legacy() {
            return;
        }
        tracing::debug!(cid = %self.cid, "MCP 会话结束");
        self.shared.remove_subscriber(self.id);
        self.shared.end_task(&self.session_key());
    }
}

/// MCP 请求带 `progressToken` 时：合并后的进度逐条以 `notifications/progress` 发给该客户端（spec/hub-api.md 3.12）。
/// 转发任务在调用结束（出口被丢弃）后退出。
fn forward_progress(peer: Peer<RoleServer>, token: ProgressToken) -> call::ProgressSink {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressUpdate>();
    tokio::spawn(async move {
        while let Some(u) = rx.recv().await {
            let mut param = ProgressNotificationParam::new(token.clone(), u.progress);
            param.total = u.total;
            param.message = u.message;
            if let Err(e) = peer.notify_progress(param).await {
                tracing::debug!(error = %e, "进度通知发送失败（MCP 客户端可能已断开）");
                break;
            }
        }
    });
    tx
}

#[allow(deprecated)] // subscribe / unsubscribe 在 rmcp 中标记为仅旧协议可用：legacy 会话仍用它们，无会话请求用 listen（见模块文档）
impl ServerHandler for McpSession {
    /// `initialize`（legacy）的服务器信息：`instructions` 含"首次附带"与本会话渐进暴露的说明（spec/protocol.md 7.2）。
    fn get_info(&self) -> ServerConfig {
        self.server_config(overview::instructions(&self.shared.summaries(), self.shared.progressive()))
    }

    /// 声明的版本（`server/discover` 的 `supportedVersions`，也约束 `initialize` 与每请求版本校验）。
    ///
    /// @why 上限写明 2026-07-28 而不用 rmcp 的 `LATEST`：rmcp 小版本升级加入更新的协议版本时，不能未经适配就对外声明。
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        let max = match self.shared.config.mcp_protocol_mode {
            McpProtocolMode::Auto => &MAX_PROTOCOL_VERSION,
            McpProtocolMode::LegacyOnly => &ProtocolVersion::LATEST_WITH_INITIALIZE,
        };
        Cow::Borrowed(ProtocolVersion::known_up_to(max))
    }

    /// rmcp 默认实现（登记 peer 信息 + 协商版本）之外只记下"本连接握手过"（rmcp 文档给出的覆盖方式）。
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.handshake.store(true, Ordering::SeqCst);
        // 重复的 initialize 不改变会话身份（OnceLock 只取第一次）。
        let _ = self.session_principal.set(request_principal(&context));
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    /// `server/discover`（只有无会话请求会发）：`instructions` 按无会话语义给出 App 简介与获取总览的方式（S5）；带缓存提示。
    async fn discover(&self, context: RequestContext<RoleServer>) -> Result<DiscoverResult, McpError> {
        let client = context.client_info().map(|i| format!("{} {}", i.name, i.version));
        let version = context.protocol_version().map(|v| v.to_string());
        tracing::info!(cid = %self.cid, ?client, ?version, "MCP server/discover");
        let instructions = overview::stateless_instructions(&self.shared.summaries(), self.shared.stateless_progressive());
        Ok(DiscoverResult::from_server_info(self.supported_protocol_versions().into_owned(), self.server_config(instructions))
            .with_ttl_ms(millis(self.shared.config.stateless_list_ttl))
            .with_cache_scope(CacheScope::Private))
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let client = context.peer.peer_info().map(|i| i.client_info.name.clone());
        tracing::info!(cid = %self.cid, ?client, "MCP 客户端已初始化");
        self.shared.register_session(self.id, context.peer);
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let caller = self.caller(&context);
        let _activity = self.shared.session_request(&caller.key);
        let r = ListToolsResult::with_all_items(self.shared.mcp_tools(&caller.key));
        Ok(match self.list_ttl_ms(&caller) {
            Some(ttl) => r.with_ttl_ms(ttl).with_cache_scope(CacheScope::Private),
            None => r,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Agent 的截止时间、幂等键（spec/hub-api.md 3.15）与任务句柄（3.6）：不合法时不执行，以工具错误返回。
        let agent = match request_meta::parse(&context.meta) {
            Ok(m) => m,
            Err(e) => return Ok(call::error_result(&e).into()),
        };
        let caller = self.caller(&context);
        let progress = context.meta.get_progress_token().map(|token| forward_progress(context.peer.clone(), token));
        let ctx = CallCtx {
            name: request.name.to_string(),
            arguments: Value::Object(request.arguments.unwrap_or_default()),
            session: Some(caller.key.to_string()),
            caller: caller.key,
            instance_id: None,
            timeout: request_meta::effective_timeout(agent.timeout, self.shared.config.response_timeout),
            call_id: None,
            mcp_session: caller.mcp_session,
            progress,
            idempotency_key: agent.idempotency_key,
            principal: Some(caller.principal.label()),
            client_name: context.client_info().map(|i| i.name),
            task_id: agent.task_id,
            priority: agent.priority,
            cache_bypass: agent.cache_bypass,
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
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let caller = self.caller(&context);
        let _activity = self.shared.session_request(&caller.key);
        let policy = self.shared.policy();
        let resources = self
            .shared
            .registry()
            .resources()
            .into_iter()
            .filter(|r| policy.app_hidden(&r.app_id).is_none())
            .map(|r| {
                let desc = if r.available {
                    r.info.description.clone()
                } else {
                    format!("{UNAVAILABLE_PREFIX}{}（App 未连接）", r.info.description)
                };
                let resource = Resource::new(
                    resource_uri(&r.app_id, &r.info.name),
                    format!("{}.{}", r.app_id, r.info.name),
                )
                .with_description(desc)
                .with_mime_type(
                    r.info
                        .mime_type
                        .clone()
                        .unwrap_or_else(|| DEFAULT_MIME.to_owned()),
                );
                // App 对资源内容的标注原样转发（spec/protocol.md 3.2）。
                match &r.info.annotations {
                    Some(a) => resource.with_annotations(crate::mcp_convert::content_annotations(a)),
                    None => resource,
                }
            })
            .chain(self.shared.upstream_resources())
            .chain(crate::hub_state::hub_state_resources())
            .chain(std::iter::once(crate::events::events_self_resource()))
            .collect();
        let r = ListResourcesResult::with_all_items(resources);
        Ok(match self.list_ttl_ms(&caller) {
            Some(ttl) => r.with_ttl_ms(ttl).with_cache_scope(CacheScope::Private),
            None => r,
        })
    }

    /// 本 Hub 没有资源模板。无会话请求的结果带缓存提示（12 迁移计划 m5），legacy 同 rmcp 默认实现。
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let r = ListResourceTemplatesResult::default();
        Ok(match self.list_ttl_ms(&self.caller(&context)) {
            Some(ttl) => r.with_ttl_ms(ttl).with_cache_scope(CacheScope::Private),
            None => r,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let caller = self.caller(&context);
        let version = context.protocol_version();
        let bypass = request_meta::parse_cache_bypass(&context.meta)
            .map_err(|e| error_for_version(call::to_mcp_error(&e), version.as_ref()))?;
        let (r, hint) = call::read_resource(&self.shared, &request.uri, &caller.key, bypass)
            .await
            .map_err(|e| error_for_version(e, version.as_ref()))?;
        // 资源内容随 App 状态随时变化：无会话请求的结果默认标为立即过期（`ttlMs: 0`）、`private`；App 声明了 `cache` 的
        // 资源按其剩余有效期与范围（spec/hub-api.md 3.20）。legacy 会话的结果不带这两个字段（3.7）。
        if !caller.key.is_stateless() {
            return Ok(r.into());
        }
        Ok(match hint {
            Some(h) => r.with_ttl_ms(h.ttl_ms).with_cache_scope(mcp_cache_scope(h.scope)),
            None => r.with_ttl_ms(0).with_cache_scope(CacheScope::Private),
        }
        .into())
    }

    /// `subscriptions/listen` 接受的类别：工具 / 资源列表变化，以及可订阅的资源 URI（本 Hub 的 `app-mcp://` 资源、App 未被
    /// `hide`、不是上游资源；去重后至多 `max_listen_resources` 个）。回退开关或 `max_listen_streams = 0` 时不提供（`None`）。
    fn accepted_subscription_filter(&self, requested: &SubscriptionFilter) -> Option<SubscriptionFilter> {
        let config = &self.shared.config;
        if config.mcp_protocol_mode == McpProtocolMode::LegacyOnly || config.max_listen_streams == 0 {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        let uris: Vec<String> = requested
            .resource_subscriptions
            .iter()
            .flatten()
            .filter(|uri| self.listen_subscribable(uri) && seen.insert(uri.as_str()))
            .take(config.max_listen_resources)
            .cloned()
            .collect();
        let filter = SubscriptionFilter::builder().tools_list_changed().resources_list_changed();
        Some(match uris.is_empty() {
            true => filter.build(),
            false => filter.resource_subscriptions(uris).build(),
        })
    }

    /// 一个 listen 流：登记为订阅方直到客户端关闭流（取消）或 Hub 停止（返回 `Ok` → rmcp 发最终结果）。
    /// 不计入请求流活动（[`HubShared::session_request`]）：长期打开的流不应阻止租约与任务的空闲回收。
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let version = context.request_context().protocol_version();
        let _registration = self
            .shared
            .open_listen(context.sink().clone(), request_principal(context.request_context()))
            .map_err(|e| error_for_version(to_mcp_error(&e), version.as_ref()))?;
        tracing::info!(cid = %self.cid, subscription = %context.sink().id(), accepted = ?context.accepted(), "subscriptions/listen 已建立");
        tokio::select! {
            () = context.cancelled() => tracing::debug!(cid = %self.cid, "listen 流已由客户端关闭"),
            () = self.shared.closing() => tracing::debug!(cid = %self.cid, "Hub 停止，结束 listen 流"),
        }
        Ok(())
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        // rmcp 只把 legacy 请求的 `resources/subscribe` 交到这里（无会话请求得 method not found，订阅改经 S7 的 listen）。
        let caller = self.caller(&context);
        let _activity = self.shared.session_request(&caller.key);
        if let Some((app_id, _)) = parse_resource_uri(&request.uri)
            && self.shared.is_upstream(app_id)
        {
            return Err(McpError::invalid_params(
                "上游 MCP 服务器的资源暂不支持订阅，请直接读取。",
                None,
            ));
        }
        self.shared
            .subscribe(self.id, &request.uri, &caller.key)
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

/// 第 12 项 S4 / 第 16 项 P1 的验收（docs/plans/12-mcp-stateless.md 第 6 节）：无会话请求经 rmcp 的 `server/discover`
/// 生命周期（`ClientLifecycleMode::Discover`）真实到达本处理器；legacy 客户端走 `initialize`。
#[cfg(test)]
mod tests;
