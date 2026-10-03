//! 任务句柄（第 12 项 S8 / 第 16 项 P1；spec/hub-api.md 3.6「任务句柄」）：同一无会话主体经 Hub 签发的任务 ID 同时运行
//! 多个互相隔离的 Agent 任务（各自的 `apps.select` 选择与租约）。
//!
//! 只提供机制：何时开、开几个、何时结束由 Agent 决定；Hub 只签发（≥128 位随机、不可猜测）、按名下数量设上限（B-07）、
//! 按请求流空闲回收（`HubConfig::task_idle_ttl`，与主体任务相同）。
//!
//! 句柄的两条通道（取值相同，同时出现且不同 → `INVALID_INPUT`）：
//! - **工具参数** `taskId`：只在 [`TASK_SCOPED_TOOLS`] 上（模型可写，主通道）；
//! - **请求 `_meta`** `dev.appwire/taskId`：任何工具调用（含 App 工具与上游工具；供自己实现客户端的 Agent 宿主）。
//!
//! `apps.task.begin` 不读任何通道（总是为主体签发新句柄）。legacy MCP 会话与 Hub API 已各有自己的任务，出示句柄 →
//! `INVALID_INPUT`（不静默忽略）。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::call::{CallCtx, json_result};
use crate::hub::{HubShared, lock};
use crate::lifecycle::SessionRequest;
use crate::names::{ARG_TASK_ID, META_TASK_ID, TASK_SCOPED_TOOLS, TOOL_APPS_TASK_BEGIN, TOOL_APPS_TASK_END};
use crate::task::{CallerKey, is_task_id};

/// 句柄不存在或已回收时错误 `data.reason` 的取值（可恢复：重新 `apps.task.begin`）。
pub(crate) const REASON_TASK_EXPIRED: &str = "task-expired";
/// 调用方不能使用任务句柄（legacy 会话、Hub API、未启用）时错误 `data.reason` 的取值（去掉 `taskId` 即可）。
pub(crate) const REASON_TASK_UNSUPPORTED: &str = "task-handle-unsupported";

/// 进入调用失败时的结果：`Ok` 为直接给出的成功结果（结束一个已不存在的句柄是幂等的），`Err` 为错误。
pub(crate) type EnterRejected = Result<CallToolResult, ToolError>;

fn invalid(message: String) -> ToolError {
    ToolError::new(ErrorKind::InvalidInput, message)
}

fn expired(task_id: &str, idle_ttl_secs: Option<f64>) -> ToolError {
    let why = match idle_ttl_secs {
        Some(s) => format!("空闲 {s} 秒后回收，或已调用 {TOOL_APPS_TASK_END}"),
        None => format!("已调用 {TOOL_APPS_TASK_END}，或 Hub 已重启"),
    };
    invalid(format!(
        "任务句柄「{task_id}」不存在或已被回收（{why}）。请调用 {TOOL_APPS_TASK_BEGIN} 取得新句柄后重试；原任务的实例选择与租约\
         已失效，需要时在新任务中重新 apps.select。"
    ))
    .with_details(json!({ "reason": REASON_TASK_EXPIRED, "taskId": task_id }))
}

fn unsupported(message: String) -> ToolError {
    invalid(message).with_details(json!({ "reason": REASON_TASK_UNSUPPORTED }))
}

/// 本次调用出示的任务句柄（尚未核对是否存在）。
///
/// @error `INVALID_INPUT`：参数 `taskId` 不是字符串；参数与 `_meta` 取值不同。
fn presented_task_id<'a>(ctx: &'a CallCtx, args: &'a Value) -> Result<Option<&'a str>, ToolError> {
    if ctx.name == TOOL_APPS_TASK_BEGIN {
        return Ok(None);
    }
    let param = match TASK_SCOPED_TOOLS.contains(&ctx.name.as_str()) {
        false => None,
        true => match args.get(ARG_TASK_ID) {
            None => None,
            Some(Value::String(id)) => Some(id.as_str()),
            Some(v) => return Err(invalid(format!("参数 {ARG_TASK_ID} 必须是字符串（{TOOL_APPS_TASK_BEGIN} 返回的 taskId），收到 {v}。"))),
        },
    };
    match (param, ctx.task_id.as_deref()) {
        (Some(p), Some(m)) if p != m => Err(invalid(format!(
            "参数 {ARG_TASK_ID}（{p}）与 _meta「{META_TASK_ID}」（{m}）指向不同的任务；只保留其中一个。"
        ))),
        (p, m) => Ok(p.or(m)),
    }
}

impl HubShared {
    fn idle_ttl_secs(&self) -> Option<f64> {
        let ttl = self.config.task_idle_ttl;
        (!ttl.is_zero()).then_some(ttl.as_secs_f64())
    }

    /// 进入一次调用：按出示的任务句柄确定调用方（替换 `ctx.caller` 与审批用的 `ctx.session`），并取得该调用方的请求活动守卫
    /// （调用进行中任务不会被空闲回收）。没有句柄时调用方不变。
    ///
    /// @error 见 [`presented_task_id`]；调用方不能使用句柄 / 未启用 → `INVALID_INPUT`（`reason: task-handle-unsupported`）；
    /// 格式不对 → `INVALID_INPUT`；句柄不存在、已回收或属于其他主体 → `INVALID_INPUT`（`reason: task-expired`，可恢复）。
    /// `apps.task.end` 出示已不存在的句柄 → 成功（`ended: false`）。
    pub(crate) fn enter_task(self: &Arc<Self>, mut ctx: CallCtx, args: &Value) -> Result<(CallCtx, SessionRequest), EnterRejected> {
        let Some(task_id) = presented_task_id(&ctx, args).map_err(Err)? else {
            let activity = self.session_request(&ctx.caller);
            return Ok((ctx, activity));
        };
        if !ctx.caller.can_own_handles() {
            return Err(Err(unsupported(format!(
                "任务句柄（{ARG_TASK_ID} / _meta「{META_TASK_ID}」）只用于无会话的 MCP 请求：本会话已有自己的任务（legacy MCP 会话一会话\
                 一任务；Hub API 用 CallRequest.session 区分）。去掉 {ARG_TASK_ID} 后重试。"
            ))));
        }
        if self.config.max_task_handles == 0 {
            return Err(Err(unsupported(format!("本 Hub 未启用任务句柄（max_task_handles = 0）。去掉 {ARG_TASK_ID} 后重试。"))));
        }
        if !is_task_id(task_id) {
            return Err(Err(invalid(format!(
                "「{task_id}」不是 Hub 签发的任务句柄（应形如 task-<32 位十六进制>，由 {TOOL_APPS_TASK_BEGIN} 返回）。"
            ))));
        }
        let key = CallerKey::task_handle(&ctx.caller, task_id);
        let gone = |shared: &Self| shared.agent_tasks().get(&key).is_none();
        if gone(self) {
            return Err(self.missing_task(&ctx.name, task_id));
        }
        let activity = self.session_request(&key);
        // 核对与取得守卫之间被空闲回收 / 结束：守卫留下的活动记录随即删除，按不存在处理。
        if gone(self) {
            drop(activity);
            lock(&self.leases).forget_session(key.as_str());
            return Err(self.missing_task(&ctx.name, task_id));
        }
        ctx.session = Some(key.to_string());
        ctx.caller = key;
        Ok((ctx, activity))
    }

    fn missing_task(&self, tool: &str, task_id: &str) -> EnterRejected {
        if tool == TOOL_APPS_TASK_END {
            return Ok(json_result(json!({
                "taskId": task_id,
                "ended": false,
                "released": 0,
                "message": format!("任务「{task_id}」不存在或已被回收，无需结束。"),
            })));
        }
        Err(expired(task_id, self.idle_ttl_secs()))
    }

    /// `apps.task.begin {}`：为调用方主体签发一个任务句柄。结果 `{taskId, idleTtlMs, message}`（`idleTtlMs` 为 `null` = 不因空闲回收）。
    ///
    /// @error 调用方不能使用句柄 / 未启用 → `INVALID_INPUT`（`reason: task-handle-unsupported`）；名下句柄已达
    /// `max_task_handles` → `RATE_LIMITED`（`data.limit`）。
    pub(crate) fn builtin_task_begin(self: &Arc<Self>, ctx: &CallCtx) -> Result<CallToolResult, ToolError> {
        if !self.task_handles_for(&ctx.caller) {
            return Err(unsupported(format!(
                "{TOOL_APPS_TASK_BEGIN} 只用于无会话的 MCP 请求且需启用任务句柄（max_task_handles > 0）：legacy MCP 会话一会话一任务，\
                 Hub API 用 CallRequest.session 区分，直接调用即可。"
            )));
        }
        let max = self.config.max_task_handles;
        let key = self.agent_tasks().begin_handle(&ctx.caller, max).map_err(|limit| {
            let idle = self.idle_ttl_secs().map_or_else(String::new, |s| format!("，或等待空闲任务被回收（{s} 秒）"));
            ToolError::new(
                ErrorKind::RateLimited,
                format!("同时存在的任务句柄已达上限（{limit} 个）。请对不再使用的任务调用 {TOOL_APPS_TASK_END}{idle}后重试。"),
            )
            .with_details(json!({ "limit": limit }))
        })?;
        // 记一次请求活动：空闲回收从签发时算起（没有活动记录的任务会被立即回收）。
        drop(self.session_request(&key));
        let task_id = key.handle_task_id().unwrap_or_default().to_owned();
        let idle = match self.idle_ttl_secs() {
            Some(s) => format!("空闲 {s} 秒后回收（之后再用会得到错误，可重新 {TOOL_APPS_TASK_BEGIN}）"),
            None => "不因空闲回收".to_owned(),
        };
        Ok(json_result(json!({
            "taskId": task_id,
            "idleTtlMs": self.idle_ttl_secs().map(|_| u64::try_from(self.config.task_idle_ttl.as_millis()).unwrap_or(u64::MAX)),
            "message": format!(
                "已创建任务 {task_id}。在 {} 的参数中带 {ARG_TASK_ID}: \"{task_id}\" 即在该任务中操作：实例选择与租约只属于该任务，\
                 与其他任务及不带 {ARG_TASK_ID} 的调用互不影响；工具列表不变。任务{idle}；用完可调用 {TOOL_APPS_TASK_END}。\
                 App 工具的参数由 App 定义、不带 {ARG_TASK_ID}，按不带任务的默认规则路由。",
                TASK_SCOPED_TOOLS.iter().filter(|n| **n != TOOL_APPS_TASK_END).copied().collect::<Vec<_>>().join(" / "),
            ),
        })))
    }

    /// `apps.task.end {taskId}`：结束该任务——收回其全部租约（其他调用方的未到期租约随后补发）、清除其选择。
    /// 结果 `{taskId, ended, released, message}`（`released` = 结束时仍未到期的租约数）。句柄已不存在时见 [`Self::enter_task`]。
    pub(crate) fn builtin_task_end(&self, ctx: &CallCtx) -> Result<CallToolResult, ToolError> {
        let Some(task_id) = ctx.caller.handle_task_id().map(str::to_owned) else {
            // 参数必填（inputSchema）且已在 enter_task 中解析为句柄调用方；到这里说明调用方不能使用句柄。
            return Err(unsupported(format!("{TOOL_APPS_TASK_END} 需要无会话 MCP 请求出示的 {ARG_TASK_ID}。")));
        };
        let now = tokio::time::Instant::now();
        let released = self
            .agent_tasks()
            .get(&ctx.caller)
            .map_or(0, |t| t.leases.values().filter(|l| l.expires().is_some_and(|e| e > now)).count());
        self.end_task(&ctx.caller);
        Ok(json_result(json!({
            "taskId": task_id,
            "ended": true,
            "released": released,
            "message": format!("任务 {task_id} 已结束：其实例选择已清除，{released} 个租约已收回。该句柄不能再使用。"),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(name: &str, meta_task: Option<&str>) -> CallCtx {
        let mut c = CallCtx::from_request(crate::CallRequest::new(name, json!({})));
        c.task_id = meta_task.map(str::to_owned);
        c
    }

    const A: &str = "task-0123456789abcdef0123456789abcdef";
    const B: &str = "task-fedcba9876543210fedcba9876543210";

    type Case<'a> = (&'a str, Option<&'a str>, &'a Value, Result<Option<&'a str>, ()>);

    /// 通道表：参数只在接受 taskId 的内置工具上读；`_meta` 对任何工具生效；两者相同可并存、不同即冲突；begin 不读。
    #[test]
    fn presented_task_id_channels() {
        let args = json!({ ARG_TASK_ID: A });
        let cases: [Case<'_>; 9] = [
            ("apps.select", None, &args, Ok(Some(A))),
            ("apps.task.end", None, &args, Ok(Some(A))),
            ("apps.select", Some(A), &args, Ok(Some(A))),
            ("apps.select", Some(B), &args, Err(())),
            ("apps.select", Some(B), &Value::Null, Ok(Some(B))),
            // App 工具的 taskId 参数属于 App，不读；_meta 生效
            ("shop.cart.add", None, &args, Ok(None)),
            ("shop.cart.add", Some(B), &args, Ok(Some(B))),
            ("apps.tools", None, &args, Ok(None)),
            ("apps.task.begin", Some(B), &args, Ok(None)),
        ];
        for (name, meta, a, want) in cases {
            let c = ctx(name, meta);
            let got = presented_task_id(&c, a);
            match want {
                Ok(v) => assert_eq!(got.as_ref().ok().copied(), Some(v), "{name} {meta:?}"),
                Err(()) => assert_eq!(got.map_err(|e| e.kind), Err(ErrorKind::InvalidInput), "{name} {meta:?}"),
            }
        }
        let c = ctx("apps.select", None);
        assert_eq!(presented_task_id(&c, &json!({ ARG_TASK_ID: 5 })).map_err(|e| e.kind), Err(ErrorKind::InvalidInput));
    }

    #[test]
    fn expired_error_is_recoverable() {
        let e = expired(A, Some(600.0));
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        assert!(e.message.contains(TOOL_APPS_TASK_BEGIN) && e.message.contains("600"), "{}", e.message);
        assert_eq!(e.details, Some(json!({ "reason": REASON_TASK_EXPIRED, "taskId": A })));
        assert!(!expired(A, None).message.contains("空闲"));
    }
}
