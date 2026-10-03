//! 调用分派：生成 callId、登记取消、按上游 → 内置工具 → App 工具的顺序执行，以及转发给上游 MCP 服务器。

use std::future::Future;
use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::{ErrorData as McpError, Peer, RoleClient, ServiceError};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;

use crate::hub::{HubShared, lock};
use crate::limits::Payload;
use crate::mcp_convert::OutputShape;
use crate::navigate;
use crate::usage::UsageEvent;

use super::{Body, CallCtx, CancelFut, Invocation, ToolRun, cancelled, unknown_app};
use super::builtin_defs::builtin_schema;
use super::tool_convert::upstream_hub_tool;
use super::results::error_result;

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
        let started = tokio::time::Instant::now();
        let call_id = ctx.call_id.clone().unwrap_or_else(|| self.new_call_id());
        // 任务句柄（参数 taskId / _meta）决定调用方；活动守卫持有到调用结束（进行中的任务不被空闲回收）。
        let args = ctx.arguments.clone();
        let (ctx, _activity) = match self.enter_task(ctx, &args) {
            Ok(entered) => entered,
            Err(body) => {
                let mut inv = Invocation::bare(&call_id, None, Body::Builtin(body));
                inv.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                return inv;
            }
        };
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
        let mut inv = self.call_inner(&call_id, ctx, combined.as_mut()).await;
        inv.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
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
        let inv = |app_id: Option<&str>, body: Body| Invocation::bare(call_id, app_id, body);

        if let Some(Err(e)) = ctx.idempotency_key.as_deref().map(crate::request_meta::check_idempotency_key) {
            return inv(None, Body::Builtin(Err(e)));
        }

        // 策略：整体隐藏的 App / 上游与不存在的 appId 相同（spec/hub-api.md 3.13）。
        if let Some((app_id, _)) = name.split_once('.')
            && builtin_schema(&name).is_none()
            && self.app_hidden_hit(app_id)
        {
            return inv(Some(app_id), Body::NotFound(unknown_app(app_id)));
        }

        // 上游 MCP 服务器
        if let Some((app_id, tool)) = name.split_once('.')
            && let Some(peer) = self.upstream_peer(app_id)
        {
            let guard = if args.is_object() {
                self.admit_call(app_id, tool, &args, &ctx)
            } else {
                Ok(())
            };
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
                            self.call_upstream(app_id, tool, map, peer, &ctx, cancel.as_mut())
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
                out.overview = self.attach_overview(&ctx.caller, app_id);
            }
            self.expose_in_session(&ctx, app_id);
            return out;
        }

        // 内置工具
        if let Some(r) = self.call_builtin(&ctx, &name, &args) {
            return inv(None, Body::Builtin(r));
        }
        if let Some(r) = self.call_control_builtin(&ctx, &name, &args, cancel.as_mut()).await {
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
            return inv(Some(app_id), Body::NotFound(unknown_app(app_id)));
        }
        if let Err(e) = self.check_call_policy(app_id, tool, ctx.agent()).and_then(|()| self.check_app_lock(app_id, Some(tool), &ctx.caller)) {
            return inv(Some(app_id), Body::App(Err(e)));
        }
        // 后台替代（spec/hub-api.md 3.14）：view 工具够不着且已知 App 在后台 → 直接改调声明的 app 工具；
        // 否则照常（导航），导航因 App 不能自行回到前台被拒（USER_ACTION_REQUIRED / foreground）时再改调。
        let prefer = ctx.instance_id.clone().or_else(|| self.selected_for(&ctx.caller, app_id));
        let mut routed_to = self
            .background_alternative(app_id, tool, &args)
            .filter(|_| self.app_in_background(app_id, prefer.as_deref(), ctx.instance_id.is_some()));
        let fallback_args = routed_to.is_none().then(|| args.clone());
        let mut run = match routed_to.clone() {
            Some(alt) => self.run_routed_tool(call_id, app_id, &alt, args, &ctx, cancel.as_mut()).await,
            None => {
                if let Err(e) = self.guard_call(app_id, tool, &args, &ctx.caller) {
                    let mut out = inv(Some(app_id), Body::App(Err(e)));
                    out.overview = self.attach_overview(&ctx.caller, app_id);
                    return out;
                }
                self.invoke_tool(call_id, app_id, tool, args, &ctx, cancel.as_mut()).await
            }
        };
        if let (Some(args), Err(e)) = (fallback_args, &run.result)
            && navigate::needs_foreground(e)
            && let Some(alt) = self.background_alternative(app_id, tool, &args)
        {
            let first_woke = run.woke;
            run = self.run_routed_tool(call_id, app_id, &alt, args, &ctx, cancel.as_mut()).await;
            run.woke |= first_woke;
            routed_to = Some(alt);
        }
        let mut out = inv(Some(app_id), Body::App(run.result));
        out.instance_id = run.instance_id;
        out.output_shape = run.output_shape;
        out.woke = run.woke;
        out.routed_to = routed_to.map(|alt| format!("{app_id}.{alt}"));
        out.overview = self.attach_overview(&ctx.caller, app_id);
        self.expose_in_session(&ctx, app_id);
        out
    }

    /// 改调后台替代（spec/hub-api.md 3.14）：策略 `call` 执行点与资源保护按被改调的工具执行，之后与直接调用它相同。
    async fn run_routed_tool(
        self: &Arc<Self>,
        call_id: &str,
        app_id: &str,
        tool: &str,
        args: Value,
        ctx: &CallCtx,
        cancel: CancelFut<'_>,
    ) -> ToolRun {
        tracing::info!(app_id, tool, "App 在后台，改调 view 工具声明的后台替代");
        if let Err(e) = self.admit_call(app_id, tool, &args, ctx) {
            return ToolRun { result: Err(e), instance_id: None, output_shape: OutputShape::Undeclared, woke: false };
        }
        self.invoke_tool(call_id, app_id, tool, args, ctx, cancel).await
    }

    /// 转发前的准入（顺序即优先级）：策略 `call` 执行点 → 对象锁 → 资源保护（大小上限、限流与记账）。
    fn admit_call(&self, app_id: &str, tool: &str, args: &Value, ctx: &CallCtx) -> Result<(), ToolError> {
        self.check_call_policy(app_id, tool, ctx.agent())?;
        self.check_app_lock(app_id, Some(tool), &ctx.caller)?;
        self.guard_call(app_id, tool, args, &ctx.caller)
    }

    /// 渐进暴露：把 App 加入调用方会话的工具列表；列表因此变化时通知该 MCP 会话。
    pub(super) fn expose_in_session(self: &Arc<Self>, ctx: &CallCtx, app_id: &str) {
        if self.expose_app(&ctx.caller, app_id)
            && let Some(id) = ctx.mcp_session
        {
            self.notify_session_tools_changed(id);
        }
    }

    /// 转发给上游 MCP 服务器；结果只检查大小（不核对 `outputSchema`，上游自己负责其 schema，spec/hub-api.md 3.2）后原样返回，
    /// 协议错误原样透传。
    async fn call_upstream(
        &self,
        name: &str,
        tool: &str,
        args: Map<String, Value>,
        peer: Option<Peer<RoleClient>>,
        ctx: &CallCtx,
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
        let timeout = ctx.timeout.unwrap_or(self.config.response_timeout);
        tokio::select! {
            r = tokio::time::timeout(timeout, peer.call_tool(params)) => match r {
                Ok(Ok(result)) => {
                    let size = serde_json::to_vec(&result).map_or(0, |v| v.len());
                    self.record_usage(&ctx.caller, name, UsageEvent::Result { bytes: size as u64 });
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
}
