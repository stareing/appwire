//! `Hub` 的公开 API（spec/hub-api.md 第 3 节）：查询、调用、订阅、回调设置与对外出口。

use std::net::SocketAddr;
use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::call::{self, CallCtx};
use crate::format::{self, ToolFormat};
use crate::http_server::{HttpOptions, Router, Transport};
#[cfg(feature = "mcp-server")]
use crate::mcp::McpSession;
use crate::policy::{PolicyConfig, PolicyStatus};
use crate::task::CallerKey;
use crate::types::{
    AppInfo, AppKind, AppOverviewInfo, ApprovalHandler, CallOutcome, CallRequest, HubError, HubEvent, HubResource,
    HubStatus, HubTool, PairingHandler, ResourceContent, ToolFilter,
};
use crate::upstream::encode_uri_component;
use crate::wake::Waker;

use super::{API_SUBSCRIBER, Hub, LocalAppChannel, lock, overview_info, parse_resource_uri, resource_uri};
use super::upstreams::upstream_overview;

impl Hub {
    // ---- 查询 ----

    /// 所有已知 App（静态清单、已连接实例）与上游（`kind = Upstream`）。
    pub fn apps(&self) -> Vec<AppInfo> {
        let selected = lock(&self.shared.global_selected).clone();
        let mut out = self.shared.registry().app_infos(&selected);
        out.extend(lock(&self.shared.upstreams).iter().map(|(name, st)| AppInfo {
            app_id: name.clone(),
            name: st.server_name.clone().unwrap_or_else(|| name.clone()),
            kind: AppKind::Upstream,
            summary: upstream_overview(name, st).map(|o| o.summary),
            connected: st.connected(),
            instances: Vec::new(),
            selected_instance: None,
            dormant_instances: Vec::new(),
        }));
        let policy = self.shared.policy();
        out.retain(|a| policy.app_hidden(&a.app_id).is_none());
        out
    }

    /// 工具列表。渐进暴露生效（spec/hub-api.md 3.7）且 `filter.apps` 为 `None` 时，只含内置工具
    /// （此时另有 `apps.tools`）与 `filter.session` 会话已展开 / 调用过 / 选定了实例的 App 的工具；
    /// 显式给出 `filter.apps` 时列出这些 App 的全部工具。
    pub fn tools(&self, filter: &ToolFilter) -> Vec<HubTool> {
        let exposed = self.shared.exposed_apps(&CallerKey::api(filter.session.as_deref()));
        let progressive = exposed.is_some();
        let exposed = exposed.filter(|_| filter.apps.is_none());
        let wanted = |app: &str| {
            filter.apps.as_ref().is_none_or(|a| a.iter().any(|x| x == app)) && exposed.as_ref().is_none_or(|e| e.contains(app))
        };
        self.shared
            .visible_tools(progressive, wanted)
            .into_iter()
            .filter(|(t, builtin)| filter.accepts(t, *builtin))
            .filter(|(t, builtin)| *builtin || exposed.as_ref().is_none_or(|e| e.contains(&t.app_id)))
            .map(|(t, _)| t)
            .collect()
    }

    pub fn resources(&self) -> Vec<HubResource> {
        let mut out: Vec<HubResource> = self
            .shared
            .registry()
            .resources()
            .into_iter()
            .map(|r| HubResource {
                uri: resource_uri(&r.app_id, &r.info.name),
                name: format!("{}.{}", r.app_id, r.info.name),
                app_id: r.app_id,
                description: r.info.description,
                mime_type: r.info.mime_type,
                available: r.available,
                annotations: r.info.annotations,
            })
            .collect();
        for (name, st) in lock(&self.shared.upstreams).iter() {
            for r in &st.resources {
                out.push(HubResource {
                    uri: resource_uri(name, &encode_uri_component(&r.uri)),
                    name: format!("{name}.{}", r.name),
                    app_id: name.clone(),
                    description: r.description.clone().unwrap_or_default(),
                    mime_type: r.mime_type.clone(),
                    available: true,
                    annotations: r.annotations.as_ref().map(crate::mcp_convert::from_mcp_content_annotations),
                });
            }
        }
        let policy = self.shared.policy();
        out.retain(|r| policy.app_hidden(&r.app_id).is_none());
        out
    }

    pub fn overview(&self, app_id: &str) -> Option<AppOverviewInfo> {
        self.shared.overview(app_id).as_ref().map(overview_info)
    }

    /// 运行状态：身份、监听位置、令牌策略、各 App 与实例的状态（在线 / 休眠 / 唤醒中）、最近错误与 SDK 诊断上报
    /// （spec/hub-api.md 3.9）。`GET /status` 返回同样的内容。
    pub fn status(&self) -> HubStatus {
        self.shared.status()
    }

    /// 替换策略规则（spec/hub-api.md 3.13）。规则不合法时返回 `INVALID_INPUT`，之前的规则继续生效（错误记入
    /// `status().policy.last_error`）；成功后命中计数清零，并按工具 / 资源列表变化通知所有会话。
    pub fn set_policy(&self, config: PolicyConfig) -> Result<(), HubError> {
        self.shared
            .set_policy(config)
            .map_err(|e| HubError(ToolError::new(ErrorKind::InvalidInput, e)))
    }

    /// 替换已登记的 Agent（第 16 项 N5，spec/hub-api.md 3.6「Agent 身份」）。不合法时返回 `INVALID_INPUT`，之前的登记继续生效。
    /// 只影响之后到达的请求：已建立的 legacy 会话保持建立时的身份，被移除 Agent 的任务按空闲回收。
    pub fn set_agents(&self, config: crate::agents::AgentsConfig) -> Result<(), HubError> {
        self.shared.set_agents(&config).map_err(|e| HubError(ToolError::new(ErrorKind::InvalidInput, e)))
    }

    /// 生效的策略规则、各规则命中次数与最近的加载错误。
    pub fn policy(&self) -> PolicyStatus {
        lock(&self.shared.policy).status()
    }

    // ---- 操作 ----

    /// 调用工具（与 MCP 出口同一实现：schema 校验 → 审批 → 路由 → 转发）。
    ///
    /// 工具执行层面的失败（参数不合法、用户拒绝、超时、App 报错等）在 `Ok(CallOutcome)` 的
    /// `result` 中返回；只有名称无法解析（格式不对或 appId 未知）时返回 `Err`。
    pub async fn call_tool(&self, req: CallRequest) -> Result<CallOutcome, HubError> {
        let ctx = CallCtx::from_request(req);
        self.shared
            .call(ctx, std::future::pending())
            .await
            .into_outcome()
    }

    /// 与 [`Hub::call_tool`] 相同，并接收调用进度（spec/hub-api.md 3.12）：App 报告的进度按
    /// [`HubConfig::progress_interval`](super::HubConfig::progress_interval) 合并、丢弃不递增的值后逐条发到 `progress`；调用结束后不再发送。
    pub async fn call_tool_with_progress(
        &self,
        req: CallRequest,
        progress: tokio::sync::mpsc::UnboundedSender<crate::ProgressUpdate>,
    ) -> Result<CallOutcome, HubError> {
        let mut ctx = CallCtx::from_request(req);
        ctx.progress = Some(progress);
        self.shared
            .call(ctx, std::future::pending())
            .await
            .into_outcome()
    }

    /// 取消进行中的调用（含等待审批中的）；未知 callId 忽略。
    pub fn cancel_call(&self, call_id: &str) {
        if let Some((_, tx)) = lock(&self.shared.calls).remove(call_id) {
            let _ = tx.send(());
        }
    }

    /// 读取资源（`app-mcp://<appId>/<name>`）。
    pub async fn read_resource(&self, uri: &str) -> Result<ResourceContent, HubError> {
        call::read_resource(&self.shared, uri, &CallerKey::api(None))
            .await
            .map_err(|e| HubError(call::mcp_resource_error_to_tool(&e)))
            .map(|r| call::first_content(uri, r))
    }

    /// 订阅资源变化，之后收到 [`HubEvent::ResourceUpdated`]。
    pub fn subscribe(&self, uri: &str) -> Result<(), HubError> {
        if let Some((app_id, _)) = parse_resource_uri(uri)
            && self.shared.is_upstream(app_id)
        {
            return Err(HubError::new(
                ErrorKind::ResourceNotFound,
                "上游 MCP 服务器的资源暂不支持订阅，请直接读取。",
            ));
        }
        self.shared
            .subscribe(API_SUBSCRIBER, uri)
            .map_err(HubError::from)
    }

    pub fn unsubscribe(&self, uri: &str) {
        self.shared.unsubscribe(API_SUBSCRIBER, uri);
    }

    /// 指定某 App 的目标实例（所有会话共用；会话内 `apps.select` 的选择优先）。`None` 清除。
    /// 渐进暴露生效时，选定实例的 App 在所有会话中直接列出，因此会触发一次工具列表变化。
    pub fn select_instance(&self, app_id: &str, instance_id: Option<&str>) {
        let changed = {
            let mut sel = lock(&self.shared.global_selected);
            match instance_id {
                Some(id) => sel.insert(app_id.to_owned(), id.to_owned()).is_none(),
                None => sel.remove(app_id).is_some(),
            }
        };
        if changed && (self.shared.progressive() || self.shared.stateless_progressive()) {
            self.shared.mark_tools_changed();
        }
    }

    /// 清除某个 API 会话的状态（已附带的总览、`apps.select`），并取消该会话发出的租约（`app/lease { ttlMs: 0 }`）。
    /// 厂商开始新对话时调用。spec 之外的补充方法。
    pub fn reset_session(&self, session: Option<&str>) {
        self.shared.end_task(&CallerKey::api(session));
    }

    // ---- 事件 ----

    pub fn events(&self) -> broadcast::Receiver<HubEvent> {
        self.shared.events.subscribe()
    }

    // ---- 策略回调 ----

    pub fn set_approval_handler(&self, h: Arc<dyn ApprovalHandler>) {
        *lock(&self.shared.approval_handler) = Some(h);
    }

    /// 设置后，未知 App（无静态清单，或 Origin 不在白名单）连接时先返回 `pending`，
    /// 询问 handler 后以 `app/pairingResult` 通知结果。未设置时行为与原 Host 一致。
    pub fn set_pairing_handler(&self, h: Arc<dyn PairingHandler>) {
        *lock(&self.shared.pairing_handler) = Some(h);
    }

    /// 替换唤醒实现（默认由 `HubConfig::waker` 决定，即 [`crate::SystemWaker`]）。spec/hub-api.md 3.5。
    pub fn set_waker(&self, w: Arc<dyn Waker>) {
        *lock(&self.shared.waker) = Some(w);
    }

    /// 撤销 [`Hub::set_waker`]，恢复按 `HubConfig::waker` 构造的唤醒器（`none` 即不唤醒）。
    /// 绑定层清除自定义唤醒回调时调用。spec 之外的补充方法。
    pub fn reset_waker(&self) {
        // 配置在 Hub::start 时已成功构造过一次，这里不会失败；万一失败按不唤醒处理。
        *lock(&self.shared.waker) = self.shared.config.waker.build().ok().flatten();
    }

    // ---- 对外出口 ----

    /// 为一个 MCP 连接创建会话处理器（rmcp `ServerHandler`）。feature `mcp-server`。
    #[cfg(feature = "mcp-server")]
    pub fn mcp_session(&self) -> McpSession {
        McpSession::new(self.shared.clone())
    }

    /// 以 stdio 作为 MCP 传输运行，直到 MCP 客户端断开。feature `mcp-server`。
    #[cfg(feature = "mcp-server")]
    pub async fn serve_stdio(&self) -> anyhow::Result<()> {
        use rmcp::ServiceExt;
        let service = self.mcp_session().serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }

    /// 在一条双向字节流上提供 MCP（帧与 [`Hub::serve_stdio`] 相同：每行一条 JSON-RPC 消息），直到对端关闭。
    /// 用于系统 IPC 交来的通道（Android 独立 Hub App：Agent 经 `bindService` 换得的 socketpair fd，TASKS 4g d）。
    /// 每次调用是一个独立的 MCP 会话。feature `mcp-server`。
    #[cfg(feature = "mcp-server")]
    pub async fn serve_mcp_stream<S>(&self, stream: S) -> anyhow::Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        use rmcp::ServiceExt;
        let service = self.mcp_session().serve(stream).await?;
        service.waiting().await?;
        Ok(())
    }

    /// 另开一个 HTTP 监听器（路径与主服务相同：`/app`、`/mcp`、`/healthz`，且总是提供 `/mcp`），返回实际监听地址。
    ///
    /// 非回环地址需要 `allow_remote`；否则返回错误。等价于
    /// [`Hub::serve_http_with`]`(addr, HttpOptions { allow_remote, ..Default::default() })`。
    pub async fn serve_http(&self, addr: &str, allow_remote: bool) -> std::io::Result<SocketAddr> {
        self.serve_http_with(
            addr,
            HttpOptions {
                allow_remote,
                ..Default::default()
            },
        )
        .await
    }

    /// 另开一个 HTTP 监听器，带访问令牌等选项（`/mcp` 总是开启）。主服务见 [`HubConfig::listen`](super::HubConfig::listen)；
    /// 本方法用于额外的地址（如 `app-mcp-host` 兼容期内的旧 MCP 端口）。
    ///
    /// 可多次调用；同一 Hub 上的所有 HTTP 会话共享 App 连接，每个会话有独立的 `apps.select` 选择与
    /// “已附带总览”状态。
    pub async fn serve_http_with(
        &self,
        addr: &str,
        options: HttpOptions,
    ) -> std::io::Result<SocketAddr> {
        // 本方法总是提供 /mcp：缺少 MCP 出口时 Unsupported（spec/hub-api.md cargo features）。
        crate::features::require_mcp_server("MCP 出口（Hub::serve_http）")?;
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        if !local.ip().is_loopback() && !options.allow_remote {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "HTTP 监听地址 {local} 不是回环地址；如确需远程访问请加 --http-allow-remote"
                ),
            ));
        }
        let router = Router::new(self.shared.clone(), Transport::Tcp, options, true, self.health_base());
        let task = tokio::spawn(router.serve_tcp(listener));
        lock(&self.tasks).push(task);
        tracing::info!(%local, "额外的 HTTP 服务已启动：MCP http://{local}/mcp，App 连接 ws://{local}/app");
        Ok(local)
    }

    // ---- 工具格式导出（spec/hub-api.md 第 5 节）----

    /// 按格式导出工具定义（名称已编码为 `[a-zA-Z0-9_-]{1,64}`）。
    pub fn export_tools(&self, format: ToolFormat, filter: &ToolFilter) -> Value {
        let codec = self.shared.name_codec();
        format::export(format, &self.tools(filter), &codec)
    }

    /// 执行模型返回的一个工具调用，返回该格式的“工具结果”消息。永不失败：错误以该格式的错误结果返回。
    pub async fn dispatch(&self, format: ToolFormat, tool_call: Value) -> Value {
        self.dispatch_in_session(format, tool_call, None).await
    }

    /// 同 [`Hub::dispatch`]，指定会话（总览首次附带按会话计算）。spec 之外的补充方法。
    pub async fn dispatch_in_session(
        &self,
        format: ToolFormat,
        tool_call: Value,
        session: Option<&str>,
    ) -> Value {
        let parsed = match format::parse_call(format, &tool_call) {
            Ok(p) => p,
            Err(p) => {
                let r = call::error_result(&ToolError::new(ErrorKind::InvalidInput, p.error.clone().unwrap_or_default()));
                return format::render_result(format, &p, &r);
            }
        };
        if let Some(msg) = &parsed.error {
            let r = call::error_result(&ToolError::new(ErrorKind::InvalidInput, msg.clone()));
            return format::render_result(format, &parsed, &r);
        }
        let ctx = CallCtx {
            mcp_session: None,
            name: self.shared.resolve_export_name(&parsed.name),
            arguments: parsed.arguments.clone(),
            caller: CallerKey::api(session),
            session: session.map(str::to_owned),
            instance_id: None,
            timeout: None,
            call_id: parsed.id.clone(),
            progress: None,
            idempotency_key: None,
            principal: None,
            client_name: None,
            task_id: None,
            priority: Default::default(),
        };
        let inv = self.shared.call(ctx, std::future::pending()).await;
        let r = match inv.to_mcp() {
            Ok(r) => r,
            Err(e) => call::error_result(&call::mcp_error_to_tool(&e)),
        };
        format::render_result(format, &parsed, &r)
    }

    /// 进程内 App（spec/hub-api.md 第 4 节，M2）：本期只预留，总是返回未实现错误。
    pub fn attach_local(
        &self,
        _hello: app_mcp_protocol::HelloParams,
    ) -> Result<LocalAppChannel, HubError> {
        Err(HubError::new(
            ErrorKind::UnsupportedProtocol,
            "进程内 App（attach_local）尚未实现，请通过 WebSocket 连接。",
        ))
    }
}
