//! 调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」）：每个进行中的调用在 Hub 中有一个可查询、可取消的对象，
//! 状态为 `created → approving → activating → running`，调用结束即释放（不保留结果；需要脱离请求的长作业由 App 以
//! `status: "pending"` + `stateResource` 自行持久化，spec/protocol.md 3.2）。
//!
//! - Agent：内置工具 `apps.calls`（列出自己的进行中调用）/ `apps.cancel`（取消自己的调用），也出现在 `app-mcp://apps/self`。
//! - 厂商 / 机主：[`crate::HubStatus::calls`]（全部调用）与 [`crate::Hub::cancel_call`]。
//!
//! @security "自己"按调用方键判定（自身与其任务句柄，[`crate::hub_state::owned_by`]），与 `apps/self` 相同：取消他人的调用与
//! 不存在的调用回复相同，不泄露其他调用方的 callId。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError, ToolsProgressParams};
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::hub::{HubShared, lock};
use crate::hub_state::owned_by;
use crate::progress::ProgressUpdate;
use crate::task::CallerKey;

/// 进行中调用的阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallState {
    /// 已受理：解析名称、路由、策略与限流检查中。
    Created,
    /// 等待审批（[`crate::ApprovalHandler`] 正在询问用户）。
    Approving,
    /// 唤醒 App 或导航到工具所在页面中。
    Activating,
    /// 已交给 App / 上游 MCP 服务器 / 内置工具执行，等待结果。
    Running,
}

/// 调用最近报告的进度（只记最新一条，说明按 [`crate::progress::MAX_PROGRESS_MESSAGE_CHARS`] 截断）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallProgress {
    pub progress: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 一个进行中的调用（[`crate::HubStatus::calls`]、`apps.calls`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallStatus {
    pub call_id: String,
    /// 工具全名（`<appId>.<tool>`、`apps.*`）。
    pub name: String,
    /// 调用方键（只在 [`crate::HubStatus::calls`] 中给出；`apps.calls` 不含）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    /// 调用方的记账主体（`agent:<名>` / `local` / `api`）。
    pub subject: String,
    pub state: CallState,
    /// 从 Hub 受理起的毫秒数。
    pub elapsed_ms: u64,
    /// 执行该调用的 App 实例（`running` 且路由到 App 时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<CallProgress>,
    /// 诊断（第 4f 项 i）：执行该调用的实例最近上报的可见性（前台 / 后台 / 冻结）；不是调用状态的一部分，
    /// 只帮助判断"为什么还没完成"（如后台被系统限制）。实例未上报或调用不在 App 上执行时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_state: Option<app_mcp_protocol::Visibility>,
}

/// [`HubShared::calls`] 中的一项。
pub(crate) struct CallEntry {
    /// 登记序号：同名 callId 被新调用覆盖时，旧调用结束不误删新登记。
    pub token: u64,
    pub cancel: oneshot::Sender<()>,
    pub caller: CallerKey,
    pub name: String,
    pub state: CallState,
    pub started: Instant,
    pub instance_id: Option<String>,
    /// 执行该调用的 App 连接：只接受这条连接报告的进度。
    pub conn_id: Option<u64>,
    pub progress: Option<CallProgress>,
}

fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl CallEntry {
    fn status(&self, call_id: &str, now: Instant, with_caller: bool) -> CallStatus {
        CallStatus {
            call_id: call_id.to_owned(),
            name: self.name.clone(),
            caller: with_caller.then(|| self.caller.as_str().to_owned()),
            subject: self.caller.usage_subject(),
            state: self.state,
            elapsed_ms: millis(now.saturating_duration_since(self.started)),
            instance_id: self.instance_id.clone(),
            progress: self.progress.clone(),
            platform_state: None,
        }
    }
}

impl HubShared {
    /// 推进调用 `call_id` 的阶段；调用已结束或不存在时忽略。
    pub(crate) fn set_call_state(&self, call_id: &str, state: CallState) {
        if let Some(e) = lock(&self.calls).get_mut(call_id) {
            e.state = state;
        }
    }

    /// 调用已交给 App 实例 `instance_id`（连接 `conn_id`）执行。
    pub(crate) fn set_call_running(&self, call_id: &str, instance_id: &str, conn_id: u64) {
        if let Some(e) = lock(&self.calls).get_mut(call_id) {
            e.state = CallState::Running;
            e.instance_id = Some(instance_id.to_owned());
            e.conn_id = Some(conn_id);
        }
    }

    /// 记下 App 连接 `conn_id` 报告的进度（只认执行该调用的连接）。
    pub(crate) fn record_call_progress(&self, conn_id: u64, p: &ToolsProgressParams) {
        if let Some(e) = lock(&self.calls).get_mut(&p.call_id)
            && e.conn_id == Some(conn_id)
        {
            let u = ProgressUpdate::from(p.clone());
            e.progress = Some(CallProgress { progress: u.progress, total: u.total, message: u.message });
        }
    }

    /// 全部进行中的调用（含调用方键），按开始时刻排序。
    pub(crate) fn call_statuses(&self) -> Vec<CallStatus> {
        self.collect_calls(|_, _| true, true)
    }

    /// 按开始时刻排序的调用对象，并补上诊断字段 `platform_state`（先放开调用表锁再查注册表）。
    fn collect_calls(&self, keep: impl Fn(&str, &CallEntry) -> bool, with_caller: bool) -> Vec<CallStatus> {
        let now = Instant::now();
        let mut out: Vec<(Instant, CallStatus)> = lock(&self.calls)
            .iter()
            .filter(|(id, e)| keep(id, e))
            .map(|(id, e)| (e.started, e.status(id, now, with_caller)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.call_id.cmp(&b.1.call_id)));
        let registry = self.registry();
        out.into_iter()
            .map(|(_, mut s)| {
                s.platform_state = match (s.name.split_once('.'), s.instance_id.as_deref()) {
                    (Some((app_id, _)), Some(instance)) => registry.instance(app_id, instance).and_then(|i| i.visibility),
                    _ => None,
                };
                s
            })
            .collect()
    }

    /// `owner`（调用方键）及其任务句柄的进行中调用，不含 `except`（正在查询的这次调用），按开始时刻排序。
    pub(crate) fn own_calls(&self, owner: &CallerKey, except: Option<&str>) -> Vec<CallStatus> {
        self.collect_calls(|id, e| Some(id) != except && owned_by(e.caller.as_str(), owner.as_str()), false)
    }

    /// 取消 `owner` 自己（含其任务句柄）的进行中调用；不是自己的或不存在时返回 `false`。
    pub(crate) fn cancel_own_call(&self, owner: &CallerKey, call_id: &str) -> bool {
        let mut calls = lock(&self.calls);
        if !calls.get(call_id).is_some_and(|e| owned_by(e.caller.as_str(), owner.as_str())) {
            return false;
        }
        calls.remove(call_id).is_some_and(|e| e.cancel.send(()).is_ok())
    }

    /// 内置工具 `apps.calls`。
    pub(crate) fn builtin_calls(&self, caller: &CallerKey, current: Option<&str>) -> Result<CallToolResult, ToolError> {
        let calls = self.own_calls(caller, current);
        let message = if calls.is_empty() {
            "没有进行中的调用（本次查询除外）。".to_owned()
        } else {
            format!("{} 个进行中的调用。可用 apps.cancel 按 callId 取消。", calls.len())
        };
        Ok(crate::call::json_result(json!({ "calls": calls, "message": message })))
    }

    /// 内置工具 `apps.cancel`。
    pub(crate) fn builtin_cancel(self: &Arc<Self>, caller: &CallerKey, args: &Value) -> Result<CallToolResult, ToolError> {
        let call_id = args.get("callId").and_then(Value::as_str).unwrap_or_default();
        if self.cancel_own_call(caller, call_id) {
            return Ok(crate::call::json_result(json!({
                "callId": call_id,
                "cancelled": true,
                "message": format!("已取消调用 {call_id}；它的发起方会收到 CANCELLED。"),
            })));
        }
        Err(ToolError::new(
            ErrorKind::ToolNotFound,
            format!("没有属于你的进行中调用「{call_id}」（可能已经结束）。可调用 apps.calls 查看。"),
        )
        .with_details(json!({ "callId": call_id })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_hides_caller_key_unless_requested() {
        let (tx, _rx) = oneshot::channel();
        let e = CallEntry {
            token: 1,
            cancel: tx,
            caller: CallerKey::api(None),
            name: "shop.cart.add".into(),
            state: CallState::Running,
            started: Instant::now(),
            instance_id: Some("i1".into()),
            conn_id: Some(3),
            progress: Some(CallProgress { progress: 1.0, total: Some(4.0), message: None }),
        };
        let s = e.status("c1", e.started, false);
        assert_eq!(s.caller, None);
        assert_eq!(s.subject, "api");
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "running");
        assert!(v.get("caller").is_none());
        assert_eq!(e.status("c1", e.started, true).caller.as_deref(), Some("api"));
    }
}
