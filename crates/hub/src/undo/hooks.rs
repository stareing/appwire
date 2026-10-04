//! 撤销接入调用：App 工具结果的登记（[`HubShared::register_undo`]）与 `apps.undo` 的转发（[`HubShared::call_undo`]）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use app_mcp_protocol::{CallPriority, ErrorKind, ResultStatus, ToolError, ToolsInvokeResult, UndoAction};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::call::{Body, CallCtx, CancelFut, Invocation};
use crate::hub::HubShared;
use crate::task::CallerKey;
use crate::types::HubTool;

use super::{UndoGrant, UndoRecord};

/// 结果状态是否允许登记撤销（spec/protocol.md 3.8：`pending` 尚未完成、`noop` 无改动）。
fn undoable_status(status: ResultStatus) -> bool {
    matches!(status, ResultStatus::Done | ResultStatus::Partial)
}

/// 找不到可撤销的记录：与不存在、已过期、已撤销、属于其他任务同构（不泄露他人调用，与 `apps.cancel` 相同）。
fn no_record(call_id: Option<&str>) -> ToolError {
    let message = match call_id {
        Some(id) => format!("没有属于你的可撤销调用「{id}」（可能不存在、已过期或已撤销）。"),
        None => "你（本任务）没有可撤销的调用（可能都已过期或已撤销）。".to_owned(),
    };
    ToolError::new(ErrorKind::ToolNotFound, message).with_details(json!({ "callId": call_id }))
}

/// 逆调用失败：错误原样，`data.undo` 为 `{tool: <全名>, arguments}`（记录已取出，是否重试由 Agent 决定）。
fn with_undo_details(mut e: ToolError, tool: &str, arguments: &Value) -> ToolError {
    let undo = json!({ "tool": tool, "arguments": arguments });
    match &mut e.details {
        Some(Value::Object(m)) => {
            m.insert("undo".to_owned(), undo);
        }
        None => e.details = Some(json!({ "undo": undo })),
        // @compat 非对象的 details（不应出现）原样保留，不改写其形状。
        Some(_) => {}
    }
    e
}

type BoxedCall<'a> = Pin<Box<dyn Future<Output = Invocation> + Send + 'a>>;

impl HubShared {
    /// 撤销是否开启（[`crate::HubConfig::undo`] 的 `max_per_task > 0`）。
    pub(crate) fn undo_enabled(&self) -> bool {
        self.config.undo.enabled()
    }

    /// App 工具 `app_id.tool` 的已知定义声明了 `undoable`（上游工具与未知工具为 `false`）。
    pub(crate) fn tool_undoable(&self, app_id: &str, tool: &str) -> bool {
        self.registry().app_tool(app_id, tool).is_some_and(|d| d.undoable)
    }

    /// `apps.tools` 的工具条目：声明了 `undoable` 的追加 `undoable: true`（spec/hub-api.md 3.23；Hub API `HubTool` 二期补齐）。
    pub(crate) fn tool_entries(&self, tools: &[HubTool]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                let mut v = serde_json::to_value(t).unwrap_or(Value::Null);
                if self.tool_undoable(&t.app_id, &t.tool)
                    && let Value::Object(m) = &mut v
                {
                    m.insert("undoable".to_owned(), Value::Bool(true));
                }
                v
            })
            .collect()
    }

    /// App 工具调用结果的登记（spec/hub-api.md 3.23）：成功、`done` / `partial`、带合法 `undo` 且撤销开启时登记到调用方的任务，
    /// 返回写进结果 `_meta` 的内容；否则 `None`。不合法的 `undo` 只记 warn 日志，结果照常。
    pub(crate) fn register_undo(
        &self,
        caller: &CallerKey,
        call_id: &str,
        app_id: &str,
        instance_id: Option<&str>,
        result: &Result<ToolsInvokeResult, ToolError>,
    ) -> Option<UndoGrant> {
        let limits = self.config.undo;
        let Ok(r) = result else { return None };
        let raw = r.undo.as_ref()?;
        if !limits.enabled() || !undoable_status(r.status) {
            return None;
        }
        let action = match UndoAction::parse(raw) {
            Ok(a) => a,
            Err(error) => {
                tracing::warn!(app_id, call_id, %error, "结果中的 undo 不合法，忽略（结果照常）");
                return None;
            }
        };
        let UndoAction { tool, arguments, label } = action;
        let record = UndoRecord {
            call_id: call_id.to_owned(),
            app_id: app_id.to_owned(),
            instance_id: instance_id.map(str::to_owned),
            tool,
            arguments,
            label: label.clone(),
            at: Instant::now(),
        };
        self.agent_tasks().entry(caller).undo.push(record, limits.ttl, limits.max_per_task);
        let expires_in_ms = u64::try_from(limits.ttl.as_millis()).unwrap_or(u64::MAX);
        Some(UndoGrant { label, expires_in_ms })
    }

    /// `apps.undo {callId?}`：从调用方任务取出记录（只能取出一次），以同一调用方对 `<appId>.<逆工具>` 发起普通 App 工具调用，
    /// 逆调用的结果原样返回并标 `undoOf`。参数已按内置 inputSchema 校验；撤销关闭时不会到这里（`TOOL_NOT_FOUND`）。
    ///
    /// @why 逆调用沿用 `apps.undo` 本身的 callId（对原调用而言是新 callId）：结果 `_meta.callId`、`apps.calls` / `apps.cancel`
    /// 与逆调用结果再登记的撤销（"重做"）都指向同一个 ID。
    /// @error 记录不存在 / 已过期 / 已撤销 / 属于其他任务 → `TOOL_NOT_FOUND`（`data.callId`）；逆调用失败 → 其错误原样，
    /// `data.undo` 为 `{tool, arguments}`。
    ///
    /// @why 调用路径经 apps.undo 重入 [`HubShared::call`]（异步递归）：返回装箱的 `Send` future，打断 `Send` 推断的环。
    pub(crate) fn call_undo<'a>(
        self: &'a Arc<Self>,
        call_id: &'a str,
        ctx: &'a CallCtx,
        args: &'a Value,
        cancel: CancelFut<'a>,
    ) -> BoxedCall<'a> {
        Box::pin(self.undo_inner(call_id, ctx, args, cancel))
    }

    async fn undo_inner(self: &Arc<Self>, call_id: &str, ctx: &CallCtx, args: &Value, cancel: CancelFut<'_>) -> Invocation {
        let wanted = args.get("callId").and_then(Value::as_str);
        let ttl = self.config.undo.ttl;
        let now = Instant::now();
        let record = self.agent_tasks().get_mut(&ctx.caller).and_then(|t| t.undo.take(wanted, ttl, now));
        let Some(record) = record else {
            return Invocation::bare(call_id, None, Body::Builtin(Err(no_record(wanted))));
        };
        let tool = format!("{}.{}", record.app_id, record.tool);
        let online = record
            .instance_id
            .clone()
            .filter(|id| self.registry().instance(&record.app_id, id).is_some());
        let inverse = CallCtx {
            name: tool.clone(),
            arguments: record.arguments.clone(),
            caller: ctx.caller.clone(),
            session: ctx.session.clone(),
            instance_id: online,
            timeout: ctx.timeout,
            call_id: Some(call_id.to_owned()),
            mcp_session: ctx.mcp_session,
            progress: ctx.progress.clone(),
            idempotency_key: None,
            principal: ctx.principal.clone(),
            client_name: ctx.client_name.clone(),
            task_id: None,
            priority: CallPriority::Normal,
            cache_bypass: false,
        };
        tracing::info!(call_id, undo_of = %record.call_id, %tool, "撤销：转发逆调用");
        let mut inv = self.call(inverse, cancel).await;
        inv.body = match inv.body {
            Body::NotFound(e) => Body::NotFound(with_undo_details(e, &tool, &record.arguments)),
            Body::Builtin(Err(e)) => Body::Builtin(Err(with_undo_details(e, &tool, &record.arguments))),
            Body::App(Err(e)) => Body::App(Err(with_undo_details(e, &tool, &record.arguments))),
            other => other,
        };
        inv.undo_of = Some(record.call_id);
        inv
    }
}
