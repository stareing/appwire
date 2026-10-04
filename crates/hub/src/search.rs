//! 工具检索：内置工具 `apps.search`（spec/hub-api.md 3.18，第 16 项 O1）。
//!
//! - [`score`]：分词、关键词得分、排序加成（纯函数，规则表）。
//! - [`stats`]：按（记账主体, 工具全名）的使用统计（纯状态，只在内存）。
//! - `candidates`：候选工具（注册表、休眠快照、清单、上游缓存、页面目录）；`apps.intents` 复用（[`crate::intents`]）。
//! - `builtin`：参数校验、检索与结果。
//!
//! @invariant 检索只读注册表与缓存：不唤醒 App、不连接上游、不新增定时器或后台任务。

use crate::call::Body;

mod builtin;
pub(crate) mod candidates;
pub(crate) mod score;
pub(crate) mod stats;

pub(crate) use stats::SearchStats;

/// `query` 的最大字符数。
pub const MAX_QUERY_CHARS: usize = 200;
/// `limit` 的上限。
pub const MAX_SEARCH_LIMIT: u64 = 50;
/// `limit` 的缺省值。
pub const DEFAULT_SEARCH_LIMIT: u64 = 10;

/// 调用结束时计入使用统计的结果：App 与上游工具的成败（错误与 `isError` 结果都算失败）；内置工具与名称无法解析的不记。
pub(crate) fn call_outcome(body: &Body) -> Option<bool> {
    match body {
        Body::App(r) => Some(r.is_ok()),
        Body::Upstream(Ok(r)) => Some(r.is_error != Some(true)),
        Body::Upstream(Err(_)) => Some(false),
        Body::Builtin(_) | Body::NotFound(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use app_mcp_protocol::{ErrorKind, ToolError, ToolsInvokeResult};
    use rmcp::ErrorData as McpError;
    use rmcp::model::{CallToolResult, ContentBlock};

    use super::*;

    #[test]
    fn call_outcome_per_body() {
        let err = || ToolError::new(ErrorKind::HandlerError, "x");
        let cases: [(Body, Option<bool>); 7] = [
            (Body::App(Ok(ToolsInvokeResult::default())), Some(true)),
            (Body::App(Err(err())), Some(false)),
            (Body::Upstream(Ok(CallToolResult::success(vec![ContentBlock::text("ok")]))), Some(true)),
            (Body::Upstream(Ok(CallToolResult::error(vec![ContentBlock::text("bad")]))), Some(false)),
            (Body::Upstream(Err(McpError::internal_error("x", None))), Some(false)),
            (Body::Builtin(Ok(CallToolResult::success(vec![]))), None),
            (Body::NotFound(err()), None),
        ];
        for (i, (body, want)) in cases.iter().enumerate() {
            assert_eq!(call_outcome(body), *want, "第 {i} 项");
        }
    }
}
