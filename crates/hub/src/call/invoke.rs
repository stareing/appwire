//! App 工具的调用：路由 → schema 校验 → 审批 → 转发（含唤醒、导航与进度）。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError, ToolsCancelParams, ToolsInvokeParams, ToolsInvokeResult, method};
use serde_json::{Value, json};

use crate::hub::HubShared;
use crate::mcp_convert::OutputShape;
use crate::registry::WakeTargetPresence;
use crate::schema::{self, SchemaCheck};
use crate::types::Availability;

use super::{CallCtx, CancelFut, ToolRun, cancelled};
use super::tool_convert::app_hub_tool;

impl HubShared {
    /// 调用 App 工具：路由 → schema 校验 → 审批 → 转发。
    pub(super) async fn invoke_tool(
        self: &Arc<Self>,
        call_id: &str,
        app_id: &str,
        tool_name: &str,
        arguments: Value,
        ctx: &CallCtx,
        cancel: CancelFut<'_>,
    ) -> ToolRun {
        let mut output_shape = OutputShape::Undeclared;
        let mut woke = false;
        let (result, instance_id) = self
            .invoke_routed(call_id, app_id, tool_name, arguments, ctx, cancel, &mut output_shape, &mut woke)
            .await;
        ToolRun { result, instance_id, output_shape, woke }
    }

    /// [`HubShared::invoke_tool`] 的主体：返回结果与目标实例；路由到实例后把其 `outputSchema` 形式写入 `output_shape`，
    /// 经历唤醒（[`ToolRun::woke`]）时把 `woke` 置真。
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
        woke: &mut bool,
    ) -> (Result<ToolsInvokeResult, ToolError>, Option<String>) {
        self.lease_call_started(&ctx.caller, app_id);
        let selected = ctx
            .instance_id
            .clone()
            .or_else(|| self.selected_for(&ctx.caller, app_id));
        // 休眠实例 / 未运行的 App：先按快照（或清单）定义校验并审批，再唤醒（spec/lifecycle.md §9）。
        // 注意：先放开注册表锁再解析唤醒描述（resolve_wake_descriptor 会再次加锁）。
        let plan = self.registry().wake_plan_tool(
            app_id,
            tool_name,
            selected.as_deref(),
            ctx.instance_id.is_some(),
        );
        let plan = plan.filter(|p| {
            p.instance_id.is_some() || self.named_route(&p.app_id).is_some() || self.resolve_wake_descriptor(p).is_some()
        });
        let mut woken = None;
        // 已按快照 / 目录定义审批过（之后路由到实例时不再审批）。
        let mut approved = false;
        if let Some(plan) = plan {
            // 不唤醒（`waker: none`）：不做审批，直接按未连接返回（带 launchUrl）。
            if !self.wake_reachable(&plan) {
                return (Err(self.registry().disconnected_error(app_id)), plan.instance_id.clone());
            }
            if let Err(e) = self.admit_wake(app_id, Some(tool_name), &ctx.caller) {
                return (Err(e), plan.instance_id.clone());
            }
            if let Some(tool) = &plan.tool {
                if let SchemaCheck::Invalid(msg) = schema::check_json(tool.input_schema_json(), &arguments) {
                    return (
                        Err(ToolError::new(
                            ErrorKind::InvalidInput,
                            format!("参数不符合工具「{app_id}.{tool_name}」的 inputSchema：{msg}"),
                        )),
                        plan.instance_id.clone(),
                    );
                }
                let hub_tool = app_hub_tool(app_id, tool, Availability::Dormant);
                let req = self.approval_request(call_id, &hub_tool, &arguments, ctx);
                if let Err(e) = self.approve(req, cancel.as_mut()).await {
                    return (Err(e), plan.instance_id.clone());
                }
                approved = true;
            }
            // @why 审批期间目标可能已自行连上，此时 wake_and_wait 直接返回、不算唤醒。
            let absent = matches!(
                self.registry().wake_target_presence(app_id, plan.instance_id.as_deref()),
                WakeTargetPresence::Absent
            );
            match self.wake_and_wait(&plan, cancel.as_mut()).await {
                Ok(id) => {
                    *woke = absent;
                    woken = Some(id);
                }
                Err(e) => return (Err(e), plan.instance_id.clone()),
            }
        }
        // 页面目录（spec/hub-api.md 3.14）：没有实例注册该工具、而目录中有 →（休眠则先唤醒）→ 导航 → 等待注册。
        if let Some(target) = self.page_of_tool(app_id, tool_name) {
            match self
                .reach_page_tool(call_id, app_id, &target, &arguments, ctx, selected.as_deref(), woken.clone(), approved, cancel.as_mut())
                .await
            {
                Ok((id, page_woke)) => {
                    *woke |= page_woke;
                    woken = Some(id);
                    approved = true;
                }
                Err(e) => return (Err(e), woken),
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
        *output_shape = OutputShape::of(target.tool.output_schema().as_ref());
        match schema::check_json(target.tool.input_schema_json(), &arguments) {
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

        if !approved {
            let hub_tool = app_hub_tool(app_id, &target.tool, Availability::Available);
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
            idempotency_key: ctx.idempotency_key.clone(),
            priority: ctx.priority,
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
        // 先登记进度路由再发请求：App 收到请求后立即报告的进度也能送达。
        let mut progress = ctx.progress.as_ref().map(|sink| self.watch_progress(call_id, conn.id, sink.clone()));
        let (req_id, rx) = match conn.start_request(method::TOOLS_INVOKE, params) {
            Ok(v) => v,
            Err(_) => return (Err(disconnected()), instance),
        };
        tracing::info!(cid = %conn.cid, call_id, app_id, tool = tool_name, instance_id = %target.instance_id, woke = *woke, "转发工具调用");
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

        let response = tokio::time::timeout(response_timeout, rx);
        tokio::pin!(response);
        let outcome = loop {
            let flush_at = progress.as_ref().and_then(|p| p.flush_deadline());
            tokio::select! {
                r = &mut response => break r,
                _ = cancel.as_mut() => {
                    send_cancel("cancelled by MCP client");
                    return (Err(cancelled()), instance);
                }
                Some(p) = async { progress.as_mut()?.rx.recv().await }, if progress.is_some() => {
                    if let Some(w) = progress.as_mut() {
                        w.offer(p);
                    }
                }
                _ = tokio::time::sleep_until(flush_at.unwrap_or_else(tokio::time::Instant::now)), if flush_at.is_some() => {
                    if let Some(w) = progress.as_mut() {
                        w.flush();
                    }
                }
            }
        };
        drop(progress);
        let result = match outcome {
            Ok(Ok(Ok(v))) => self.accept_result(app_id, tool_name, &target.tool, v, &ctx.caller),
            Ok(Ok(Err(rpc))) => Err(self.accept_error(app_id, tool_name, &rpc, Some(&ctx.caller))),
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
        self.grant_lease(&ctx.caller, app_id, &conn);
        (result, instance)
    }

}
