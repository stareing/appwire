//! MCP `tools/call` 请求 `_meta` 中 Agent 给出的调用控制（spec/hub-api.md 3.15）：截止时间、幂等键与任务句柄（3.6）。
//!
//! 纯函数：只解析与校验（信任边界，G-02），不读时钟、不依赖 rmcp 类型。

use std::time::Duration;

use app_mcp_protocol::{CallPriority, ErrorKind, MAX_IDEMPOTENCY_KEY_LEN, ToolError};
use serde_json::{Map, Value};

use crate::names::{
    LEGACY_META_IDEMPOTENCY_KEY, LEGACY_META_TIMEOUT_MS, META_IDEMPOTENCY_KEY, META_PRIORITY, META_TASK_ID, META_TIMEOUT_MS,
};

/// Agent 给出的调用控制。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentCallMeta {
    /// [`META_TIMEOUT_MS`]：本次调用愿意等待的时长。
    pub timeout: Option<Duration>,
    /// [`META_IDEMPOTENCY_KEY`]：原样转交 App。
    pub idempotency_key: Option<String>,
    /// [`META_TASK_ID`]：任务句柄（只校验是字符串；格式与是否有效在解析调用方时判定，[`crate::task_handle`]）。
    pub task_id: Option<String>,
    /// [`META_PRIORITY`]：原样转交 App（省略 = normal）。
    pub priority: CallPriority,
}

fn invalid(message: String) -> ToolError {
    ToolError::new(ErrorKind::InvalidInput, message)
}

/// 幂等键的边界校验（MCP `_meta` 与 Hub API [`crate::CallRequest::idempotency_key`] 共用）。
///
/// @error `INVALID_INPUT`：空串或超过 [`MAX_IDEMPOTENCY_KEY_LEN`] 个字符。
pub(crate) fn check_idempotency_key(key: &str) -> Result<(), ToolError> {
    if key.is_empty() || key.chars().count() > MAX_IDEMPOTENCY_KEY_LEN {
        return Err(invalid_idempotency_key());
    }
    Ok(())
}

fn invalid_idempotency_key() -> ToolError {
    invalid(format!("幂等键（{META_IDEMPOTENCY_KEY}）必须是 1 到 {MAX_IDEMPOTENCY_KEY_LEN} 个字符的字符串。"))
}

/// 取新键，弃用期内回退到旧键。
///
/// @compat 旧前缀 `app-mcp/`（docs/plans/12-mcp-stateless.md S1）。
/// @error `INVALID_INPUT`：新旧键同时出现且取值不同。
fn pick<'a>(meta: &'a Map<String, Value>, key: &str, legacy: &str) -> Result<Option<&'a Value>, ToolError> {
    match (meta.get(key), meta.get(legacy)) {
        (Some(new), Some(old)) if new != old => {
            Err(invalid(format!("_meta「{key}」与旧键「{legacy}」取值不同（{new} / {old}）；只保留「{key}」。")))
        }
        (new, old) => Ok(new.or(old)),
    }
}

/// 从请求 `_meta` 取出调用控制；键缺省时为 `None`。
///
/// @error `INVALID_INPUT`：`dev.appwire/timeoutMs` 不是正整数；`dev.appwire/idempotencyKey` 不是 1..=256 个字符的字符串；
/// 新旧键取值冲突（见 [`pick`]）；`dev.appwire/taskId` 不是字符串；`dev.appwire/priority` 不是 `interactive` / `normal` / `background`。
#[cfg_attr(not(feature = "mcp-server"), allow(dead_code))]
pub(crate) fn parse(meta: &Map<String, Value>) -> Result<AgentCallMeta, ToolError> {
    let timeout = match pick(meta, META_TIMEOUT_MS, LEGACY_META_TIMEOUT_MS)? {
        None => None,
        Some(v) => match v.as_u64().filter(|ms| *ms > 0) {
            Some(ms) => Some(Duration::from_millis(ms)),
            None => return Err(invalid(format!("_meta「{META_TIMEOUT_MS}」必须是正整数（毫秒），收到 {v}。"))),
        },
    };
    let idempotency_key = match pick(meta, META_IDEMPOTENCY_KEY, LEGACY_META_IDEMPOTENCY_KEY)? {
        None => None,
        Some(Value::String(k)) => {
            check_idempotency_key(k)?;
            Some(k.clone())
        }
        Some(_) => return Err(invalid_idempotency_key()),
    };
    let task_id = match meta.get(META_TASK_ID) {
        None => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(v) => return Err(invalid(format!("_meta「{META_TASK_ID}」必须是字符串（apps.task.begin 返回的 taskId），收到 {v}。"))),
    };
    let priority = match meta.get(META_PRIORITY) {
        None => CallPriority::Normal,
        Some(v) => match v.as_str().and_then(CallPriority::parse) {
            Some(p) => p,
            None => {
                return Err(invalid(format!("_meta「{META_PRIORITY}」必须是 \"interactive\"、\"normal\" 或 \"background\"，收到 {v}。")));
            }
        },
    };
    Ok(AgentCallMeta { timeout, idempotency_key, task_id, priority })
}

/// 本次调用的等待上限：Agent 的截止时间与配置值取较小者；Agent 没给时为 `None`（用配置值）。
#[cfg_attr(not(feature = "mcp-server"), allow(dead_code))]
pub(crate) fn effective_timeout(agent: Option<Duration>, configured: Duration) -> Option<Duration> {
    agent.map(|t| t.min(configured))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }

    #[test]
    fn absent_keys_are_none() {
        assert_eq!(parse(&meta(json!({ "progressToken": 1 }))).unwrap(), AgentCallMeta::default());
    }

    #[test]
    fn valid_values() {
        let m = parse(&meta(json!({ META_TIMEOUT_MS: 1500, META_IDEMPOTENCY_KEY: "order-7" }))).unwrap();
        assert_eq!(m.timeout, Some(Duration::from_millis(1500)));
        assert_eq!(m.idempotency_key.as_deref(), Some("order-7"));
        let long = "键".repeat(MAX_IDEMPOTENCY_KEY_LEN);
        assert!(parse(&meta(json!({ META_IDEMPOTENCY_KEY: long }))).is_ok(), "上限按字符计");
    }

    #[test]
    fn invalid_values_are_rejected() {
        for bad in [json!(0), json!(-5), json!(1.5), json!("100"), json!(null)] {
            let e = parse(&meta(json!({ META_TIMEOUT_MS: bad }))).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
        }
        let too_long = "k".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1);
        for bad in [json!(""), json!(7), json!(too_long), json!(null)] {
            let e = parse(&meta(json!({ META_IDEMPOTENCY_KEY: bad }))).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn keys_use_reverse_domain_prefix() {
        assert_eq!(META_TIMEOUT_MS, "dev.appwire/timeoutMs");
        assert_eq!(META_IDEMPOTENCY_KEY, "dev.appwire/idempotencyKey");
        assert_eq!(META_TASK_ID, "dev.appwire/taskId");
        assert_eq!(META_PRIORITY, "dev.appwire/priority");
    }

    #[test]
    fn priority_is_validated() {
        assert_eq!(parse(&meta(json!({}))).unwrap().priority, CallPriority::Normal);
        assert_eq!(parse(&meta(json!({ META_PRIORITY: "interactive" }))).unwrap().priority, CallPriority::Interactive);
        assert_eq!(parse(&meta(json!({ META_PRIORITY: "background" }))).unwrap().priority, CallPriority::Background);
        for bad in [json!("urgent"), json!(1), json!(null)] {
            let e = parse(&meta(json!({ META_PRIORITY: bad }))).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
            assert!(e.message.contains(META_PRIORITY), "{}", e.message);
        }
    }

    #[test]
    fn task_id_must_be_string() {
        let id = "task-0123456789abcdef0123456789abcdef";
        assert_eq!(parse(&meta(json!({ META_TASK_ID: id }))).unwrap().task_id.as_deref(), Some(id));
        for bad in [json!(7), json!(null), json!({"id": id})] {
            let e = parse(&meta(json!({ META_TASK_ID: bad }))).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
            assert!(e.message.contains(META_TASK_ID), "{}", e.message);
        }
    }

    #[test]
    fn legacy_keys_still_accepted() {
        let m = parse(&meta(json!({ LEGACY_META_TIMEOUT_MS: 800, LEGACY_META_IDEMPOTENCY_KEY: "old-1" }))).unwrap();
        assert_eq!(m.timeout, Some(Duration::from_millis(800)));
        assert_eq!(m.idempotency_key.as_deref(), Some("old-1"));
        let e = parse(&meta(json!({ LEGACY_META_TIMEOUT_MS: 0 }))).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput, "旧键同样校验");
    }

    #[test]
    fn new_and_legacy_keys_conflict() {
        let same = parse(&meta(json!({ META_TIMEOUT_MS: 500, LEGACY_META_TIMEOUT_MS: 500 }))).unwrap();
        assert_eq!(same.timeout, Some(Duration::from_millis(500)), "取值相同不算冲突");
        for m in [
            json!({ META_TIMEOUT_MS: 500, LEGACY_META_TIMEOUT_MS: 900 }),
            json!({ META_IDEMPOTENCY_KEY: "a", LEGACY_META_IDEMPOTENCY_KEY: "b" }),
        ] {
            let e = parse(&meta(m)).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
            assert!(e.message.contains("旧键"), "{}", e.message);
        }
    }

    #[test]
    fn timeout_is_min_of_agent_and_config() {
        let cfg = Duration::from_secs(60);
        assert_eq!(effective_timeout(None, cfg), None);
        assert_eq!(effective_timeout(Some(Duration::from_secs(5)), cfg), Some(Duration::from_secs(5)));
        assert_eq!(effective_timeout(Some(Duration::from_secs(600)), cfg), Some(cfg));
    }
}
