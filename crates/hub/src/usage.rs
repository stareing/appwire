//! 按调用方记账（docs/plans/16-agent-os.md P3；spec/hub-api.md 3.11「按 Agent 记账与配额」）。
//!
//! 记账主体（[`crate::task::CallerKey::usage_subject`]）：已登记的 Agent 为 `agent:<名>`（含其任务句柄与以其令牌建立的 legacy
//! 会话），其余 MCP 调用方为 `local`，Hub API 为 `api`。每个主体累计调用、唤醒、被限流次数与参数 / 结果字节数，并按 App 细分，
//! 回答"谁唤醒了这个 App 多少次"。计数自 Hub 启动起累计，不持久化、不重置。
//!
//! 本模块是纯状态（不做 I/O、不读时钟）；记账点在 [`crate::call`]（调用守卫、结果接收）与 [`crate::hub`]（唤醒入口）。
//!
//! @invariant 主体数至多 [`MAX_USAGE_SUBJECTS`]、每主体的 App 至多 [`MAX_USAGE_APPS`]（B-07）：超出的计入主体合计，
//! 不再细分到新的 App（`appsTruncated`）；新主体超出时计入 `other`。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 记账主体数上限（Agent 至多 256 个，另有 `local` / `api`；留出余量）。
pub(crate) const MAX_USAGE_SUBJECTS: usize = 512;
/// 每个主体按 App 细分的条目上限。
pub(crate) const MAX_USAGE_APPS: usize = 256;
/// 主体数达到上限后新主体的计入名。
const OVERFLOW_SUBJECT: &str = "other";

/// 一次记账事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UsageEvent {
    /// 调用通过了资源保护（参数字节数）。
    Call { arguments_bytes: u64 },
    /// 调用被限流拒绝（未执行）。
    RateLimited,
    /// 收到 App / 上游的结果或错误（字节数）。
    Result { bytes: u64 },
    /// 为该主体的调用 / 资源读取 / `apps.activate` 发起唤醒（通过唤醒策略之后；与进行中的唤醒合并的也计入）。
    Wake,
}

/// 计数（主体合计与每 App 共用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCounts {
    pub calls: u64,
    pub wakes: u64,
    pub rate_limited: u64,
    pub arguments_bytes: u64,
    pub result_bytes: u64,
}

impl UsageCounts {
    fn add(&mut self, event: UsageEvent) {
        match event {
            UsageEvent::Call { arguments_bytes } => {
                self.calls = self.calls.saturating_add(1);
                self.arguments_bytes = self.arguments_bytes.saturating_add(arguments_bytes);
            }
            UsageEvent::RateLimited => self.rate_limited = self.rate_limited.saturating_add(1),
            UsageEvent::Result { bytes } => self.result_bytes = self.result_bytes.saturating_add(bytes),
            UsageEvent::Wake => self.wakes = self.wakes.saturating_add(1),
        }
    }
}

/// 一个主体的用量（`/status` 的 `usage[]`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatus {
    /// `agent:<名>` / `local` / `api` / `other`。
    pub subject: String,
    /// 主体为已登记 Agent 时的名字。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(flatten)]
    pub total: UsageCounts,
    /// 按 App 细分，按 appId 排序。
    pub apps: Vec<AppUsageStatus>,
    /// 细分条目达到上限后，新 App 的用量只计入合计。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub apps_truncated: bool,
}

/// [`UsageStatus::apps`] 的一项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUsageStatus {
    pub app_id: String,
    #[serde(flatten)]
    pub counts: UsageCounts,
}

#[derive(Debug, Default)]
struct SubjectBook {
    agent: Option<String>,
    total: UsageCounts,
    apps: HashMap<String, UsageCounts>,
    truncated: bool,
}

/// 记账表（`HubShared` 持有，一把锁）。
#[derive(Debug, Default)]
pub(crate) struct UsageBook {
    subjects: HashMap<String, SubjectBook>,
}

impl UsageBook {
    /// 记一次事件。`agent`：主体为已登记 Agent 时的名字（只在首次记账时保存）。
    pub(crate) fn record(&mut self, subject: &str, agent: Option<&str>, app_id: &str, event: UsageEvent) {
        let key = if self.subjects.contains_key(subject) || self.subjects.len() < MAX_USAGE_SUBJECTS {
            subject
        } else {
            OVERFLOW_SUBJECT
        };
        let book = self.subjects.entry(key.to_owned()).or_insert_with(|| SubjectBook {
            agent: (key == subject).then(|| agent.map(str::to_owned)).flatten(),
            ..SubjectBook::default()
        });
        book.total.add(event);
        if !book.apps.contains_key(app_id) && book.apps.len() >= MAX_USAGE_APPS {
            book.truncated = true;
            return;
        }
        book.apps.entry(app_id.to_owned()).or_default().add(event);
    }

    /// 快照，按主体排序。
    pub(crate) fn status(&self) -> Vec<UsageStatus> {
        let mut out: Vec<UsageStatus> = self
            .subjects
            .iter()
            .map(|(subject, b)| {
                let mut apps: Vec<AppUsageStatus> =
                    b.apps.iter().map(|(app_id, c)| AppUsageStatus { app_id: app_id.clone(), counts: *c }).collect();
                apps.sort_by(|x, y| x.app_id.cmp(&y.app_id));
                UsageStatus { subject: subject.clone(), agent: b.agent.clone(), total: b.total, apps, apps_truncated: b.truncated }
            })
            .collect();
        out.sort_by(|a, b| a.subject.cmp(&b.subject));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_per_subject_and_app() {
        let mut b = UsageBook::default();
        b.record("agent:claude", Some("claude"), "shop", UsageEvent::Call { arguments_bytes: 10 });
        b.record("agent:claude", Some("claude"), "shop", UsageEvent::Result { bytes: 30 });
        b.record("agent:claude", Some("claude"), "music", UsageEvent::Wake);
        b.record("agent:claude", Some("claude"), "shop", UsageEvent::RateLimited);
        b.record("local", None, "shop", UsageEvent::Call { arguments_bytes: 2 });
        let st = b.status();
        assert_eq!(st.iter().map(|s| s.subject.as_str()).collect::<Vec<_>>(), ["agent:claude", "local"]);
        let c = &st[0];
        assert_eq!(c.agent.as_deref(), Some("claude"));
        assert_eq!(c.total, UsageCounts { calls: 1, wakes: 1, rate_limited: 1, arguments_bytes: 10, result_bytes: 30 });
        assert_eq!(c.apps.iter().map(|a| (a.app_id.as_str(), a.counts.calls, a.counts.wakes)).collect::<Vec<_>>(), [("music", 0, 1), ("shop", 1, 0)]);
        assert_eq!(st[1].agent, None);
        let json = serde_json::to_value(c).unwrap();
        assert_eq!(json["calls"], 1);
        assert_eq!(json["apps"][1]["appId"], "shop");
        assert!(json.get("appsTruncated").is_none());
    }

    /// B-07：主体与每主体的 App 条目有上限，超出的计入 `other` / 只计入合计。
    #[test]
    fn bounded_tables() {
        let mut b = UsageBook::default();
        for i in 0..MAX_USAGE_APPS + 3 {
            b.record("local", None, &format!("app{i}"), UsageEvent::Wake);
        }
        let st = b.status();
        assert_eq!((st[0].apps.len(), st[0].total.wakes, st[0].apps_truncated), (MAX_USAGE_APPS, (MAX_USAGE_APPS + 3) as u64, true));

        let mut b = UsageBook::default();
        for i in 0..MAX_USAGE_SUBJECTS + 2 {
            b.record(&format!("agent:a{i}"), Some(&format!("a{i}")), "shop", UsageEvent::Wake);
        }
        let st = b.status();
        assert_eq!(st.len(), MAX_USAGE_SUBJECTS + 1);
        let other = st.iter().find(|s| s.subject == "other").expect("other");
        assert_eq!((other.total.wakes, other.agent.clone()), (2, None));
    }
}
