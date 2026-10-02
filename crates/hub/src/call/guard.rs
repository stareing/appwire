//! 调用前后的检查：参数 / 结果大小上限与限流（spec/hub-api.md 3.11）、审批（3.3）、结果核对。

use app_mcp_protocol::{ErrorKind, ToolError, ToolsInvokeResult};
use serde_json::{Value, json};

use crate::hub::{HubShared, lock};
use crate::limits::{OutputValidation, Payload};
use crate::schema::{self, SchemaCheck};
use crate::tool_def::ToolDef;
use crate::types::{ApprovalRequest, HubTool};

use super::{CallCtx, CancelFut, cancelled};

impl HubShared {
    /// 转发前的资源保护（spec/hub-api.md 3.11）：参数大小上限，然后（App, 工具）与 App 两级限流。
    /// 超出时计入该 App 的拒绝计数，返回 `PAYLOAD_TOO_LARGE` / `RATE_LIMITED`。
    pub(crate) fn guard_call(&self, app_id: &str, tool: &str, args: &Value) -> Result<(), ToolError> {
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

    pub(super) fn approval_request(&self, call_id: &str, t: &HubTool, args: &Value, ctx: &CallCtx) -> ApprovalRequest {
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
            principal: ctx.principal.clone(),
            client_name: ctx.client_name.clone(),
        }
    }

    /// 按策略审批（spec/hub-api.md 3.3）：拒绝 / 超时 / 未设置处理器 → `USER_REJECTED`。
    pub(super) async fn approve(&self, req: ApprovalRequest, cancel: CancelFut<'_>) -> Result<(), ToolError> {
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

    /// 结果到达后的处理：大小上限 → 解析 → 按 [`OutputValidation`] 核对 `outputSchema`。
    pub(super) fn accept_result(&self, app_id: &str, tool: &str, info: &ToolDef, v: Value) -> Result<ToolsInvokeResult, ToolError> {
        let size = serde_json::to_vec(&v).map_or(0, |b| b.len());
        self.guard_payload(app_id, Payload::Result, size, &format!("工具「{app_id}.{tool}」"))?;
        let r = match serde_json::from_value::<ToolsInvokeResult>(v.clone()) {
            Ok(r) => r,
            Err(_) => ToolsInvokeResult { data: v, ..ToolsInvokeResult::default() },
        };
        let mode = self.config.output_validation;
        if mode == OutputValidation::Off || r.data.is_null() {
            return Ok(r);
        }
        let Some(schema) = info.output_schema() else {
            return Ok(r);
        };
        let msg = match schema::check(&schema, &r.data) {
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

    /// App 返回的错误：与结果一样受结果大小上限约束（`USER_ACTION_REQUIRED` 等错误的说明来自 App，spec/protocol.md 第 4 节），
    /// 超出时返回 `PAYLOAD_TOO_LARGE`；否则原样转为 [`ToolError`]。
    pub(crate) fn accept_error(&self, app_id: &str, tool: &str, rpc: &app_mcp_protocol::RpcError) -> ToolError {
        let size = serde_json::to_vec(rpc).map_or(0, |b| b.len());
        match self.guard_payload(app_id, Payload::Result, size, &format!("工具「{app_id}.{tool}」")) {
            Err(e) => e,
            Ok(()) => rpc.to_tool_error(),
        }
    }

}
