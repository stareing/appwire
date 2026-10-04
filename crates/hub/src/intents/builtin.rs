//! `apps.intents {intent?}`（spec/intents.md 第 4 节）：参数校验、结果与渐进暴露。

use std::collections::BTreeSet;
use std::sync::Arc;

use app_mcp_protocol::intents::parse_verb_query;
use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::call::{CallCtx, json_result};
use crate::hub::{HubShared, lock};
use crate::names::{ARG_TASK_ID, TOOL_APPS_INTENTS};
use crate::types::HubTool;

use super::MAX_INTENT_ARG_CHARS;
use super::collect::{IntentEntry, IntentQuery, collect};

fn invalid(message: String) -> ToolError {
    ToolError::new(ErrorKind::InvalidInput, message)
}

/// 参数校验（不依赖 schema 校验是否编入，feature `schema-validation`）：`intent` 缺省 = 全部。
///
/// @error 有 `intent` / `taskId` 以外的参数、`intent` 不是字符串、超过 [`MAX_INTENT_ARG_CHARS`] 个字符、不是 `<域>.<动作>` 或 `<域>.<动作>@<主版本>` →
/// `INVALID_INPUT`。
pub(super) fn parse_args(args: &Value) -> Result<Option<String>, ToolError> {
    if let Some(key) = args.as_object().and_then(|m| m.keys().find(|k| !matches!(k.as_str(), "intent" | ARG_TASK_ID))) {
        return Err(invalid(format!("{TOOL_APPS_INTENTS} 不接受参数「{key}」（只有 intent）。")));
    }
    let intent = match args.get("intent") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s,
        Some(_) => return Err(invalid(format!("{TOOL_APPS_INTENTS} 的 intent 必须是字符串。"))),
    };
    if intent.chars().count() > MAX_INTENT_ARG_CHARS {
        return Err(invalid(format!("intent 最多 {MAX_INTENT_ARG_CHARS} 个字符。")));
    }
    parse_verb_query(intent).map_err(|_| {
        invalid(format!("intent「{intent}」不合法：写动词（如 message.send，任意版本）或带主版本（如 message.send@1）。"))
    })?;
    Ok(Some(intent.clone()))
}

impl HubShared {
    /// 内置工具 `apps.intents`。
    ///
    /// @error 参数不合法 → `INVALID_INPUT`。
    /// @side-effect 列为实现者的 App 记入调用方的暴露集合（同 `apps.tools`，spec/hub-api.md 3.7）；不唤醒 App。
    pub(crate) fn builtin_intents(self: &Arc<Self>, ctx: &CallCtx, args: &Value) -> Result<CallToolResult, ToolError> {
        let intent = parse_args(args)?;
        let query = match intent.as_deref() {
            None => None,
            Some(text) => {
                let (verb, version) = parse_verb_query(text).map_err(|e| invalid(e.to_string()))?;
                Some(IntentQuery { verb, version })
            }
        };
        let tools: Vec<HubTool> =
            self.search_candidates(None).into_iter().map(|c| c.tool).filter(|t| !t.implements.is_empty()).collect();
        let defaults = lock(&self.intents).defaults.clone();
        let entries = collect(&tools, &defaults, query);
        let apps: BTreeSet<&str> =
            entries.iter().flat_map(|e| e.implementations.iter().map(|i| i.app_id.as_str())).collect();
        for app in apps {
            self.expose_in_session(ctx, app);
        }
        let message = intents_message(&entries, intent.as_deref());
        Ok(json_result(json!({ "intents": entries, "message": message })))
    }
}

/// 结果的说明文字。
fn intents_message(entries: &[IntentEntry], intent: Option<&str>) -> String {
    let implemented = entries.iter().filter(|e| !e.implementations.is_empty()).count();
    let mut message = match (intent, implemented) {
        (Some(i), 0) if entries.is_empty() => format!("没有 App 声明实现「{i}」，标准意图词表中也没有这个动词。"),
        (Some(i), 0) => format!("没有 App 声明实现「{i}」（或声明的工具不满足必填参数，见 incompatible）。"),
        (None, 0) => "目前没有 App 声明实现任何标准意图；intents 列出了词表中的动词。".to_owned(),
        (_, n) => format!("{n} 个标准意图有实现者。"),
    };
    if implemented > 0 {
        message.push_str(
            "选定实现者后按工具全名（tool）调用，参数按该工具的 inputSchema（apps.tools / apps.search 可查看）；default 为机主设置的\
             默认 App，只是提示。不会启动 App。",
        );
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_rules() {
        assert_eq!(parse_args(&json!({})).unwrap(), None);
        assert_eq!(parse_args(&json!({"intent": null})).unwrap(), None);
        assert_eq!(parse_args(&json!({"intent": "message.send"})).unwrap().as_deref(), Some("message.send"));
        assert_eq!(parse_args(&json!({"intent": "x.y@3"})).unwrap().as_deref(), Some("x.y@3"));
        assert_eq!(parse_args(&json!({"taskId": "task-1"})).unwrap(), None, "任务级工具可带 taskId");
        let bad = [
            json!({"intent": 1}),
            json!({"intent": ""}),
            json!({"intent": "message"}),
            json!({"intent": "message.send@0"}),
            json!({"intents": "message.send"}),
            json!({"intent": format!("a.{}", "b".repeat(MAX_INTENT_ARG_CHARS))}),
        ];
        for args in bad {
            assert_eq!(parse_args(&args).unwrap_err().kind, ErrorKind::InvalidInput, "{args}");
        }
    }

    #[test]
    fn message_per_case() {
        let e = |implementations: usize| IntentEntry {
            intent: "a.b@1".into(),
            known: true,
            description: None,
            implementations: (0..implementations)
                .map(|i| super::super::collect::Implementation {
                    tool: format!("app.t{i}"),
                    app_id: "app".into(),
                    availability: crate::types::Availability::Available,
                    default: false,
                })
                .collect(),
            incompatible: Vec::new(),
        };
        assert!(intents_message(&[], Some("x.y")).contains("词表中也没有"));
        assert!(intents_message(&[e(0)], Some("a.b")).contains("incompatible"));
        assert!(intents_message(&[e(0)], None).contains("没有 App 声明实现任何"));
        let m = intents_message(&[e(2), e(0)], None);
        assert!(m.starts_with("1 个标准意图有实现者") && m.contains("按工具全名"), "{m}");
    }
}
