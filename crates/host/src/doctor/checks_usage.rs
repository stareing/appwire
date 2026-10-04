//! 调用方用量（第 16 项 P3）：各主体（Agent / 本机 / Hub API）的调用、唤醒与被限流次数，回答"谁唤醒了这个 App 多少次"；
//! 只读结果缓存（第 16 项 O3）的条目与命中统计。

use app_mcp_hub::{CacheLimits, CacheStatus, HubStatus};
use app_mcp_hub::usage::UsageStatus;
use serde_json::json;

use super::{Check, Level};

const ID: &str = "usage";
const TITLE: &str = "调用方用量";

/// 一个主体的一行描述：`agent:claude 调用 12 次、唤醒 3 次（shop 2、music 1）、被限流 1 次`。
fn describe(u: &UsageStatus) -> String {
    let mut text = format!("{} 调用 {} 次", u.subject, u.total.calls);
    if u.total.wakes > 0 {
        let by_app: Vec<String> =
            u.apps.iter().filter(|a| a.counts.wakes > 0).map(|a| format!("{} {}", a.app_id, a.counts.wakes)).collect();
        text.push_str(&format!("、唤醒 {} 次（{}）", u.total.wakes, by_app.join("、")));
    }
    if u.total.rate_limited > 0 {
        text.push_str(&format!("、被限流 {} 次", u.total.rate_limited));
    }
    text
}

/// 有被限流的主体为注意（配额 / 限流可能过紧，或 Agent 在循环调用）；有用量为信息；尚无调用为通过；旧 Host 不报告时跳过。
pub(super) fn usage_check(status: Option<&Result<HubStatus, String>>) -> Check {
    let Some(Ok(st)) = status else {
        return Check::new(ID, TITLE, Level::Skip, "Host 未运行或状态不可读");
    };
    let Some(usage) = &st.usage else {
        return Check::new(ID, TITLE, Level::Skip, "运行中的 Host 版本不报告用量");
    };
    let details = json!({ "usage": usage });
    if usage.is_empty() {
        return Check::new(ID, TITLE, Level::Ok, "启动以来没有调用").details(details);
    }
    let summary = usage.iter().map(describe).collect::<Vec<_>>().join("；");
    if usage.iter().any(|u| u.total.rate_limited > 0) {
        return Check::new(ID, TITLE, Level::Warn, summary)
            .hint("被限流的调用未执行：确认 Agent 没有循环调用，或调高 limits（toolRatePerMinute / appRatePerMinute / agentRatePerMinute）")
            .details(details);
    }
    Check::new(ID, TITLE, Level::Info, summary).details(details)
}

const CACHE_ID: &str = "cache";
const CACHE_TITLE: &str = "结果缓存";

/// 缓存统计的一句描述：`3 条（1.5 KiB），命中 10、未命中 4、淘汰 0`。
fn describe_cache(c: &CacheStatus) -> String {
    format!("{} 条（{:.1} KiB），命中 {}、未命中 {}、淘汰 {}", c.entries, c.bytes as f64 / 1024.0, c.hits, c.misses, c.evictions)
}

/// `app-mcp-host status` 摘要中的缓存部分；旧 Host 不报告时为空。
pub(crate) fn cache_text(st: &HubStatus) -> String {
    st.cache.as_ref().map(|c| format!("，结果缓存 {}", describe_cache(c))).unwrap_or_default()
}

/// 字节数的简短写法：不足 1 MiB 用 KiB。
fn describe_bytes(n: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if n >= MIB { format!("{:.1} MiB", n as f64 / MIB as f64) } else { format!("{:.1} KiB", n as f64 / 1024.0) }
}

/// 上限的一句描述：`上限 1024 条、8.0 MiB、单条 64.0 KiB`。
fn describe_limits(l: &CacheLimits) -> String {
    format!("上限 {} 条、{}、单条 {}", l.max_entries, describe_bytes(l.max_bytes), describe_bytes(l.max_entry_bytes))
}

/// 只读结果缓存（spec/hub-api.md 3.20）：有缓存活动为信息，尚无可缓存的请求为通过，按配置关闭为信息；旧 Host 不报告时跳过。
///
/// @input `limits`：本配置目录的设置解析出的上限（`resultCache`）；运行中的 Host 若以不同的命令行参数启动，以其为准。
pub(super) fn cache_check(status: Option<&Result<HubStatus, String>>, limits: &CacheLimits) -> Check {
    let Some(Ok(st)) = status else {
        return Check::new(CACHE_ID, CACHE_TITLE, Level::Skip, "Host 未运行或状态不可读");
    };
    let Some(cache) = &st.cache else {
        return Check::new(CACHE_ID, CACHE_TITLE, Level::Skip, "运行中的 Host 版本不报告结果缓存");
    };
    let details = json!({
        "cache": cache,
        "limits": {"maxEntries": limits.max_entries, "maxBytes": limits.max_bytes, "maxEntryBytes": limits.max_entry_bytes},
    });
    if !limits.enabled() {
        return Check::new(CACHE_ID, CACHE_TITLE, Level::Info, "已关闭（resultCache.maxEntries 为 0）").details(details);
    }
    if *cache == CacheStatus::default() {
        let summary = format!("启动以来没有可缓存的请求（App 未声明 cache）；{}", describe_limits(limits));
        return Check::new(CACHE_ID, CACHE_TITLE, Level::Ok, summary).details(details);
    }
    Check::new(CACHE_ID, CACHE_TITLE, Level::Info, format!("{}；{}", describe_cache(cache), describe_limits(limits)))
        .details(details)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(usage: serde_json::Value) -> HubStatus {
        let mut st: HubStatus = serde_json::from_value(json!({
            "service": "app-mcp", "version": "t", "user": "u", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
            "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 0, "apps": [], "reports": []
        }))
        .unwrap();
        st.usage = serde_json::from_value(usage).unwrap();
        st
    }

    #[test]
    fn levels_and_summary() {
        let st = status(json!([
            {"subject": "agent:claude", "agent": "claude", "calls": 12, "wakes": 3, "rateLimited": 0, "argumentsBytes": 1, "resultBytes": 2,
             "apps": [{"appId": "music", "calls": 0, "wakes": 1, "rateLimited": 0, "argumentsBytes": 0, "resultBytes": 0},
                      {"appId": "shop", "calls": 12, "wakes": 2, "rateLimited": 0, "argumentsBytes": 1, "resultBytes": 2}]},
            {"subject": "local", "calls": 5, "wakes": 0, "rateLimited": 0, "argumentsBytes": 0, "resultBytes": 0, "apps": []}
        ]));
        let c = usage_check(Some(&Ok(st)));
        assert!(matches!(c.status, Level::Info), "{c:?}");
        assert_eq!(c.summary, "agent:claude 调用 12 次、唤醒 3 次（music 1、shop 2）；local 调用 5 次");

        let limited = status(json!([{"subject": "agent:bot", "agent": "bot", "calls": 2, "wakes": 0, "rateLimited": 4,
            "argumentsBytes": 0, "resultBytes": 0, "apps": []}]));
        let c = usage_check(Some(&Ok(limited)));
        assert!(matches!(c.status, Level::Warn) && c.summary.ends_with("被限流 4 次"), "{c:?}");

        assert!(matches!(usage_check(Some(&Ok(status(json!([]))))).status, Level::Ok));
        assert!(matches!(usage_check(Some(&Ok(status(json!(null))))).status, Level::Skip));
        assert!(matches!(usage_check(None).status, Level::Skip));
    }

    #[test]
    fn cache_levels_and_text() {
        let mut st = status(json!(null));
        let limits = CacheLimits::default();
        assert!(matches!(cache_check(Some(&Ok(st.clone())), &limits).status, Level::Skip), "旧 Host 不报告");
        assert_eq!(cache_text(&st), "");
        st.cache = Some(CacheStatus::default());
        let c = cache_check(Some(&Ok(st.clone())), &limits);
        assert!(matches!(c.status, Level::Ok), "{c:?}");
        assert!(c.summary.ends_with("；上限 1024 条、8.0 MiB、单条 64.0 KiB"), "{c:?}");
        assert_eq!(c.details["limits"], json!({"maxEntries": 1024, "maxBytes": 8_388_608, "maxEntryBytes": 65_536}));
        st.cache = Some(CacheStatus { entries: 3, bytes: 1536, hits: 10, misses: 4, evictions: 1 });
        let c = cache_check(Some(&Ok(st.clone())), &limits);
        assert!(matches!(c.status, Level::Info), "{c:?}");
        assert_eq!(c.summary, "3 条（1.5 KiB），命中 10、未命中 4、淘汰 1；上限 1024 条、8.0 MiB、单条 64.0 KiB");
        assert_eq!(cache_text(&st), "，结果缓存 3 条（1.5 KiB），命中 10、未命中 4、淘汰 1");
        assert!(matches!(cache_check(None, &limits).status, Level::Skip));
        let off = CacheLimits { max_entries: 0, ..limits };
        let c = cache_check(Some(&Ok(st.clone())), &off);
        assert!(matches!(c.status, Level::Info) && c.summary.starts_with("已关闭"), "{c:?}");
        assert_eq!(c.details["limits"]["maxEntries"], 0);
    }
}
