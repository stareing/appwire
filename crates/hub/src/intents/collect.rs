//! 按意图分组列出实现者（spec/intents.md 第 4 节）：纯函数，输入为候选工具（来源同 `apps.search`，已按 `hide` 过滤）。
//!
//! 规则：
//! - 词表中的动词版本做兼容性检查（[`check_required`]）：满足的列入 `implementations`，不满足的列入 `incompatible`（带原因）。
//! - 不在词表中的照常列入 `implementations`，`known: false`。
//! - `implementations` 排序：机主默认在前（`default: true`），其余按工具全名；`incompatible` 按工具全名。
//! - 条目按（动词, 主版本）排序；无查询时含词表全部动词版本（即使没有实现者）。

use std::collections::BTreeMap;

use app_mcp_protocol::intents::{IntentRef, VOCABULARY, check_required, lookup};
use serde::Serialize;

use crate::types::{Availability, HubTool};

use super::defaults::default_for;

/// `apps.intents` 的 `intent` 参数：动词与可选主版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IntentQuery<'a> {
    pub verb: &'a str,
    pub version: Option<u32>,
}

impl IntentQuery<'_> {
    fn matches(&self, verb: &str, version: u32) -> bool {
        self.verb == verb && self.version.is_none_or(|v| v == version)
    }
}

/// 一个实现者。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Implementation {
    /// 工具全名（Agent 按它调用）。
    pub tool: String,
    pub app_id: String,
    pub availability: Availability,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}

/// 声明了意图但不满足词表必填参数的工具。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct Incompatible {
    pub tool: String,
    pub reason: String,
}

/// 一个意图版本及其实现者。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct IntentEntry {
    /// `<动词>@<主版本>`。
    pub intent: String,
    pub known: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    pub implementations: Vec<Implementation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incompatible: Vec<Incompatible>,
}

impl IntentEntry {
    fn new(verb: &str, version: u32) -> Self {
        let def = lookup(verb, version);
        Self {
            intent: format!("{verb}@{version}"),
            known: def.is_some(),
            description: def.map(|d| d.description),
            implementations: Vec::new(),
            incompatible: Vec::new(),
        }
    }
}

type Entries = BTreeMap<(String, u32), IntentEntry>;

fn entry<'e>(entries: &'e mut Entries, verb: &str, version: u32) -> &'e mut IntentEntry {
    entries.entry((verb.to_owned(), version)).or_insert_with(|| IntentEntry::new(verb, version))
}

/// 词表中的条目（无查询时全部；有查询时匹配的；带版本的查询总有一条，不在词表中时为 `known: false`）。
fn seed(entries: &mut Entries, query: Option<IntentQuery<'_>>) {
    for d in VOCABULARY.iter().filter(|d| query.is_none_or(|q| q.matches(d.verb, d.version))) {
        entry(entries, d.verb, d.version);
    }
    if let Some(IntentQuery { verb, version: Some(v) }) = query {
        entry(entries, verb, v);
    }
}

/// 按意图分组列出实现者。`tools` 中格式不合法的 `implements` 项跳过（核心与清单已拒绝，只可能来自不校验的旧 SDK）。
pub(super) fn collect(
    tools: &[HubTool],
    defaults: &BTreeMap<String, String>,
    query: Option<IntentQuery<'_>>,
) -> Vec<IntentEntry> {
    let mut entries = Entries::new();
    seed(&mut entries, query);
    for tool in tools {
        for item in &tool.implements {
            let Ok(intent) = IntentRef::parse(item) else {
                tracing::debug!(tool = %tool.name, %item, "忽略格式不合法的 implements 项");
                continue;
            };
            if query.is_some_and(|q| !q.matches(intent.verb, intent.version)) {
                continue;
            }
            let e = entry(&mut entries, intent.verb, intent.version);
            match intent.lookup().map(|def| check_required(def, &tool.input_schema)) {
                Some(Err(reason)) => e.incompatible.push(Incompatible { tool: tool.name.clone(), reason }),
                Some(Ok(())) | None => e.implementations.push(Implementation {
                    tool: tool.name.clone(),
                    app_id: tool.app_id.clone(),
                    availability: tool.availability,
                    default: default_for(defaults, intent.verb, intent.version) == Some(tool.name.as_str()),
                }),
            }
        }
    }
    let mut out: Vec<IntentEntry> = entries.into_values().collect();
    for e in &mut out {
        e.implementations.sort_by(|a, b| b.default.cmp(&a.default).then_with(|| a.tool.cmp(&b.tool)));
        e.incompatible.sort_by(|a, b| a.tool.cmp(&b.tool));
    }
    out
}

#[cfg(test)]
mod tests;
