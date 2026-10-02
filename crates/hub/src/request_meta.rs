//! MCP `tools/call` 请求 `_meta` 中 Agent 给出的调用控制（spec/hub-api.md 3.15）：截止时间与幂等键。
//!
//! 纯函数：只解析与校验（信任边界，G-02），不读时钟、不依赖 rmcp 类型。

use std::time::Duration;

use app_mcp_protocol::{ErrorKind, MAX_IDEMPOTENCY_KEY_LEN, ToolError};
use serde_json::{Map, Value};

use crate::names::{META_IDEMPOTENCY_KEY, META_TIMEOUT_MS};

/// Agent 给出的调用控制。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentCallMeta {
    /// [`META_TIMEOUT_MS`]：本次调用愿意等待的时长。
    pub timeout: Option<Duration>,
    /// [`META_IDEMPOTENCY_KEY`]：原样转交 App。
    pub idempotency_key: Option<String>,
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

/// 从请求 `_meta` 取出调用控制；键缺省时为 `None`。
///
/// @error `INVALID_INPUT`：`app-mcp/timeoutMs` 不是正整数；`app-mcp/idempotencyKey` 不是 1..=256 个字符的字符串。
#[cfg_attr(not(feature = "mcp-server"), allow(dead_code))]
pub(crate) fn parse(meta: &Map<String, Value>) -> Result<AgentCallMeta, ToolError> {
    let timeout = match meta.get(META_TIMEOUT_MS) {
        None => None,
        Some(v) => match v.as_u64().filter(|ms| *ms > 0) {
            Some(ms) => Some(Duration::from_millis(ms)),
            None => return Err(invalid(format!("_meta「{META_TIMEOUT_MS}」必须是正整数（毫秒），收到 {v}。"))),
        },
    };
    let idempotency_key = match meta.get(META_IDEMPOTENCY_KEY) {
        None => None,
        Some(Value::String(k)) => {
            check_idempotency_key(k)?;
            Some(k.clone())
        }
        Some(_) => return Err(invalid_idempotency_key()),
    };
    Ok(AgentCallMeta { timeout, idempotency_key })
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
    fn timeout_is_min_of_agent_and_config() {
        let cfg = Duration::from_secs(60);
        assert_eq!(effective_timeout(None, cfg), None);
        assert_eq!(effective_timeout(Some(Duration::from_secs(5)), cfg), Some(Duration::from_secs(5)));
        assert_eq!(effective_timeout(Some(Duration::from_secs(600)), cfg), Some(cfg));
    }
}
