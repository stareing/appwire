//! 工具调用与资源读取的唯一实现（spec/hub-api.md 3.2）。
//!
//! 调用顺序：上游 MCP 服务器 → 内置工具 → App 工具。App 工具：路由 → schema 校验 → 审批 → 转发
//! （路由在校验之前，因为 schema 取自目标实例注册的定义）。
//!
//! 结果先表示为 [`Invocation`]，再分别转换为 MCP `CallToolResult`（MCP 出口、格式分派）
//! 或 [`CallOutcome`]（Hub API）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use app_mcp_protocol::{
    Activation, ErrorKind, Risk, ToolError, ToolInfo, ToolsCancelParams, ToolsInvokeParams,
    ToolsInvokeResult, method,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorCode, MetaObject, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, TextContent, Tool, ToolAnnotations,
};
use rmcp::{ErrorData as McpError, Peer, RoleClient, ServiceError};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;

use crate::hub::{
    DEFAULT_MIME, HubShared, lock, overview_info, parse_resource_uri, resource_uri,
};
use crate::limits::{OutputValidation, Payload};
use crate::mcp_convert::{self, OutputShape};
use crate::overview::Overview;
use crate::schema::{self, SchemaCheck};
use crate::types::{
    ApprovalRequest, Availability, CallOutcome, CallRequest, HubError, HubTool, ResourceContent, ToolDeclaration,
};
use crate::upstream::decode_uri_component;

/// 内置工具名。
pub const TOOL_APPS_LIST: &str = "apps.list";
pub const TOOL_APPS_SELECT: &str = "apps.select";
pub const TOOL_APPS_OVERVIEW: &str = "apps.overview";
/// 渐进暴露（spec/hub-api.md 3.7）：查看某个 App 的工具并加入本会话的工具列表。
pub const TOOL_APPS_TOOLS: &str = "apps.tools";

/// 内置工具的 appId（保留名）。
pub const BUILTIN_APP_ID: &str = "apps";

/// 静态工具在 App 已连接但没有实例注册时的描述前缀（spec/manifest.md 第 5 节）。
pub const UNAVAILABLE_PREFIX: &str = "[当前不可用] ";

/// 一次调用的输入。
#[derive(Clone, Debug)]
pub(crate) struct CallCtx {
    pub name: String,
    pub arguments: Value,
    /// 会话键（总览附带、`apps.select` 按它计算）。
    pub session_key: String,
    /// 厂商会话 ID（原样放进 [`ApprovalRequest::session`]）。
    pub session: Option<String>,
    pub instance_id: Option<String>,
    pub timeout: Option<Duration>,
    pub call_id: Option<String>,
    /// 发起调用的 MCP 会话（渐进暴露展开新 App 时只通知该会话）；Hub API 为 `None`。
    pub mcp_session: Option<u64>,
}

impl CallCtx {
    pub(crate) fn from_request(req: CallRequest) -> Self {
        Self {
            mcp_session: None,
            session_key: crate::hub::api_session_key(req.session.as_deref()),
            name: req.name,
            arguments: req.arguments,
            session: req.session,
            instance_id: req.instance_id,
            timeout: req.timeout,
            call_id: req.call_id,
        }
    }
}

/// 调用的结果主体。
pub(crate) enum Body {
    /// 名称无法解析（格式不对或 appId 未知）。
    NotFound(ToolError),
    Builtin(Result<CallToolResult, ToolError>),
    App(Result<ToolsInvokeResult, ToolError>),
    /// 上游结果原样保留；协议错误（`McpError`）在 MCP 出口原样透传。
    Upstream(Result<CallToolResult, McpError>),
}

pub(crate) struct Invocation {
    pub call_id: String,
    pub app_id: Option<String>,
    pub instance_id: Option<String>,
    /// 本次附带的总览（该会话首次接触此 App 或版本变化时）。
    pub overview: Option<Overview>,
    pub body: Body,
    /// App 工具声明的 `outputSchema` 决定的 `structuredContent` 形式。
    pub output_shape: OutputShape,
}

impl Invocation {
    /// MCP 出口的结果：总览在内容最前面。
    pub(crate) fn to_mcp(&self) -> Result<CallToolResult, McpError> {
        let mut r = match &self.body {
            Body::NotFound(e) | Body::Builtin(Err(e)) | Body::App(Err(e)) => error_result(e),
            Body::Builtin(Ok(r)) | Body::Upstream(Ok(r)) => r.clone(),
            Body::App(Ok(r)) => {
                success_result(self.app_id.as_deref().unwrap_or_default(), r.clone(), self.output_shape)
            }
            Body::Upstream(Err(e)) => return Err(e.clone()),
        };
        if let Some(ov) = &self.overview {
            r.content.insert(0, ContentBlock::text(ov.render()));
        }
        Ok(r)
    }

    /// Hub API 的结果。
    pub(crate) fn into_outcome(self) -> Result<CallOutcome, HubError> {
        let mut state_hints = Vec::new();
        let mut app_result = None;
        let result = match self.body {
            Body::NotFound(e) => return Err(HubError(e)),
            Body::Builtin(r) => r.map(|r| result_value(&r)),
            Body::App(r) => r.map(|mut r| {
                state_hints = std::mem::take(&mut r.state_hints);
                let data = std::mem::take(&mut r.data);
                app_result = Some(r);
                data
            }),
            Body::Upstream(Ok(r)) if r.is_error == Some(true) => Err(ToolError::new(
                ErrorKind::HandlerError,
                result_text(&r),
            )),
            Body::Upstream(Ok(r)) => Ok(result_value(&r)),
            Body::Upstream(Err(e)) => Err(mcp_error_to_tool(&e)),
        };
        let app_id = self.app_id.unwrap_or_default();
        let r = app_result.unwrap_or_default();
        Ok(CallOutcome {
            call_id: self.call_id,
            result,
            state_hints,
            instance_id: self.instance_id,
            overview: self.overview.as_ref().map(overview_info),
            status: r.status,
            state_resource: r.state_resource.map(|n| resource_uri(&app_id, &n)),
            summary: r.summary,
            annotations: r.annotations,
        })
    }
}

type CancelFut<'a> = Pin<&'a mut (dyn Future<Output = ()> + Send)>;

/// 一次 App 工具调用的结果。
pub(crate) struct ToolRun {
    pub result: Result<ToolsInvokeResult, ToolError>,
    /// 实际处理调用的实例。
    pub instance_id: Option<String>,
    pub output_shape: OutputShape,
}

fn cancelled() -> ToolError {
    ToolError::new(ErrorKind::Cancelled, "调用已被取消。")
}

impl HubShared {
    fn new_call_id(&self) -> String {
        format!("call-{}-{:08x}", self.next_id(), rand::random::<u32>())
    }

    /// 执行一次调用。`cancel` 完成时视为调用方取消；[`crate::Hub::cancel_call`] 同样可以取消。
    pub(crate) async fn call(
        self: &Arc<Self>,
        ctx: CallCtx,
        cancel: impl Future<Output = ()> + Send,
    ) -> Invocation {
        let call_id = ctx.call_id.clone().unwrap_or_else(|| self.new_call_id());
        let _activity = self.session_request(&ctx.session_key);
        let (tx, rx) = oneshot::channel::<()>();
        let token = self.next_id();
        lock(&self.calls).insert(call_id.clone(), (token, tx));
        let combined = async move {
            tokio::select! {
                _ = cancel => {}
                r = rx => {
                    if r.is_err() {
                        // 发送端被丢弃（同名 callId 被新调用覆盖）：不再能取消。
                        std::future::pending::<()>().await;
                    }
                }
            }
        };
        let mut combined = std::pin::pin!(combined);
        let inv = self.call_inner(&call_id, ctx, combined.as_mut()).await;
        let mut calls = lock(&self.calls);
        if calls.get(&call_id).is_some_and(|(t, _)| *t == token) {
            calls.remove(&call_id);
        }
        inv
    }

    async fn call_inner(self: &Arc<Self>, call_id: &str, ctx: CallCtx, mut cancel: CancelFut<'_>) -> Invocation {
        let name = ctx.name.clone();
        let args = if ctx.arguments.is_null() {
            json!({})
        } else {
            ctx.arguments.clone()
        };
        let inv = |app_id: Option<&str>, body: Body| Invocation {
            call_id: call_id.to_owned(),
            app_id: app_id.map(str::to_owned),
            instance_id: None,
            overview: None,
            body,
            output_shape: OutputShape::Undeclared,
        };

        // 上游 MCP 服务器
        if let Some((app_id, tool)) = name.split_once('.')
            && let Some(peer) = self.upstream_peer(app_id)
        {
            let guard = if args.is_object() { self.guard_call(app_id, tool, &args) } else { Ok(()) };
            let result = match (args, guard) {
                (Value::Object(_), Err(e)) => Ok(error_result(&e)),
                (Value::Object(map), Ok(())) => {
                    // 缓存中没有该工具（列表刚变化）时按 write 风险审批。
                    let hub_tool = match self.upstream_tool(app_id, tool) {
                        Some(t) => upstream_hub_tool(app_id, &t),
                        None => upstream_hub_tool(
                            app_id,
                            &Tool::new(tool.to_owned(), String::new(), Map::new()),
                        ),
                    };
                    let req =
                        self.approval_request(call_id, &hub_tool, &Value::Object(map.clone()), &ctx);
                    let approval = self.approve(req, cancel.as_mut()).await;
                    match approval {
                        Err(e) => Ok(error_result(&e)),
                        Ok(()) => {
                            self.call_upstream(app_id, tool, map, peer, ctx.timeout, cancel.as_mut())
                                .await
                        }
                    }
                }
                _ => Ok(error_result(&ToolError::new(
                    ErrorKind::InvalidInput,
                    "参数必须是 JSON 对象。",
                ))),
            };
            let mut out = inv(Some(app_id), Body::Upstream(result));
            if !matches!(out.body, Body::Upstream(Err(_))) {
                out.overview = self.attach_overview(&ctx.session_key, app_id);
            }
            self.expose_in_session(&ctx, app_id);
            return out;
        }

        // 内置工具
        if let Some(r) = self.call_builtin(&ctx, &name, &args) {
            return inv(None, Body::Builtin(r));
        }

        // App 工具
        let Some((app_id, tool)) = name.split_once('.') else {
            return inv(
                None,
                Body::NotFound(ToolError::new(
                    ErrorKind::ToolNotFound,
                    format!("工具「{name}」不存在，工具名格式为 <appId>.<工具名>。"),
                )),
            );
        };
        if !self.registry().has_app(app_id) {
            let e = ToolError::new(
                ErrorKind::ToolNotFound,
                format!("没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"),
            );
            return inv(Some(app_id), Body::NotFound(e));
        }
        if let Err(e) = self.guard_call(app_id, tool, &args) {
            let mut out = inv(Some(app_id), Body::App(Err(e)));
            out.overview = self.attach_overview(&ctx.session_key, app_id);
            return out;
        }
        let run = self.invoke_tool(call_id, app_id, tool, args, &ctx, cancel).await;
        let mut out = inv(Some(app_id), Body::App(run.result));
        out.instance_id = run.instance_id;
        out.output_shape = run.output_shape;
        out.overview = self.attach_overview(&ctx.session_key, app_id);
        self.expose_in_session(&ctx, app_id);
        out
    }

    /// 渐进暴露：把 App 加入调用方会话的工具列表；列表因此变化时通知该 MCP 会话。
    fn expose_in_session(self: &Arc<Self>, ctx: &CallCtx, app_id: &str) {
        if self.expose_app(&ctx.session_key, app_id)
            && let Some(id) = ctx.mcp_session
        {
            self.notify_session_tools_changed(id);
        }
    }

    /// 转发前的资源保护（spec/hub-api.md 3.11）：参数大小上限，然后（App, 工具）与 App 两级限流。
    /// 超出时计入该 App 的拒绝计数，返回 `PAYLOAD_TOO_LARGE` / `RATE_LIMITED`。
    fn guard_call(&self, app_id: &str, tool: &str, args: &Value) -> Result<(), ToolError> {
        let limits = &self.config.limits;
        if limits.max_arguments_bytes > 0 {
            let size = serde_json::to_vec(args).map_or(0, |v| v.len());
            if let Err(e) = Payload::Arguments.check(size, limits.max_arguments_bytes, &format!("工具「{app_id}.{tool}」")) {
                lock(&self.rates).record_too_large(app_id);
                return Err(e);
            }
        }
        let now = tokio::time::Instant::now();
        lock(&self.rates)
            .acquire(limits, app_id, tool, now)
            .map_err(|r| r.to_error(app_id, tool))
    }

    /// 结果 / 资源内容的大小上限；超出时计入该 App 的拒绝计数。
    pub(crate) fn guard_payload(&self, app_id: &str, part: Payload, size: usize, target: &str) -> Result<(), ToolError> {
        let limits = &self.config.limits;
        let limit = match part {
            Payload::Arguments => limits.max_arguments_bytes,
            Payload::Result => limits.max_result_bytes,
            Payload::Resource => limits.max_resource_bytes,
        };
        part.check(size, limit, target).inspect_err(|_| lock(&self.rates).record_too_large(app_id))
    }

    fn approval_request(&self, call_id: &str, t: &HubTool, args: &Value, ctx: &CallCtx) -> ApprovalRequest {
        let app_name = if self.is_upstream(&t.app_id) {
            self.upstream_display_name(&t.app_id)
        } else {
            self.registry()
                .display_name(&t.app_id)
                .unwrap_or_else(|| t.app_id.clone())
        };
        ApprovalRequest {
            call_id: call_id.to_owned(),
            app_id: t.app_id.clone(),
            app_name,
            tool: t.tool.clone(),
            title: t.title.clone(),
            description: t.description.clone(),
            risk: t.risk,
            arguments: args.clone(),
            session: ctx.session.clone(),
            annotations: t.annotations.clone(),
        }
    }

    /// 按策略审批（spec/hub-api.md 3.3）：拒绝 / 超时 / 未设置处理器 → `USER_REJECTED`。
    async fn approve(&self, req: ApprovalRequest, cancel: CancelFut<'_>) -> Result<(), ToolError> {
        let policy = &self.config.approval;
        if !policy.requires(req.risk) {
            return Ok(());
        }
        let Some(handler) = self.approval_handler() else {
            return Err(ToolError::new(
                ErrorKind::UserRejected,
                "该操作需要用户确认，但当前没有可用的确认界面，调用未执行。",
            ));
        };
        let timeout = policy.timeout.unwrap_or(self.config.response_timeout);
        let label = format!("{}.{}", req.app_id, req.tool);
        tokio::select! {
            r = tokio::time::timeout(timeout, handler.approve(req)) => match r {
                Ok(true) => Ok(()),
                Ok(false) => Err(ToolError::new(
                    ErrorKind::UserRejected,
                    format!("用户拒绝了本次调用（{label}），操作未执行。请勿在未获用户同意时重试。"),
                )),
                Err(_) => Err(ToolError::new(
                    ErrorKind::UserRejected,
                    format!("等待用户确认超时（{} 秒），调用（{label}）未执行。", timeout.as_secs()),
                )),
            },
            _ = cancel => Err(cancelled()),
        }
    }

    /// 调用 App 工具：路由 → schema 校验 → 审批 → 转发。
    async fn invoke_tool(
        self: &Arc<Self>,
        call_id: &str,
        app_id: &str,
        tool_name: &str,
        arguments: Value,
        ctx: &CallCtx,
        cancel: CancelFut<'_>,
    ) -> ToolRun {
        let mut output_shape = OutputShape::Undeclared;
        let (result, instance_id) = self
            .invoke_routed(call_id, app_id, tool_name, arguments, ctx, cancel, &mut output_shape)
            .await;
        ToolRun { result, instance_id, output_shape }
    }

    /// 结果到达后的处理：大小上限 → 解析 → 按 [`OutputValidation`] 核对 `outputSchema`。
    fn accept_result(&self, app_id: &str, tool: &str, info: &ToolInfo, v: Value) -> Result<ToolsInvokeResult, ToolError> {
        let size = serde_json::to_vec(&v).map_or(0, |b| b.len());
        self.guard_payload(app_id, Payload::Result, size, &format!("工具「{app_id}.{tool}」"))?;
        let r = match serde_json::from_value::<ToolsInvokeResult>(v.clone()) {
            Ok(r) => r,
            Err(_) => ToolsInvokeResult { data: v, ..ToolsInvokeResult::default() },
        };
        let (Some(schema), false) = (&info.output_schema, r.data.is_null()) else {
            return Ok(r);
        };
        let mode = self.config.output_validation;
        if mode == OutputValidation::Off {
            return Ok(r);
        }
        let msg = match schema::check(schema, &r.data) {
            SchemaCheck::Invalid(msg) => msg,
            SchemaCheck::BadSchema(e) => {
                tracing::warn!(app_id, tool, error = %e, "工具的 outputSchema 无法编译，跳过结果校验");
                return Ok(r);
            }
            SchemaCheck::Valid | SchemaCheck::Unchecked => return Ok(r),
        };
        if mode == OutputValidation::Reject {
            return Err(ToolError::new(
                ErrorKind::HandlerError,
                format!("App 返回的结果不符合工具「{app_id}.{tool}」声明的 outputSchema：{msg}。调用可能已在 App 内执行，请确认状态后再决定是否重试。"),
            )
            .with_details(json!({ "appId": app_id, "outputSchemaError": msg })));
        }
        tracing::warn!(app_id, tool, error = %msg, "App 返回的结果不符合其声明的 outputSchema（只记录，结果照常返回）");
        Ok(r)
    }

    /// [`HubShared::invoke_tool`] 的主体：返回结果与目标实例；路由到实例后把其 `outputSchema` 形式写入 `output_shape`。
    #[allow(clippy::too_many_arguments)]
    async fn invoke_routed(
        self: &Arc<Self>,
        call_id: &str,
        app_id: &str,
        tool_name: &str,
        arguments: Value,
        ctx: &CallCtx,
        mut cancel: CancelFut<'_>,
        output_shape: &mut OutputShape,
    ) -> (Result<ToolsInvokeResult, ToolError>, Option<String>) {
        self.lease_call_started(&ctx.session_key, app_id);
        let selected = ctx
            .instance_id
            .clone()
            .or_else(|| self.selected_for(&ctx.session_key, app_id));
        // 休眠实例 / 未运行的 App：先按快照（或清单）定义校验并审批，再唤醒（spec/lifecycle.md §9）。
        // 注意：先放开注册表锁再解析唤醒描述（resolve_wake_descriptor 会再次加锁）。
        let plan = self.registry().wake_plan_tool(
            app_id,
            tool_name,
            selected.as_deref(),
            ctx.instance_id.is_some(),
        );
        let plan =
            plan.filter(|p| p.instance_id.is_some() || self.resolve_wake_descriptor(p).is_some());
        let mut woken = None;
        if let Some(plan) = plan {
            // 不唤醒（`waker: none`）：不做审批，直接按未连接返回（带 launchUrl）。
            if !self.wake_enabled() {
                return (Err(self.registry().disconnected_error(app_id)), plan.instance_id.clone());
            }
            if let Some(tool) = &plan.tool {
                if let SchemaCheck::Invalid(msg) = schema::check(&tool.input_schema, &arguments) {
                    return (
                        Err(ToolError::new(
                            ErrorKind::InvalidInput,
                            format!("参数不符合工具「{app_id}.{tool_name}」的 inputSchema：{msg}"),
                        )),
                        plan.instance_id.clone(),
                    );
                }
                let hub_tool = app_hub_tool(app_id, tool.clone(), Availability::Dormant);
                let req = self.approval_request(call_id, &hub_tool, &arguments, ctx);
                if let Err(e) = self.approve(req, cancel.as_mut()).await {
                    return (Err(e), plan.instance_id.clone());
                }
            }
            match self.wake_and_wait(&plan, cancel.as_mut()).await {
                Ok(id) => woken = Some(id),
                Err(e) => return (Err(e), plan.instance_id.clone()),
            }
        }
        let prefer = woken.clone().or(selected);
        let routed = self
            .registry()
            .route_tool(app_id, tool_name, prefer.as_deref());
        let target = match routed {
            Ok(t) => t,
            Err(e) => return (Err(e), None),
        };
        let _work = target.conn.begin_work();
        if let Some(want) = &ctx.instance_id
            && woken.is_none()
            && &target.instance_id != want
        {
            return (
                Err(ToolError::new(
                    ErrorKind::ToolNotFound,
                    format!("实例「{want}」不存在或没有注册工具「{tool_name}」。可调用 apps.list 查看各实例注册的工具。"),
                )
                .with_details(json!({ "appId": app_id, "instanceId": want }))),
                None,
            );
        }
        let instance = Some(target.instance_id.clone());
        *output_shape = OutputShape::of(target.tool.output_schema.as_ref());
        match schema::check(&target.tool.input_schema, &arguments) {
            SchemaCheck::Valid => {}
            SchemaCheck::Invalid(msg) => {
                return (
                    Err(ToolError::new(
                        ErrorKind::InvalidInput,
                        format!("参数不符合工具「{app_id}.{tool_name}」的 inputSchema：{msg}"),
                    )),
                    instance,
                );
            }
            SchemaCheck::BadSchema(e) => {
                tracing::warn!(app_id, tool = tool_name, error = %e, "工具的 inputSchema 无法编译，跳过 Hub 侧校验");
            }
            SchemaCheck::Unchecked => {}
        }

        if woken.is_none() {
            let hub_tool = app_hub_tool(app_id, target.tool.clone(), Availability::Available);
            let req = self.approval_request(call_id, &hub_tool, &arguments, ctx);
            if let Err(e) = self.approve(req, cancel.as_mut()).await {
                return (Err(e), instance);
            }
        }

        let response_timeout = ctx.timeout.unwrap_or(self.config.response_timeout);
        let sdk_timeout = ctx
            .timeout
            .map_or(self.config.invoke_timeout, |t| t.min(self.config.invoke_timeout));
        let params = ToolsInvokeParams {
            call_id: call_id.to_owned(),
            name: tool_name.to_owned(),
            arguments,
            timeout_ms: Some(sdk_timeout.as_millis() as u64),
        };
        let params = match serde_json::to_value(params) {
            Ok(p) => p,
            Err(e) => return (Err(ToolError::new(ErrorKind::HandlerError, e.to_string())), instance),
        };
        let conn = target.conn.clone();
        let disconnected = || {
            ToolError::new(
                ErrorKind::AppDisconnected,
                format!(
                    "调用过程中 App「{app_id}」的实例断开了连接，调用结果未知。请用 apps.list 确认状态后再决定是否重试。"
                ),
            )
        };
        let (req_id, rx) = match conn.start_request(method::TOOLS_INVOKE, params) {
            Ok(v) => v,
            Err(_) => return (Err(disconnected()), instance),
        };
        let send_cancel = |reason: &str| {
            conn.forget(&req_id);
            let p = ToolsCancelParams {
                call_id: call_id.to_owned(),
                reason: Some(reason.to_owned()),
            };
            conn.notify(
                method::TOOLS_CANCEL,
                serde_json::to_value(p).unwrap_or(Value::Null),
            );
        };

        let outcome = tokio::select! {
            r = tokio::time::timeout(response_timeout, rx) => r,
            _ = cancel => {
                send_cancel("cancelled by MCP client");
                return (Err(cancelled()), instance);
            }
        };
        let result = match outcome {
            Ok(Ok(Ok(v))) => self.accept_result(app_id, tool_name, &target.tool, v),
            Ok(Ok(Err(rpc))) => Err(rpc.to_tool_error()),
            Ok(Err(_)) => return (Err(disconnected()), instance),
            Err(_) => {
                send_cancel("timeout");
                return (
                    Err(ToolError::new(
                        ErrorKind::Timeout,
                        format!(
                            "App 在 {} 秒内没有返回结果，调用已取消。App 可能正忙或页面处于后台，可稍后重试。",
                            response_timeout.as_secs()
                        ),
                    )),
                    instance,
                );
            }
        };
        self.registry().touch(app_id, &target.instance_id);
        self.grant_lease(&ctx.session_key, app_id, &conn);
        (result, instance)
    }

    /// 转发给上游 MCP 服务器；结果原样返回，协议错误原样透传。
    async fn call_upstream(
        &self,
        name: &str,
        tool: &str,
        args: Map<String, Value>,
        peer: Option<Peer<RoleClient>>,
        timeout: Option<Duration>,
        cancel: CancelFut<'_>,
    ) -> Result<CallToolResult, McpError> {
        let Some(peer) = peer else {
            return Ok(error_result(&ToolError::new(
                ErrorKind::AppDisconnected,
                format!(
                    "上游 MCP 服务器「{name}」当前未连接（进程已退出，Hub 正在重启它），请稍后重试。"
                ),
            )));
        };
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(args);
        let timeout = timeout.unwrap_or(self.config.response_timeout);
        tokio::select! {
            r = tokio::time::timeout(timeout, peer.call_tool(params)) => match r {
                Ok(Ok(result)) => {
                    let size = serde_json::to_vec(&result).map_or(0, |v| v.len());
                    match self.guard_payload(name, Payload::Result, size, &format!("工具「{name}.{tool}」")) {
                        Ok(()) => Ok(result),
                        Err(e) => Ok(error_result(&e)),
                    }
                }
                Ok(Err(ServiceError::McpError(e))) => Err(e),
                Ok(Err(e)) => Ok(error_result(&ToolError::new(
                    ErrorKind::AppDisconnected,
                    format!("调用上游 MCP 服务器「{name}」失败：{e}"),
                ))),
                Err(_) => Ok(error_result(&ToolError::new(
                    ErrorKind::Timeout,
                    format!("上游 MCP 服务器「{name}」在 {} 秒内没有返回结果。", timeout.as_secs()),
                ))),
            },
            _ = cancel => Ok(error_result(&cancelled())),
        }
    }

    /// 内置工具；不是内置工具时返回 `None`。
    fn call_builtin(
        self: &Arc<Self>,
        ctx: &CallCtx,
        name: &str,
        args: &Value,
    ) -> Option<Result<CallToolResult, ToolError>> {
        let key = ctx.session_key.as_str();
        let schema = builtin_schema(name)?;
        if let SchemaCheck::Invalid(msg) = schema::check(&schema, args) {
            return Some(Err(ToolError::new(
                ErrorKind::InvalidInput,
                format!("参数不符合工具「{name}」的 inputSchema：{msg}"),
            )));
        }
        let arg = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Some(match name {
            TOOL_APPS_LIST => Ok(json_result(self.apps_json(&self.merged_selection(key)))),
            TOOL_APPS_SELECT => {
                let (app_id, instance_id) = (arg("appId"), arg("instanceId"));
                if self.registry().instance(&app_id, &instance_id).is_none() {
                    return Some(Err(ToolError::new(
                        ErrorKind::AppDisconnected,
                        format!(
                            "App「{app_id}」没有已连接的实例「{instance_id}」。可调用 apps.list 查看当前实例。"
                        ),
                    )));
                }
                // 选定前判断是否已列出，选定后该 App 在本会话直接列出。
                let newly_listed = self
                    .exposed_apps(key)
                    .is_some_and(|e| !e.contains(&app_id));
                self.select_in_session(key, &app_id, &instance_id);
                if newly_listed && let Some(id) = ctx.mcp_session {
                    self.notify_session_tools_changed(id);
                }
                Ok(json_result(json!({
                    "appId": app_id,
                    "instanceId": instance_id,
                    "message": format!("本会话中对 {app_id} 的调用将优先路由到实例 {instance_id}（该实例注册了对应工具且仍连接时）。"),
                })))
            }
            TOOL_APPS_OVERVIEW => {
                let app_id = arg("appId");
                let known = self.is_upstream(&app_id) || self.registry().has_app(&app_id);
                if !known {
                    return Some(Err(ToolError::new(
                        ErrorKind::ToolNotFound,
                        format!(
                            "没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"
                        ),
                    )));
                }
                match self.overview(&app_id) {
                    None => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                        "App「{app_id}」没有提供总览。"
                    ))])),
                    Some(ov) => {
                        let mut r = CallToolResult::success(vec![ContentBlock::text(ov.render())]);
                        r.structured_content = Some(ov.to_json());
                        self.mark_delivered(key, &app_id, &ov.version);
                        Ok(r)
                    }
                }
            }
            TOOL_APPS_TOOLS => {
                let app_id = arg("appId");
                let known = self.is_upstream(&app_id) || self.registry().has_app(&app_id);
                if !known {
                    return Some(Err(ToolError::new(
                        ErrorKind::ToolNotFound,
                        format!(
                            "没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"
                        ),
                    )));
                }
                let tools = self.app_tools(&app_id);
                let progressive = self.progressive();
                self.expose_in_session(ctx, &app_id);
                let message = if tools.is_empty() {
                    format!("App「{app_id}」当前没有工具。")
                } else if progressive {
                    format!(
                        "App「{app_id}」的 {} 个工具已加入本会话的工具列表（客户端刷新列表后可见）；在此之前也可以直接按全名调用。",
                        tools.len()
                    )
                } else {
                    format!("App「{app_id}」的工具都已在工具列表中，可直接按全名调用。")
                };
                Ok(json_result(json!({
                    "appId": app_id,
                    "tools": tools,
                    "message": message,
                })))
            }
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// 资源读取
// ---------------------------------------------------------------------------

/// 读取资源（MCP 出口与 Hub API 共用）。上游的协议错误原样返回。
pub(crate) async fn read_resource(
    shared: &Arc<HubShared>,
    uri: &str,
    session_key: &str,
) -> Result<ReadResourceResult, McpError> {
    let Some((app_id, name)) = parse_resource_uri(uri) else {
        return Err(McpError::resource_not_found(
            format!("无法识别的资源 URI：{uri}"),
            None,
        ));
    };
    let _activity = shared.session_request(session_key);
    if let Some(peer) = shared.upstream_peer(app_id) {
        return read_upstream_resource(shared, app_id, name, uri, peer).await;
    }
    let (info, result) = shared
        .read_app_resource(app_id, name, shared.selected_for(session_key, app_id))
        .await
        .map_err(|e| to_mcp_error(&e))?;
    let contents = resource_contents(uri, info.mime_type.as_deref(), result);
    check_resource_size(shared, app_id, uri, std::slice::from_ref(&contents))?;
    Ok(ReadResourceResult::new(vec![contents]))
}

async fn read_upstream_resource(
    shared: &HubShared,
    name: &str,
    encoded: &str,
    uri: &str,
    peer: Option<Peer<RoleClient>>,
) -> Result<ReadResourceResult, McpError> {
    let peer = peer.ok_or_else(|| {
        McpError::new(
            ErrorCode(ErrorKind::AppDisconnected.code() as i32),
            format!("上游 MCP 服务器「{name}」当前未连接。"),
            Some(json!({ "kind": ErrorKind::AppDisconnected })),
        )
    })?;
    let original = decode_uri_component(encoded).ok_or_else(|| {
        McpError::resource_not_found(format!("无法识别的资源 URI：{uri}"), None)
    })?;
    let fut = peer.read_resource(ReadResourceRequestParams::new(original.clone()));
    let mut result = match tokio::time::timeout(shared.config.response_timeout, fut).await {
        Ok(Ok(r)) => r,
        Ok(Err(ServiceError::McpError(e))) => return Err(e),
        Ok(Err(e)) => {
            return Err(McpError::internal_error(
                format!("读取上游资源失败：{e}"),
                None,
            ));
        }
        Err(_) => return Err(McpError::internal_error("读取上游资源超时", None)),
    };
    for c in &mut result.contents {
        let u = match c {
            ResourceContents::TextResourceContents { uri, .. } => uri,
            ResourceContents::BlobResourceContents { uri, .. } => uri,
            #[allow(unreachable_patterns)]
            _ => continue,
        };
        if *u == original {
            *u = uri.to_owned();
        }
    }
    check_resource_size(shared, name, uri, &result.contents)?;
    Ok(result)
}

/// 资源内容（文本 / base64）的大小上限（spec/hub-api.md 3.11）。
fn check_resource_size(shared: &HubShared, app_id: &str, uri: &str, contents: &[ResourceContents]) -> Result<(), McpError> {
    let size: usize = contents
        .iter()
        .map(|c| match c {
            ResourceContents::TextResourceContents { text, .. } => text.len(),
            ResourceContents::BlobResourceContents { blob, .. } => blob.len(),
            #[allow(unreachable_patterns)]
            _ => 0,
        })
        .sum();
    shared
        .guard_payload(app_id, Payload::Resource, size, &format!("资源「{uri}」"))
        .map_err(|e| to_mcp_error(&e))
}

/// Hub API：取第一段内容。
pub(crate) fn first_content(uri: &str, r: ReadResourceResult) -> ResourceContent {
    let empty = ResourceContent {
        uri: uri.to_owned(),
        mime_type: None,
        text: None,
        blob: None,
    };
    match r.contents.into_iter().next() {
        Some(ResourceContents::TextResourceContents {
            uri, mime_type, text, ..
        }) => ResourceContent {
            uri,
            mime_type,
            text: Some(text),
            blob: None,
        },
        Some(ResourceContents::BlobResourceContents {
            uri, mime_type, blob, ..
        }) => ResourceContent {
            uri,
            mime_type,
            text: None,
            blob: Some(blob),
        },
        #[allow(unreachable_patterns)]
        _ => empty,
    }
}

pub(crate) fn resource_contents(
    uri: &str,
    default_mime: Option<&str>,
    result: app_mcp_protocol::ResourcesReadResult,
) -> ResourceContents {
    let mime = result
        .mime_type
        .as_deref()
        .or(default_mime)
        .unwrap_or(DEFAULT_MIME)
        .to_owned();
    let is_json = mime == DEFAULT_MIME || mime.ends_with("+json");
    let text = match (&result.contents, is_json) {
        (Value::String(s), false) => s.clone(),
        (v, _) => serde_json::to_string(v).unwrap_or_default(),
    };
    ResourceContents::text(text, uri).with_mime_type(mime)
}

// ---------------------------------------------------------------------------
// 工具定义
// ---------------------------------------------------------------------------

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// 内置工具（MCP 形式）。`with_apps_tools`：是否包含 `apps.tools`（只在渐进暴露生效时列出；任何时候都可调用）。
pub(crate) fn builtin_tools(with_apps_tools: bool) -> Vec<Tool> {
    let mut tools = all_builtin_tools();
    if !with_apps_tools {
        tools.retain(|t| t.name != TOOL_APPS_TOOLS);
    }
    tools
}

fn all_builtin_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            TOOL_APPS_LIST,
            "列出本机已知的 App（静态清单与已连接实例）：appId、名称、简介、是否已连接、各实例的标题 / 地址 / \
             可见性 / 焦点 / 注册的工具、休眠中的实例（dormantInstances，调用其工具时会自动唤醒），以及当前会话选定的实例。",
            obj(json!({ "type": "object", "properties": {}, "additionalProperties": false })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_SELECT,
            "在当前会话中指定某个 App 的目标实例（例如用户开了多个标签页时）。之后对该 App 的调用优先路由到此实例\
             （仅当它注册了被调用的工具）。instanceId 可从 apps.list 获取。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "instanceId": { "type": "string", "description": "实例 ID（见 apps.list）" }
                },
                "required": ["appId", "instanceId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_OVERVIEW,
            "查看某个 App 的完整总览（能力范围、典型流程、不支持的操作等）。",
            obj(json!({
                "type": "object",
                "properties": { "appId": { "type": "string", "description": "App 标识" } },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_TOOLS,
            "列出某个 App 的全部工具（全名、说明、参数 inputSchema、风险、可用性）。工具较多时工具列表只含 apps.* 与本会话\
             用过的 App；调用本工具后该 App 的工具会加入本会话的工具列表，也可以直接按全名 <appId>.<工具名> 调用。\
             appId 可从 apps.list 获取。",
            obj(json!({
                "type": "object",
                "properties": { "appId": { "type": "string", "description": "App 标识" } },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
    ]
}

fn builtin_schema(name: &str) -> Option<Value> {
    all_builtin_tools()
        .into_iter()
        .find(|t| t.name == name)
        .map(|t| Value::Object((*t.input_schema).clone()))
}

pub(crate) fn builtin_hub_tools(with_apps_tools: bool) -> Vec<HubTool> {
    builtin_tools(with_apps_tools)
        .into_iter()
        .map(|t| {
            let name = t.name.to_string();
            HubTool {
                tool: name
                    .strip_prefix("apps.")
                    .unwrap_or(&name)
                    .to_owned(),
                name,
                app_id: BUILTIN_APP_ID.to_owned(),
                title: None,
                description: t.description.map(|d| d.to_string()).unwrap_or_default(),
                input_schema: Value::Object((*t.input_schema).clone()),
                risk: Risk::Read,
                activation: Activation::Headless,
                availability: Availability::Available,
                annotations: t
                    .annotations
                    .as_ref()
                    .map(mcp_convert::from_mcp_tool_annotations)
                    .unwrap_or_default(),
                output_schema: None,
            }
        })
        .collect()
}

pub(crate) fn app_hub_tool(app_id: &str, info: ToolInfo, availability: Availability) -> HubTool {
    HubTool {
        annotations: info.effective_annotations(),
        name: format!("{app_id}.{}", info.name),
        app_id: app_id.to_owned(),
        tool: info.name,
        title: info.title,
        description: info.description,
        input_schema: info.input_schema,
        risk: info.risk,
        activation: info.activation.unwrap_or_default(),
        availability,
        output_schema: info.output_schema,
    }
}

/// App 工具的声明（`/status` 的 `tools`，docs/plans/14-safety.md S5）。
pub(crate) fn tool_declaration(info: &ToolInfo) -> ToolDeclaration {
    ToolDeclaration {
        name: info.name.clone(),
        risk: info.risk,
        annotations: info.annotations.clone(),
        effective: info.effective_annotations(),
        output_schema: info.output_schema.is_some(),
    }
}

/// 上游工具的声明：注解原样（上游没有 `risk`，按注解推导，见 [`upstream_risk`]）。
pub(crate) fn upstream_tool_declaration(t: &Tool) -> ToolDeclaration {
    let annotations = t.annotations.as_ref().map(mcp_convert::from_mcp_tool_annotations);
    ToolDeclaration {
        name: t.name.to_string(),
        risk: upstream_risk(t),
        effective: annotations.clone().unwrap_or_default(),
        annotations,
        output_schema: t.output_schema.is_some(),
    }
}

/// 上游工具的风险：`readOnlyHint` → read；`destructiveHint` → destructive；否则 write。
pub(crate) fn upstream_risk(t: &Tool) -> Risk {
    match &t.annotations {
        Some(a) if a.read_only_hint == Some(true) => Risk::Read,
        Some(a) if a.destructive_hint == Some(true) => Risk::Destructive,
        _ => Risk::Write,
    }
}

pub(crate) fn upstream_hub_tool(name: &str, t: &Tool) -> HubTool {
    HubTool {
        name: format!("{name}.{}", t.name),
        app_id: name.to_owned(),
        tool: t.name.to_string(),
        title: t
            .title
            .clone()
            .or_else(|| t.annotations.as_ref().and_then(|a| a.title.clone())),
        description: t.description.as_deref().unwrap_or_default().to_owned(),
        input_schema: Value::Object((*t.input_schema).clone()),
        risk: upstream_risk(t),
        activation: Activation::Headless,
        availability: Availability::Available,
        annotations: t.annotations.as_ref().map(mcp_convert::from_mcp_tool_annotations).unwrap_or_default(),
        output_schema: t.output_schema.as_ref().map(|s| Value::Object((**s).clone())),
    }
}

/// App 工具的 MCP 形式。
#[cfg(feature = "mcp-server")]
pub(crate) fn to_mcp_tool(app_id: &str, info: &ToolInfo, availability: Availability) -> Tool {
    let schema = match &info.input_schema {
        Value::Object(m) => m.clone(),
        _ => {
            let mut m = Map::new();
            m.insert("type".into(), json!("object"));
            m
        }
    };
    let description = match availability {
        Availability::NotRegistered => format!("{UNAVAILABLE_PREFIX}{}", info.description),
        Availability::Available | Availability::Disconnected | Availability::Dormant => {
            info.description.clone()
        }
    };
    let mut tool = Tool::new(format!("{app_id}.{}", info.name), description, schema)
        .with_annotations(mcp_convert::tool_annotations(&info.effective_annotations()));
    if let Some(title) = &info.title {
        tool = tool.with_title(title.clone());
    }
    if let Some(output) = &info.output_schema {
        tool = tool.with_raw_output_schema(Arc::new(mcp_convert::mcp_output_schema(output)));
    }
    tool
}

// ---------------------------------------------------------------------------
// 结果转换
// ---------------------------------------------------------------------------

pub(crate) fn json_result(v: Value) -> CallToolResult {
    let text = serde_json::to_string(&v).unwrap_or_default();
    let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
    if v.is_object() {
        r.structured_content = Some(v);
    }
    r
}

/// 业务错误：以 `is_error: true` 的结果返回，模型能看到原因。
pub(crate) fn error_result(e: &ToolError) -> CallToolResult {
    let mut structured = json!({ "kind": e.kind, "message": e.message });
    if let Some(d) = &e.details {
        structured["details"] = d.clone();
    }
    let mut r = CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {}",
        e.kind, e.message
    ))]);
    r.structured_content = Some(json!({ "error": structured }));
    r
}

/// App 工具的成功结果（spec/hub-api.md 3.2）：状态说明（非 `done`）→ 摘要 → 返回值 JSON（无返回值、无摘要且 `done` 时为
/// "已完成"）→ 资源变化提示。App 的内容标注只加在 App 给出的内容块（摘要、返回值）上。
pub(crate) fn success_result(app_id: &str, r: ToolsInvokeResult, shape: OutputShape) -> CallToolResult {
    let state_uri = r.state_resource.as_deref().map(|n| resource_uri(app_id, n));
    let mut app_texts = Vec::new();
    if let Some(s) = &r.summary {
        app_texts.push(TextContent::new(s.clone()));
    }
    if !r.data.is_null() {
        app_texts.push(TextContent::new(serde_json::to_string(&r.data).unwrap_or_default()));
    }
    if app_texts.is_empty() && r.status.is_done() {
        app_texts.push(TextContent::new(mcp_convert::DONE_TEXT));
    }
    let annotations = r.annotations.as_ref().map(mcp_convert::content_annotations);
    let mut content: Vec<ContentBlock> = mcp_convert::status_note(r.status, state_uri.as_deref())
        .map(ContentBlock::text)
        .into_iter()
        .collect();
    content.extend(app_texts.into_iter().map(|t| {
        ContentBlock::Text(match &annotations {
            Some(a) => t.with_annotations(a.clone()),
            None => t,
        })
    }));
    if !r.state_hints.is_empty() {
        let uris: Vec<String> = r
            .state_hints
            .iter()
            .map(|h| resource_uri(app_id, h))
            .collect();
        content.push(ContentBlock::text(format!(
            "[app-mcp] 以下资源的内容可能已因本次调用而变化，如需最新状态请重新读取：{}",
            uris.join("、")
        )));
    }
    let mut result = CallToolResult::success(content);
    result.structured_content = shape.structured(&r.data);
    if !r.status.is_done() {
        let mut meta = MetaObject::new();
        meta.insert(mcp_convert::META_STATUS.to_owned(), json!(r.status));
        if let Some(uri) = state_uri {
            meta.insert(mcp_convert::META_STATE_RESOURCE.to_owned(), json!(uri));
        }
        result.meta = Some(meta);
    }
    result
}

/// 结果内容的纯文本：文本块以空行连接，非文本块序列化为 JSON。
pub fn result_text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .map(|c| match c.as_text() {
            Some(t) => t.text.clone(),
            None => serde_json::to_string(c).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Hub API 的结果值：结构化内容优先，其次单段文本（能解析为 JSON 时解析），否则内容数组。
fn result_value(r: &CallToolResult) -> Value {
    if let Some(v) = &r.structured_content {
        return v.clone();
    }
    match r.content.as_slice() {
        [] => Value::Null,
        [one] => match one.as_text() {
            Some(t) => serde_json::from_str(&t.text).unwrap_or_else(|_| Value::String(t.text.clone())),
            None => serde_json::to_value(one).unwrap_or(Value::Null),
        },
        many => serde_json::to_value(many).unwrap_or(Value::Null),
    }
}

/// MCP 协议错误 → 协议错误类别（`data.kind` 优先，其次错误码）。
pub(crate) fn mcp_error_to_tool(e: &McpError) -> ToolError {
    let from_data = e
        .data
        .as_ref()
        .and_then(|d| d.get("kind"))
        .and_then(|k| serde_json::from_value::<ErrorKind>(k.clone()).ok());
    let kind = from_data
        .or_else(|| {
            ALL_KINDS
                .iter()
                .copied()
                .find(|k| k.code() == i64::from(e.code.0))
        })
        .unwrap_or(if e.code == ErrorCode::RESOURCE_NOT_FOUND {
            ErrorKind::ResourceNotFound
        } else {
            ErrorKind::HandlerError
        });
    let details = e
        .data
        .as_ref()
        .and_then(|d| d.get("details"))
        .cloned();
    ToolError {
        kind,
        message: e.message.to_string(),
        details,
    }
}

const ALL_KINDS: [ErrorKind; 17] = [
    ErrorKind::ToolNotFound,
    ErrorKind::ToolDisabled,
    ErrorKind::InvalidInput,
    ErrorKind::UserRejected,
    ErrorKind::Timeout,
    ErrorKind::HandlerError,
    ErrorKind::Cancelled,
    ErrorKind::AppDisconnected,
    ErrorKind::AppNotInstalled,
    ErrorKind::LaunchFailed,
    ErrorKind::AppNotResponding,
    ErrorKind::InstanceFrozen,
    ErrorKind::ResourceNotFound,
    ErrorKind::Unauthorized,
    ErrorKind::UnsupportedProtocol,
    ErrorKind::RateLimited,
    ErrorKind::PayloadTooLarge,
];

/// 把协议错误转为 MCP 协议错误（资源读取等非工具调用路径）。
pub(crate) fn to_mcp_error(e: &ToolError) -> McpError {
    let mut data = json!({ "kind": e.kind });
    if let Some(d) = &e.details {
        data["details"] = d.clone();
    }
    match e.kind {
        ErrorKind::ResourceNotFound => McpError::resource_not_found(e.message.clone(), Some(data)),
        kind => McpError::new(ErrorCode(kind.code() as i32), e.message.clone(), Some(data)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::ResourcesReadResult;

    #[test]
    fn error_result_is_error_with_kind() {
        let r = error_result(&ToolError::new(ErrorKind::HandlerError, "坏了"));
        assert_eq!(r.is_error, Some(true));
        assert_eq!(r.content[0].as_text().unwrap().text, "HANDLER_ERROR: 坏了");
        assert_eq!(
            r.structured_content.unwrap()["error"]["kind"],
            "HANDLER_ERROR"
        );
    }

    #[test]
    fn success_result_with_hints() {
        let r = success_result(
            "shop",
            ToolsInvokeResult {
                data: json!({"ok": true}),
                state_hints: vec!["cart.state".into()],
                ..ToolsInvokeResult::default()
            },
            OutputShape::Undeclared,
        );
        assert_eq!(r.is_error, Some(false));
        assert_eq!(r.content.len(), 2);
        assert!(
            r.content[1]
                .as_text()
                .unwrap()
                .text
                .contains("app-mcp://shop/cart.state")
        );
        assert_eq!(r.structured_content, Some(json!({"ok": true})));
        let r = success_result(
            "shop",
            ToolsInvokeResult { data: json!([1]), ..ToolsInvokeResult::default() },
            OutputShape::Undeclared,
        );
        assert_eq!(r.structured_content, None);
        assert_eq!(r.content[0].as_text().unwrap().text, "[1]");
        assert_eq!(r.meta, None);
    }

    /// 第 19 项 R3：无返回值 → "已完成"、不填 structuredContent；有摘要时摘要代替。
    #[test]
    fn success_result_without_value() {
        let r = success_result("shop", ToolsInvokeResult::default(), OutputShape::Object);
        assert_eq!(r.content.len(), 1);
        assert_eq!(r.content[0].as_text().unwrap().text, "已完成");
        assert_eq!(r.structured_content, None);
        let r = success_result(
            "shop",
            ToolsInvokeResult { summary: Some("已加入购物车".into()), ..ToolsInvokeResult::default() },
            OutputShape::Undeclared,
        );
        assert_eq!(r.content.len(), 1);
        assert_eq!(r.content[0].as_text().unwrap().text, "已加入购物车");
    }

    /// 第 19 项 R1 + 第 14 项 S2：状态说明在前、_meta 带状态；内容标注只加在 App 的内容块上。
    #[test]
    fn success_result_status_and_annotations() {
        let r = success_result(
            "shop",
            ToolsInvokeResult {
                data: json!({"orderId": "o1"}),
                status: app_mcp_protocol::ResultStatus::Pending,
                state_resource: Some("order.state".into()),
                summary: Some("已提交，等待付款".into()),
                annotations: Some(app_mcp_protocol::ContentAnnotations { priority: Some(0.5), ..Default::default() }),
                state_hints: vec!["cart.state".into()],
            },
            OutputShape::Object,
        );
        let texts: Vec<&str> = r.content.iter().map(|c| c.as_text().unwrap().text.as_str()).collect();
        assert_eq!(texts.len(), 4, "{texts:?}");
        assert!(texts[0].contains("尚未完成") && texts[0].contains("app-mcp://shop/order.state"));
        assert_eq!(texts[1], "已提交，等待付款");
        assert_eq!(texts[2], r#"{"orderId":"o1"}"#);
        assert!(texts[3].contains("app-mcp://shop/cart.state"));
        let annotated: Vec<bool> = r.content.iter().map(|c| c.as_text().unwrap().annotations.is_some()).collect();
        assert_eq!(annotated, [false, true, true, false]);
        let meta = r.meta.unwrap();
        assert_eq!(meta.get("app-mcp/status"), Some(&json!("pending")));
        assert_eq!(meta.get("app-mcp/stateResource"), Some(&json!("app-mcp://shop/order.state")));
        assert_eq!(r.structured_content, Some(json!({"orderId": "o1"})));
        // noop 且无返回值：只有状态说明，不出现"已完成"
        let r = success_result(
            "shop",
            ToolsInvokeResult { status: app_mcp_protocol::ResultStatus::Noop, ..ToolsInvokeResult::default() },
            OutputShape::Undeclared,
        );
        assert_eq!(r.content.len(), 1);
        assert!(r.content[0].as_text().unwrap().text.contains("没有做任何改动"));
        // 非对象结果按声明包装
        let r = success_result(
            "shop",
            ToolsInvokeResult { data: json!(["a"]), ..ToolsInvokeResult::default() },
            OutputShape::Wrapped,
        );
        assert_eq!(r.structured_content, Some(json!({"result": ["a"]})));
    }

    #[cfg(feature = "mcp-server")]
    #[test]
    fn tool_conversion() {
        let info: ToolInfo = serde_json::from_value(json!({
            "name": "orders.search", "description": "搜索", "inputSchema": {"type": "object"}, "risk": "read", "title": "搜"
        }))
        .unwrap();
        let t = to_mcp_tool("shop", &info, Availability::NotRegistered);
        assert_eq!(t.name, "shop.orders.search");
        assert_eq!(t.description.as_deref(), Some("[当前不可用] 搜索"));
        assert_eq!(t.title.as_deref(), Some("搜"));
        assert_eq!(t.annotations.unwrap().read_only_hint, Some(true));
        assert!(t.output_schema.is_none());
        // 声明的注解逐字段优先，缺少的按 risk 推导；非对象 outputSchema 包装
        let declared: ToolInfo = serde_json::from_value(json!({
            "name": "orders.cancel", "description": "取消", "inputSchema": {"type": "object"}, "risk": "destructive",
            "annotations": {"idempotentHint": true, "openWorldHint": false, "title": "取消订单"},
            "outputSchema": {"type": "array"}
        }))
        .unwrap();
        let t = to_mcp_tool("shop", &declared, Availability::Available);
        assert_eq!(
            serde_json::to_value(t.annotations.unwrap()).unwrap(),
            json!({"title": "取消订单", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false})
        );
        assert_eq!(
            Value::Object((*t.output_schema.unwrap()).clone()),
            json!({"type": "object", "properties": {"result": {"type": "array"}}, "required": ["result"]})
        );
        let d = tool_declaration(&declared);
        assert_eq!((d.risk, d.output_schema, d.effective.destructive_hint), (Risk::Destructive, true, Some(true)));
        assert_eq!(d.annotations.unwrap().destructive_hint, None, "声明原样");
        let hd = app_hub_tool("shop", declared, Availability::Available);
        assert_eq!(hd.annotations.idempotent_hint, Some(true));
        assert_eq!(hd.output_schema, Some(json!({"type": "array"})));
        let h = app_hub_tool("shop", info, Availability::Available);
        assert_eq!(h.name, "shop.orders.search");
        assert_eq!(h.tool, "orders.search");
        assert_eq!(h.activation, Activation::Foreground);
    }

    #[test]
    fn resource_text_conversion() {
        let c = resource_contents(
            "u",
            None,
            ResourcesReadResult {
                contents: json!({"a": 1}),
                mime_type: None,
            },
        );
        assert_eq!(
            c,
            ResourceContents::text(r#"{"a":1}"#, "u").with_mime_type(DEFAULT_MIME)
        );
        let c = resource_contents(
            "u",
            Some("text/markdown"),
            ResourcesReadResult {
                contents: json!("# hi"),
                mime_type: None,
            },
        );
        assert_eq!(
            c,
            ResourceContents::text("# hi", "u").with_mime_type("text/markdown")
        );
        let rc = first_content("u", ReadResourceResult::new(vec![c]));
        assert_eq!(rc.text.as_deref(), Some("# hi"));
        assert_eq!(rc.mime_type.as_deref(), Some("text/markdown"));
    }

    #[test]
    fn builtins_and_upstream_risk() {
        let b = builtin_hub_tools(false);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].name, "apps.list");
        assert_eq!(b[0].tool, "list");
        let b = builtin_hub_tools(true);
        assert_eq!(b.len(), 4);
        assert_eq!(b[3].name, "apps.tools");
        assert!(builtin_schema("apps.select").is_some());
        // 未列出时 apps.tools 仍可调用
        assert!(builtin_schema("apps.tools").is_some());
        assert!(builtin_schema("apps.nope").is_none());
        let t = Tool::new("rm", "删除", obj(json!({"type": "object"})))
            .with_annotations(ToolAnnotations::new().destructive(true));
        assert_eq!(upstream_risk(&t), Risk::Destructive);
        let t = Tool::new("ls", "列出", obj(json!({"type": "object"})));
        assert_eq!(upstream_risk(&t), Risk::Write);
        assert_eq!(upstream_hub_tool("files", &t).name, "files.ls");
        assert_eq!(b[0].annotations.read_only_hint, Some(true));
        // 上游注解原样；没有注解时为空
        assert_eq!(upstream_hub_tool("files", &t).annotations, app_mcp_protocol::ToolAnnotations::default());
        let rm = Tool::new("rm", "删除", obj(json!({"type": "object"})))
            .with_annotations(ToolAnnotations::new().destructive(true).idempotent(true));
        let h = upstream_hub_tool("files", &rm);
        assert_eq!((h.annotations.destructive_hint, h.annotations.idempotent_hint), (Some(true), Some(true)));
        let d = upstream_tool_declaration(&rm);
        assert_eq!((d.name.as_str(), d.risk, d.output_schema), ("rm", Risk::Destructive, false));
        assert_eq!(d.annotations, Some(d.effective.clone()));
    }

    #[test]
    fn hub_is_send_sync_and_futures_are_send() {
        fn send_sync<T: Send + Sync>() {}
        fn send<F: Send>(_: &F) {}
        send_sync::<crate::Hub>();
        #[cfg(feature = "mcp-server")]
        send_sync::<crate::McpSession>();
        send_sync::<crate::HubEvent>();
        // 仅做类型检查，不运行。
        let _check = |hub: &crate::Hub| {
            send(&hub.call_tool(CallRequest::default()));
            send(&hub.dispatch(crate::ToolFormat::Anthropic, Value::Null));
            send(&hub.read_resource(""));
        };
    }

    #[test]
    fn values_and_errors() {
        assert_eq!(result_value(&json_result(json!({"a": 1}))), json!({"a": 1}));
        let r = CallToolResult::success(vec![ContentBlock::text("[1,2]")]);
        assert_eq!(result_value(&r), json!([1, 2]));
        let r = CallToolResult::success(vec![ContentBlock::text("hi")]);
        assert_eq!(result_value(&r), json!("hi"));
        let r = CallToolResult::success(vec![ContentBlock::text("a"), ContentBlock::text("b")]);
        assert_eq!(result_text(&r), "a\n\nb");
        let e = to_mcp_error(
            &ToolError::new(ErrorKind::AppDisconnected, "断了").with_details(json!({"x": 1})),
        );
        let back = mcp_error_to_tool(&e);
        assert_eq!(back.kind, ErrorKind::AppDisconnected);
        assert_eq!(back.details, Some(json!({"x": 1})));
        let e = McpError::new(ErrorCode(-32004), "拒绝", None);
        assert_eq!(mcp_error_to_tool(&e).kind, ErrorKind::UserRejected);
        let e = McpError::internal_error("x", None);
        assert_eq!(mcp_error_to_tool(&e).kind, ErrorKind::HandlerError);
    }
}
