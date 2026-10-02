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
//! 注意：rmcp 3.5 在协议 2026-07-28 中去掉了 `initialize` 与 `resources/subscribe`，
//! 改用每请求 `_meta` 与 `subscriptions/listen`。本 Hub 只声明支持到 2025-11-25（S7 才放开 2026-07-28）；
//! 无会话请求现在只能以 2025-11-25 及以前的版本经 `server/discover` 到达，其列表规则（S5）与订阅（S7）尚未按 modern 改写。

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, InitializeRequestParams, InitializeResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, RequestMetaObject, ReadResourceRequestParams, ReadResourceResponse,
    Resource, ServerCapabilities, ServerConfig, SubscribeRequestParams, UnsubscribeRequestParams,
};
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::{NotificationContext, RequestContext};
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
}

/// 一个请求的调用方。
struct RequestCaller {
    key: CallerKey,
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
        }
    }

    fn is_legacy(&self) -> bool {
        self.handshake.load(Ordering::SeqCst)
    }

    /// 按请求判定调用方（模块文档）。
    ///
    /// @security 主体只取自传输层：`/mcp` 在 rmcp 之前已核对本机令牌（TCP）或由操作系统核对同一用户（IPC），stdio 的对端是
    /// 父进程——现在都是 [`Principal::Local`]；不读 `clientInfo`（自报、不可信）。
    fn caller(&self, meta: &RequestMetaObject) -> RequestCaller {
        let declares_modern = meta.protocol_version().is_some_and(|v| !v.has_initialize());
        if self.is_legacy() && !declares_modern {
            RequestCaller { key: CallerKey::mcp_session(self.id), mcp_session: Some(self.id) }
        } else {
            RequestCaller { key: CallerKey::principal(Principal::Local), mcp_session: None }
        }
    }
}

impl Drop for McpSession {
    /// 只有 legacy 会话有结束信号；无会话请求的处理器析构无副作用（任务按空闲回收）。
    fn drop(&mut self) {
        if !self.is_legacy() {
            return;
        }
        tracing::debug!(cid = %self.cid, "MCP 会话结束");
        self.shared.remove_session(self.id);
        self.shared.end_task(&CallerKey::mcp_session(self.id));
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

    /// rmcp 默认实现（登记 peer 信息 + 协商版本）之外只记下"本连接握手过"（rmcp 文档给出的覆盖方式）。
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.handshake.store(true, Ordering::SeqCst);
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
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
        let caller = self.caller(&context.meta);
        let _activity = self.shared.session_request(&caller.key);
        Ok(ListToolsResult::with_all_items(self.shared.mcp_tools(&caller.key)))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Agent 的截止时间与幂等键（spec/hub-api.md 3.15）：不合法时不执行，以工具错误返回。
        let agent = match request_meta::parse(&context.meta) {
            Ok(m) => m,
            Err(e) => return Ok(call::error_result(&e).into()),
        };
        let caller = self.caller(&context.meta);
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
        let _activity = self.shared.session_request(&self.caller(&context.meta).key);
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
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        call::read_resource(&self.shared, &request.uri, &self.caller(&context.meta).key)
            .await
            .map(Into::into)
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        // rmcp 只把 legacy 请求的 `resources/subscribe` 交到这里（无会话请求得 method not found，订阅改经 S7 的 listen）。
        let _activity = self.shared.session_request(&self.caller(&context.meta).key);
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

/// 第 12 项 S4 / 第 16 项 P1 的验收（docs/plans/12-mcp-stateless.md 第 6 节）：无会话请求经 rmcp 的 `server/discover`
/// 生命周期（`ClientLifecycleMode::Discover`）真实到达本处理器；legacy 客户端走 `initialize`。
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use app_mcp_native::{CallHandle, LifecycleMode, NativeClient, NativeConfig, ToolHandler, ToolSpec};
    use rmcp::model::{CallToolRequestParams, CallToolResult};
    use rmcp::service::{ClientLifecycleMode, RunningService};
    use rmcp::{ClientServiceExt, RoleClient, ServiceExt};
    use serde_json::{Value, json};

    use crate::task::{CallerKey, Principal};
    use crate::{Hub, HubConfig, LeasePolicy};

    const T: Duration = Duration::from_secs(10);
    const INSTANCE: &str = "shop-1";

    struct Echo;
    impl ToolHandler for Echo {
        fn invoke(&self, call: CallHandle) {
            let _ = call.complete(Some(&json!({ "tool": call.tool_name() }).to_string()), vec![]);
        }
    }

    fn config(idle_revoke: Duration, task_idle_ttl: Duration) -> HubConfig {
        HubConfig {
            listen: Some("127.0.0.1:0".into()),
            listen_alternates: Vec::new(),
            ipc_endpoint: None,
            list_changed_debounce: Duration::from_millis(10),
            lease: LeasePolicy { idle_revoke, ..LeasePolicy::default() },
            task_idle_ttl,
            ..Default::default()
        }
    }

    async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + T;
        while !f() {
            assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn start(cfg: HubConfig) -> (Hub, NativeClient) {
        let hub = Hub::start(cfg).await.expect("hub");
        let mut c = NativeConfig::new("shop", "商城");
        c.host_url = format!("ws://{}/app", hub.listen_addr().expect("listen"));
        c.instance_id = Some(INSTANCE.to_owned());
        c.lifecycle.mode = LifecycleMode::Persistent;
        let client = NativeClient::new(c, None).expect("client");
        client.register_tool(ToolSpec::new("cart.add", "加入购物车"), std::sync::Arc::new(Echo)).expect("tool");
        client.start();
        eventually("App 注册工具", || hub.status().apps.iter().any(|a| a.app_id == "shop" && !a.tools.is_empty())).await;
        (hub, client)
    }

    /// 一条 MCP 连接（`hub.mcp_session()` 的一个处理器）。`modern` = 不握手、每请求自带 `_meta`（`server/discover`）。
    async fn connect(hub: &Hub, modern: bool) -> RunningService<RoleClient, ()> {
        let (c, s) = tokio::io::duplex(1 << 20);
        let session = hub.mcp_session();
        tokio::spawn(async move {
            if let Ok(svc) = session.serve(s).await {
                let _ = svc.waiting().await;
            }
        });
        if modern {
            let lifecycle = ClientLifecycleMode::Discover { preferred_versions: vec![rmcp::model::ProtocolVersion::V_2025_11_25] };
            ().serve_with_lifecycle(c, lifecycle).await.expect("discover")
        } else {
            ().serve(c).await.expect("initialize")
        }
    }

    async fn call(agent: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
        let mut params = CallToolRequestParams::new(name.to_owned());
        params.arguments = args.as_object().cloned();
        let r = tokio::time::timeout(T, agent.peer().call_tool(params)).await.expect("MCP 调用超时").expect("call");
        assert_ne!(r.is_error, Some(true), "{name}: {r:?}");
        r
    }

    fn principal() -> CallerKey {
        CallerKey::principal(Principal::Local)
    }

    /// （任务数, 主体任务 ID, 主体任务的选择, 主体任务的租约数）
    fn principal_task(hub: &Hub) -> (usize, Option<String>, Option<String>, usize) {
        let tasks = hub.shared().agent_tasks();
        let t = tasks.get(&principal());
        (
            tasks.iter().count(),
            t.map(|t| t.id.clone()),
            t.and_then(|t| t.selected.get("shop").cloned()),
            t.map_or(0, |t| t.leases.len()),
        )
    }

    fn legacy_keys(hub: &Hub) -> Vec<String> {
        let tasks = hub.shared().agent_tasks();
        tasks.iter().map(|(k, _)| k.to_string()).filter(|k| k.starts_with("mcp:")).collect()
    }

    /// 连续 N 个无会话请求（同一连接与每请求一个新处理器——后者即 rmcp 无状态 HTTP 路径的"每请求调用工厂"）
    /// 只用一个主体任务：不新建、不删除任务，处理器析构无副作用；`apps.select` 与租约记在主体任务上。
    #[tokio::test(flavor = "multi_thread")]
    async fn stateless_requests_share_one_principal_task() {
        let (hub, client) = start(config(Duration::ZERO, Duration::ZERO)).await;
        let agent = connect(&hub, true).await;
        assert_eq!(principal_task(&hub).0, 0, "server/discover 不建任务");

        call(&agent, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
        let (n, id, selected, _) = principal_task(&hub);
        let id = id.expect("主体任务");
        assert_eq!((n, selected.as_deref()), (1, Some(INSTANCE)));
        for i in 0..5 {
            agent.peer().list_all_tools().await.expect("tools/list");
            let r = call(&agent, "shop.cart.add", json!({ "i": i })).await;
            assert_eq!(r.structured_content.as_ref().map(|v| v["tool"].clone()), Some(json!("cart.add")));
            assert_eq!(principal_task(&hub), (1, Some(id.clone()), Some(INSTANCE.to_owned()), 1), "第 {i} 次");
        }
        let _ = agent.cancel().await;
        // 每请求一个新连接 / 处理器，用完即断开
        for i in 0..3 {
            let one = connect(&hub, true).await;
            call(&one, "shop.cart.add", json!({})).await;
            let _ = one.cancel().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(principal_task(&hub), (1, Some(id.clone()), Some(INSTANCE.to_owned()), 1), "第 {i} 个连接");
        }
        // 租约统计也按主体键记（`LeasePairStatus.session`）
        let lease = hub.status().lease.expect("lease");
        assert!(lease.pairs.iter().any(|p| p.session == "principal:local" && p.app_id == "shop"), "{:?}", lease.pairs);
        assert!(lease.pairs.iter().all(|p| !p.session.starts_with("mcp:")), "{:?}", lease.pairs);
        client.stop();
        hub.shutdown().await;
    }

    /// legacy 会话与无会话请求并发：各自的选择、租约互不影响；legacy 会话结束只结束它自己的任务。
    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_and_stateless_do_not_affect_each_other() {
        let (hub, client) = start(config(Duration::ZERO, Duration::ZERO)).await;
        let legacy = connect(&hub, false).await;
        let modern = connect(&hub, true).await;

        let (a, b) = tokio::join!(
            call(&legacy, "shop.cart.add", json!({})),
            call(&modern, "shop.cart.add", json!({})),
        );
        drop((a, b));
        let keys = legacy_keys(&hub);
        assert_eq!(keys.len(), 1, "legacy 会话一个任务：{keys:?}");
        let legacy_key = keys[0].clone();
        let (n, id, selected, leases) = principal_task(&hub);
        assert_eq!((n, selected, leases), (2, None, 1));

        // 只有 legacy 会话选定实例：主体任务不受影响
        call(&legacy, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
        assert_eq!(principal_task(&hub).2, None);
        let selected_in = |r: CallToolResult| r.structured_content.map(|v| v["apps"][0]["selectedInstanceId"].clone());
        assert_eq!(selected_in(call(&legacy, "apps.list", json!({})).await), Some(json!(INSTANCE)));
        assert_eq!(selected_in(call(&modern, "apps.list", json!({})).await), Some(Value::Null));

        // 无会话请求的 apps.release 只收回主体任务的租约，legacy 会话的租约保留
        let r = call(&modern, "apps.release", json!({"appId": "shop"})).await;
        assert_eq!(r.structured_content.as_ref().map(|v| v["released"].clone()), Some(json!(1)));
        assert_eq!(principal_task(&hub).3, 0);
        {
            let tasks = hub.shared().agent_tasks();
            let legacy_task = tasks.iter().find(|(k, _)| k.to_string() == legacy_key).map(|(_, t)| t);
            assert_eq!(legacy_task.map(|t| (t.leases.len(), t.selected.len())), Some((1, 1)));
        }

        // legacy 会话结束：它的任务结束，主体任务仍在（同一 ID）
        let _ = legacy.cancel().await;
        eventually("legacy 会话的任务结束", || legacy_keys(&hub).is_empty()).await;
        assert_eq!(principal_task(&hub).1, id);
        let _ = modern.cancel().await;
        client.stop();
        hub.shutdown().await;
    }

    /// 无会话请求的租约按请求流空闲收回（`lease.idle_revoke`），任务按 `task_idle_ttl` 回收；legacy 会话的任务不按空闲回收。
    #[tokio::test(flavor = "multi_thread")]
    async fn stateless_leases_and_task_reclaimed_by_idle() {
        let (hub, client) = start(config(Duration::from_millis(150), Duration::from_millis(600))).await;
        let legacy = connect(&hub, false).await;
        let modern = connect(&hub, true).await;
        call(&legacy, "shop.cart.add", json!({})).await;
        call(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
        call(&modern, "shop.cart.add", json!({})).await;
        assert_eq!(principal_task(&hub).3, 1);

        // 请求流空闲：两个调用方的默认租约都被收回（各计一次）
        eventually("默认租约空闲收回", || hub.status().lease.is_some_and(|l| l.revoked_idle >= 2)).await;
        // 任务空闲到期：主体任务（选择、租约统计）被回收；legacy 会话的任务保留
        eventually("主体任务回收", || principal_task(&hub).0 == 1).await;
        assert_eq!(principal_task(&hub).1, None);
        assert_eq!(legacy_keys(&hub).len(), 1);
        let lease = hub.status().lease.expect("lease");
        assert!(lease.pairs.iter().all(|p| p.session != "principal:local"), "{:?}", lease.pairs);

        // 之后的无会话请求得到一个新任务，没有旧选择
        call(&modern, "shop.cart.add", json!({})).await;
        let (_, id, selected, _) = principal_task(&hub);
        assert!(id.is_some() && selected.is_none());
        let _ = (legacy.cancel().await, modern.cancel().await);
        client.stop();
        hub.shutdown().await;
    }
}
