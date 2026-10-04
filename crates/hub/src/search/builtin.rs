//! `apps.search {query, appId?, limit?}`（spec/hub-api.md 3.18）：参数校验、打分排序、结果与渐进暴露，以及调用结束时的使用统计。

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::CallToolResult;
use serde::Serialize;
use serde_json::{Value, json};

use crate::call::{CallCtx, json_result, unknown_app};
use crate::hub::{HubShared, lock};
use crate::names::TOOL_APPS_SEARCH;
use crate::task::CallerKey;
use crate::types::Availability;

use super::candidates::Candidate;
use super::score::{self, Signals, ToolText};
use super::{DEFAULT_SEARCH_LIMIT, MAX_QUERY_CHARS, MAX_SEARCH_LIMIT};

/// 校验后的参数。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct SearchArgs {
    pub query: String,
    pub app_id: Option<String>,
    pub limit: usize,
}

fn invalid(message: String) -> ToolError {
    ToolError::new(ErrorKind::InvalidInput, message)
}

/// 参数校验（不依赖 schema 校验是否编入，feature `schema-validation`）。
///
/// @error `query` 缺失、不是字符串、去掉空白后为空或超过 [`MAX_QUERY_CHARS`] 个字符；`appId` 不是字符串；`limit` 不是 1–
/// [`MAX_SEARCH_LIMIT`] 的整数 → `INVALID_INPUT`。
pub(super) fn parse_args(args: &Value) -> Result<SearchArgs, ToolError> {
    let query = args.get("query").and_then(Value::as_str).ok_or_else(|| invalid(format!("{TOOL_APPS_SEARCH} 需要字符串参数 query。")))?;
    if query.trim().is_empty() {
        return Err(invalid("query 不能为空：用关键词描述要做的事。".to_owned()));
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(invalid(format!("query 最多 {MAX_QUERY_CHARS} 个字符。")));
    }
    let app_id = match args.get("appId") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(invalid("appId 必须是字符串。".to_owned())),
    };
    let limit = match args.get("limit") {
        None | Some(Value::Null) => DEFAULT_SEARCH_LIMIT,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=MAX_SEARCH_LIMIT).contains(n))
            .ok_or_else(|| invalid(format!("limit 必须是 1–{MAX_SEARCH_LIMIT} 的整数。")))?,
    };
    Ok(SearchArgs { query: query.to_owned(), app_id, limit: usize::try_from(limit).unwrap_or(usize::MAX) })
}

/// 结果中的一个工具。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHit {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    description: String,
    app_id: String,
    availability: Availability,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<String>,
    input_schema: Value,
    /// 声明实现的标准意图（spec/intents.md）；未声明时不序列化。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    implements: Vec<String>,
    /// App 工具的 `schemaHash`（spec/hub-api.md 3.21）；上游工具不带。
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_hash: Option<String>,
    /// 弃用声明（原样）；未弃用时不序列化。
    #[serde(skip_serializing_if = "Option::is_none")]
    deprecated: Option<app_mcp_protocol::Deprecation>,
    /// 声明了 `undoable`（spec/hub-api.md 3.23）；未声明时不序列化。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    undoable: bool,
    score: f64,
}

impl HubShared {
    /// 内置工具 `apps.search`。
    ///
    /// @error 参数不合法 → `INVALID_INPUT`；`appId` 未知或被 `hide` → `TOOL_NOT_FOUND`（同 `apps.tools`）。
    /// @side-effect 命中的 App 记入调用方的暴露集合（同 `apps.tools`，spec/hub-api.md 3.7）。
    pub(crate) fn builtin_search(self: &Arc<Self>, ctx: &CallCtx, args: &Value) -> Result<CallToolResult, ToolError> {
        let SearchArgs { query, app_id, limit } = parse_args(args)?;
        if let Some(app) = app_id.as_deref() {
            let known = self.is_upstream(app) || self.registry().has_app(app);
            if !known || self.app_hidden(app) {
                return Err(unknown_app(app));
            }
        }
        let tokens = score::tokenize(&query);
        let mut ranked = self.rank_candidates(&ctx.caller, &tokens, self.search_candidates(app_id.as_deref()));
        let total = ranked.len();
        ranked.truncate(limit);
        let apps: BTreeSet<String> = ranked.iter().map(|h| h.app_id.clone()).collect();
        for app in &apps {
            self.expose_in_session(ctx, app);
        }
        let message = self.search_message(&ctx.caller, &query, total, ranked.len());
        Ok(json_result(json!({ "results": ranked, "total": total, "message": message })))
    }

    /// 打分、过滤 0 分、排序（未截断）。
    fn rank_candidates(&self, caller: &CallerKey, tokens: &[String], candidates: Vec<Candidate>) -> Vec<SearchHit> {
        let subject = caller.usage_subject();
        let now = Instant::now();
        let mut hits: Vec<SearchHit> = candidates
            .into_iter()
            .filter_map(|c| {
                let t = &c.tool;
                let text = ToolText {
                    full_name: &t.name,
                    title: t.title.as_deref(),
                    description: &t.description,
                    context: c.context.iter().map(String::as_str).collect(),
                };
                let keyword = score::keyword_score(tokens, &text);
                if keyword <= 0.0 {
                    return None;
                }
                let stat = lock(&self.search_stats).get(&subject, &t.name);
                let signals = Signals {
                    on_current_surface: self.registry().tool_on_current_surface(&t.app_id, &t.tool),
                    used_recently: stat.is_some_and(|s| s.used_within_window(now)),
                    calls: stat.map_or(0, |s| s.calls),
                    success_rate: stat.and_then(|s| s.success_rate()),
                    deprecated: t.deprecated.is_some(),
                };
                let total = score::total_score(keyword, &signals)?;
                let tool = c.tool;
                Some(SearchHit {
                    name: tool.name,
                    title: tool.title,
                    description: tool.description,
                    app_id: tool.app_id,
                    availability: tool.availability,
                    page: tool.page,
                    input_schema: tool.input_schema,
                    implements: tool.implements,
                    schema_hash: tool.schema_hash,
                    deprecated: tool.deprecated,
                    undoable: tool.undoable,
                    score: total,
                })
            })
            .collect();
        hits.sort_by(|a, b| score::rank_order((a.score, &a.name), (b.score, &b.name)));
        hits
    }

    fn search_message(&self, caller: &CallerKey, query: &str, total: usize, shown: usize) -> String {
        if total == 0 {
            return format!(
                "没有找到与「{query}」相关的工具。可以换个说法或换用中 / 英文关键词再试，或调用 apps.list 查看有哪些 App、\
                 apps.tools 查看某个 App 的全部工具。"
            );
        }
        let mut message = if shown < total {
            format!("找到 {total} 个相关工具，按相关度列出前 {shown} 个（可加大 limit 或换用更具体的关键词）。")
        } else {
            format!("找到 {total} 个相关工具，按相关度排列。")
        };
        message.push_str("可直接按全名调用；带 page 的工具不在当前页面时，Hub 会先让 App 切换到该页面。");
        if self.progressive_for(caller) && !caller.is_stateless() {
            message.push_str("命中的 App 的工具已加入本会话的工具列表（客户端刷新列表后可见）。");
        }
        message
    }

    /// 调用结束时记使用统计（[`super::call_outcome`] 为 `None` 的不记）。
    pub(crate) fn record_tool_use(&self, caller: &CallerKey, name: &str, success: Option<bool>) {
        if let Some(success) = success {
            lock(&self.search_stats).record(&caller.usage_subject(), name, success, Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults_and_bounds() {
        let ok = parse_args(&json!({"query": "导出订单"})).unwrap();
        assert_eq!(ok, SearchArgs { query: "导出订单".into(), app_id: None, limit: 10 });
        let ok = parse_args(&json!({"query": "x", "appId": "shop", "limit": 50})).unwrap();
        assert_eq!((ok.app_id.as_deref(), ok.limit), (Some("shop"), 50));
        assert_eq!(parse_args(&json!({"query": "x", "limit": 1})).unwrap().limit, 1);
        let max = "订".repeat(MAX_QUERY_CHARS);
        assert!(parse_args(&json!({ "query": max })).is_ok(), "按字符数而不是字节数计");
        let bad = [
            json!({}),
            json!({"query": 1}),
            json!({"query": ""}),
            json!({"query": "   "}),
            json!({"query": "a".repeat(MAX_QUERY_CHARS + 1)}),
            json!({"query": "x", "limit": 0}),
            json!({"query": "x", "limit": 51}),
            json!({"query": "x", "limit": 2.5}),
            json!({"query": "x", "limit": "3"}),
            json!({"query": "x", "appId": 3}),
        ];
        for args in bad {
            assert_eq!(parse_args(&args).unwrap_err().kind, ErrorKind::InvalidInput, "{args}");
        }
    }
}
