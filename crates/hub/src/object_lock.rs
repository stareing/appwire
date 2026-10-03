//! 对象锁（第 16 项 N6；spec/hub-api.md 3.6「对象锁」）：`apps.lock` / `apps.unlock` 与写调用转发前的 App 锁检查。
//!
//! 锁存放在持有者的 Agent 任务上（[`crate::task::LockTarget`]），任务结束即释放；到期在取用时判定，不设定时器。只提供机制：
//! 冲突时返回 `LOCKED`，Hub 不排队、不代为重试。
//!
//! @security 冲突错误与 `holder` 只给持有者的记账主体（`agent:<名>` / `local` / `api`），不给任务 ID（句柄是凭据）或会话号。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::call::{CallCtx, json_result, unknown_app};
use crate::hub::HubShared;
use crate::names::{BUILTIN_APP_ID, TOOL_APPS_LOCK, TOOL_APPS_UNLOCK};
use crate::task::{CallerKey, LockRefusal, LockTarget};

/// `apps.lock` 缺省的有效期。
///
/// @why 60 s：覆盖一段连续的多步操作（每步一次工具调用）；Agent 崩溃时 App 至多被锁一分钟。更长的操作由 Agent 续期。
pub const DEFAULT_LOCK_TTL: Duration = Duration::from_secs(60);
/// `ttlMs` 的下限与上限（毫秒）。
pub const MIN_LOCK_TTL_MS: u64 = 1_000;
pub const MAX_LOCK_TTL_MS: u64 = 600_000;
/// 命名锁 `key` 的最大长度（字符）。
pub const MAX_LOCK_KEY_LEN: usize = 128;

/// 一把未到期的锁（`/status` 的 `locks[]`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockStatus {
    pub app_id: String,
    /// 命名锁的名字；App 锁为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// 持有者的调用方键（`/status` 只对本机令牌开放）。
    pub caller: String,
    /// 持有者的记账主体：`agent:<名>` / `local` / `api`。
    pub holder: String,
    pub expires_in_ms: u64,
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn describe(target: &LockTarget) -> String {
    match &target.key {
        None => format!("App「{}」", target.app_id),
        Some(key) => format!("App「{}」的对象「{key}」", target.app_id),
    }
}

/// `LOCKED`：`target` 由 `holder` 持有，还剩 `remaining`；`caller` 为被拒绝的调用方。
///
/// @why 同一主体的另一个任务（如主体任务与其任务句柄）持有时单独措辞：只写主体名，Agent 会误以为是自己锁了自己。
fn locked_error(target: &LockTarget, holder: &CallerKey, caller: &CallerKey, remaining: Duration, what: &str) -> ToolError {
    let subject = holder.usage_subject();
    let secs = remaining.as_secs_f64().ceil();
    let message = if subject == caller.usage_subject() {
        format!(
            "{} 正被同为 {subject} 的另一个任务锁定（还剩约 {secs} 秒），{what}。锁按任务持有：请用持有锁的任务句柄（taskId）调用，\
             或在该任务中 apps.unlock 后重试。",
            describe(target)
        )
    } else {
        format!(
            "{} 正被 {subject} 锁定（还剩约 {secs} 秒），{what}。可等锁到期后重试、改做只读操作，或请用户协调。",
            describe(target)
        )
    };
    ToolError::new(ErrorKind::Locked, message)
    .with_details(json!({
        "appId": target.app_id,
        "key": target.key,
        "holder": subject,
        "retryAfterMs": millis(remaining),
    }))
}

/// 参数中的锁对象（`appId` 必填，`key` 可选）。
///
/// @error `key` 为空、超过 [`MAX_LOCK_KEY_LEN`] 或含控制字符 → `INVALID_INPUT`。
fn target_arg(args: &Value) -> Result<LockTarget, ToolError> {
    let app_id = args.get("appId").and_then(Value::as_str).unwrap_or_default().to_owned();
    let key = args.get("key").and_then(Value::as_str).map(str::to_owned);
    if let Some(k) = &key
        && (k.is_empty() || k.chars().count() > MAX_LOCK_KEY_LEN || k.chars().any(char::is_control))
    {
        return Err(ToolError::new(
            ErrorKind::InvalidInput,
            format!("key 需为 1–{MAX_LOCK_KEY_LEN} 个字符且不含控制字符。"),
        ));
    }
    Ok(LockTarget { app_id, key })
}

/// 参数中的有效期。
///
/// @error 不在 [`MIN_LOCK_TTL_MS`]..=[`MAX_LOCK_TTL_MS`] → `INVALID_INPUT`。
fn ttl_arg(args: &Value) -> Result<Duration, ToolError> {
    let Some(v) = args.get("ttlMs") else { return Ok(DEFAULT_LOCK_TTL) };
    match v.as_u64() {
        Some(ms) if (MIN_LOCK_TTL_MS..=MAX_LOCK_TTL_MS).contains(&ms) => Ok(Duration::from_millis(ms)),
        _ => Err(ToolError::new(
            ErrorKind::InvalidInput,
            format!("ttlMs 需为 {MIN_LOCK_TTL_MS}–{MAX_LOCK_TTL_MS} 之间的整数（毫秒）。"),
        )),
    }
}

impl HubShared {
    /// 是否提供对象锁（[`crate::HubConfig::max_locks`] > 0）。
    pub(crate) fn locks_enabled(&self) -> bool {
        self.config.max_locks > 0
    }

    /// 写调用转发前的 App 锁检查：`app_id` 被调用方以外的持有者锁定，且本次调用不是只读工具（`tool` 的生效注解
    /// `readOnlyHint: true`；`tool` 为 `None` 表示 App 级写操作，如 `apps.navigate`）→ `LOCKED`。未知注解的工具按写处理。
    pub(crate) fn check_app_lock(&self, app_id: &str, tool: Option<&str>, caller: &CallerKey) -> Result<(), ToolError> {
        if !self.locks_enabled() {
            return Ok(());
        }
        let target = LockTarget { app_id: app_id.to_owned(), key: None };
        let now = tokio::time::Instant::now();
        let Some((holder, remaining)) = self.agent_tasks().lock_holder(&target, caller, now) else {
            return Ok(());
        };
        if let Some(t) = tool
            && self.tool_annotations(app_id, t).and_then(|a| a.read_only_hint) == Some(true)
        {
            return Ok(());
        }
        Err(locked_error(&target, &holder, caller, remaining, "写操作未执行（只读工具不受影响）"))
    }

    /// 锁对象的 App 必须已知、未被整体隐藏且不是内置 `apps`。
    fn lock_app_known(&self, app_id: &str) -> Result<(), ToolError> {
        let known = self.is_upstream(app_id) || self.registry().has_app(app_id);
        if !known || self.app_hidden(app_id) || app_id == BUILTIN_APP_ID {
            return Err(unknown_app(app_id));
        }
        Ok(())
    }

    /// `apps.lock {appId, key?, ttlMs?}` → `{appId, key?, ttlMs, renewed, message}`。
    ///
    /// @error 未启用 / appId 未知 → `TOOL_NOT_FOUND`；参数不合法 → `INVALID_INPUT`；他人持有 → `LOCKED`；
    /// 持有数达上限 → `RATE_LIMITED`（`data.limit`）。
    pub(crate) fn builtin_lock(self: &Arc<Self>, ctx: &CallCtx, args: &Value) -> Result<CallToolResult, ToolError> {
        let target = target_arg(args)?;
        let ttl = ttl_arg(args)?;
        self.lock_app_known(&target.app_id)?;
        let now = tokio::time::Instant::now();
        let acquired = self.agent_tasks().acquire_lock(&ctx.caller, target.clone(), ttl, self.config.max_locks, now);
        let renewed = acquired.map_err(|refusal| match refusal {
            LockRefusal::Held { holder, remaining } => locked_error(&target, &holder, &ctx.caller, remaining, "加锁未成功"),
            LockRefusal::Limit(limit) => ToolError::new(
                ErrorKind::RateLimited,
                format!("同时持有的锁已达上限（{limit} 个）。请先用 {TOOL_APPS_UNLOCK} 释放不再需要的锁后重试。"),
            )
            .with_details(json!({ "limit": limit })),
        })?;
        let scope = match &target.key {
            None => "其他 Agent 对它的写调用会收到 LOCKED（只读工具不受影响）",
            Some(_) => "其他 Agent 对同一对象加锁会收到 LOCKED（不拦截任何调用）",
        };
        let secs = ttl.as_secs_f64();
        Ok(json_result(json!({
            "appId": target.app_id,
            "key": target.key,
            "ttlMs": millis(ttl),
            "renewed": renewed,
            "message": format!(
                "已{}{}，{secs} 秒内有效：{scope}。需要更久时在到期前再次调用 {TOOL_APPS_LOCK} 续期；用完调用 {TOOL_APPS_UNLOCK}。\
                 本任务结束时锁自动释放。",
                if renewed { "续期" } else { "锁定" },
                describe(&target)
            ),
        })))
    }

    /// `apps.unlock {appId, key?}` → `{appId, key?, released, message}`；不能释放他人的锁（幂等）。
    pub(crate) fn builtin_unlock(self: &Arc<Self>, ctx: &CallCtx, args: &Value) -> Result<CallToolResult, ToolError> {
        let target = target_arg(args)?;
        let now = tokio::time::Instant::now();
        let released = self.agent_tasks().release_lock(&ctx.caller, &target, now);
        let message = if released {
            format!("已释放{}的锁。", describe(&target))
        } else {
            format!("本任务没有持有{}的锁（可能已到期、已释放，或由其他 Agent 持有），无需释放。", describe(&target))
        };
        Ok(json_result(json!({ "appId": target.app_id, "key": target.key, "released": released, "message": message })))
    }

    /// 未到期的锁（`/status` 的 `locks`），按 appId、key 排序。
    pub(crate) fn lock_status(&self) -> Vec<LockStatus> {
        let now = tokio::time::Instant::now();
        self.agent_tasks()
            .live_locks(now)
            .into_iter()
            .map(|(caller, target, expires)| LockStatus {
                app_id: target.app_id,
                key: target.key,
                caller: caller.as_str().to_owned(),
                holder: caller.usage_subject(),
                expires_in_ms: millis(expires - now),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        assert_eq!(ttl_arg(&json!({})), Ok(DEFAULT_LOCK_TTL));
        assert_eq!(ttl_arg(&json!({"ttlMs": 1000})), Ok(Duration::from_secs(1)));
        assert_eq!(ttl_arg(&json!({"ttlMs": MAX_LOCK_TTL_MS})), Ok(Duration::from_millis(MAX_LOCK_TTL_MS)));
        for bad in [json!(999), json!(MAX_LOCK_TTL_MS + 1), json!(-1), json!("60000"), json!(1.5)] {
            assert_eq!(ttl_arg(&json!({"ttlMs": bad})).map_err(|e| e.kind), Err(ErrorKind::InvalidInput), "{bad}");
        }
        assert_eq!(target_arg(&json!({"appId": "shop"})).map(|t| t.key), Ok(None));
        assert_eq!(target_arg(&json!({"appId": "shop", "key": "doc 1"})).map(|t| t.key), Ok(Some("doc 1".into())));
        let long = "字".repeat(MAX_LOCK_KEY_LEN);
        assert!(target_arg(&json!({"appId": "shop", "key": long})).is_ok(), "按字符计");
        for bad in [String::new(), "x".repeat(MAX_LOCK_KEY_LEN + 1), "a\nb".into()] {
            assert!(target_arg(&json!({"appId": "shop", "key": bad})).is_err(), "{bad:?}");
        }
    }

    /// `LOCKED` 的 data 与消息只给记账主体，不含任务 ID。
    #[cfg(feature = "mcp-server")]
    #[test]
    fn locked_error_hides_task_id() {
        let owner = CallerKey::principal(&crate::task::Principal::Local);
        let handle = CallerKey::task_handle(&owner, "task-0123456789abcdef0123456789abcdef");
        let target = LockTarget { app_id: "shop".into(), key: Some("doc".into()) };
        let api = CallerKey::api(None);
        let e = locked_error(&target, &handle, &api, Duration::from_millis(1500), "x");
        assert_eq!(e.kind, ErrorKind::Locked);
        let d = e.details.clone().unwrap_or_default();
        assert_eq!((d["holder"].as_str(), d["retryAfterMs"].as_u64(), d["key"].as_str()), (Some("local"), Some(1500), Some("doc")));
        assert!(!format!("{e:?}").contains("task-0123"), "{e:?}");
        assert!(!e.message.contains("另一个任务"), "{}", e.message);
        let own = locked_error(&target, &handle, &owner, Duration::from_millis(1500), "x");
        assert!(own.message.contains("同为 local 的另一个任务") && own.message.contains("taskId"), "{}", own.message);
        assert!(!own.message.contains("task-0123"), "{}", own.message);
        assert_eq!(own.details, e.details, "data 不变");
    }
}
