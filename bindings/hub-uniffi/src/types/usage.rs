//! 按调用方记账（第 16 项 P3，`HubStatus.usage`）的 uniffi 记录。

use app_mcp_hub as hub;

/// 一个记账主体的用量：`agent:<名>` / `local` / `api` / `other`（主体数达上限后）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UsageStatus {
    pub subject: String,
    /// 主体为已登记 Agent 时的名字。
    #[uniffi(default = None)]
    pub agent: Option<String>,
    pub total: UsageCounts,
    /// 按 App 细分，按 appId 排序。
    pub apps: Vec<AppUsageStatus>,
    /// 细分条目达到上限后，新 App 的用量只计入合计。
    pub apps_truncated: bool,
}

/// 计数（主体合计与每 App 共用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UsageCounts {
    pub calls: u64,
    /// 为该主体发起的唤醒（通过唤醒策略之后；与进行中的唤醒合并的也计入）。
    pub wakes: u64,
    pub rate_limited: u64,
    pub arguments_bytes: u64,
    pub result_bytes: u64,
}

/// `UsageStatus.apps` 的一项。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AppUsageStatus {
    pub app_id: String,
    pub counts: UsageCounts,
}

impl From<hub::usage::UsageCounts> for UsageCounts {
    fn from(c: hub::usage::UsageCounts) -> Self {
        UsageCounts {
            calls: c.calls,
            wakes: c.wakes,
            rate_limited: c.rate_limited,
            arguments_bytes: c.arguments_bytes,
            result_bytes: c.result_bytes,
        }
    }
}

impl From<hub::usage::UsageStatus> for UsageStatus {
    fn from(u: hub::usage::UsageStatus) -> Self {
        UsageStatus {
            subject: u.subject,
            agent: u.agent,
            total: u.total.into(),
            apps: u.apps.into_iter().map(|a| AppUsageStatus { app_id: a.app_id, counts: a.counts.into() }).collect(),
            apps_truncated: u.apps_truncated,
        }
    }
}
