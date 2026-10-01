//! 资源保护（spec/hub-api.md 3.11；docs/plans/14-safety.md S3 / S4）：调用频率与数据大小上限。
//!
//! - **限流**：令牌桶，按（App, 工具）与按 App 两级；两级都有令牌时各扣一个，任一级不足时都不扣并返回
//!   `RATE_LIMITED`（`retryAfterMs` = 不足一级攒够一个令牌还需的时长，两级取较长者）。
//! - **大小上限**：调用参数、调用结果、资源内容的单项上限（序列化后的 JSON 字节数）；超出返回
//!   `PAYLOAD_TOO_LARGE`，不截断。
//!
//! 本模块是纯状态（不做 I/O、不读时钟）：时刻由调用方传入，便于确定性测试。上限检查的调用点在 [`crate::call`]。
//!
//! @invariant 令牌桶表最多 [`MAX_RATE_BUCKETS`] 个、计数表最多 [`MAX_COUNTED_APPS`] 个 App：令牌桶表满时先移除已攒满的桶
//! （与新建的桶等价），仍满则移除最久未用的桶；计数表满时移除计数最少的 App。

use std::collections::HashMap;
use std::time::Duration;

use app_mcp_protocol::{ErrorKind, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::Instant;

/// 令牌桶表的上限。
pub(crate) const MAX_RATE_BUCKETS: usize = 4096;

/// 拒绝计数表的 App 上限。
pub(crate) const MAX_COUNTED_APPS: usize = 1024;

/// 一级限流：每分钟补充 `per_minute` 个令牌，最多攒 `burst` 个（允许的突发调用数）。`per_minute = 0` 表示不限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    pub per_minute: u32,
    pub burst: u32,
}

impl RateLimit {
    pub const UNLIMITED: RateLimit = RateLimit { per_minute: 0, burst: 0 };

    pub fn is_unlimited(&self) -> bool {
        self.per_minute == 0
    }

    fn per_sec(&self) -> f64 {
        f64::from(self.per_minute) / 60.0
    }

    fn capacity(&self) -> f64 {
        f64::from(self.burst)
    }
}

/// 结果与其 `outputSchema` 不符时 Hub 的处理（第 19 项 R2）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputValidation {
    /// 不校验。
    Off,
    /// 校验，不符时只记日志（默认；App 的声明可能不准，不让调用因此失败）。
    #[default]
    Log,
    /// 校验，不符时调用以 `HANDLER_ERROR` 结束。
    Reject,
}

/// 资源保护策略（`HubConfig.limits`）。JSON 配置形式见 [`LimitOverrides`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitPolicy {
    /// 每（App, 工具）的调用频率。
    pub tool_rate: RateLimit,
    /// 每 App（所有工具合计）的调用频率。
    pub app_rate: RateLimit,
    /// 调用参数（JSON）的字节上限；0 = 不限。
    pub max_arguments_bytes: u64,
    /// 调用结果（App 回给 Hub 的整个结果 JSON，含摘要）的字节上限；0 = 不限。
    pub max_result_bytes: u64,
    /// 资源内容的字节上限；0 = 不限。
    pub max_resource_bytes: u64,
}

/// 1 MiB。
const MIB: u64 = 1024 * 1024;

impl Default for LimitPolicy {
    /// @why 默认值宽松，只拦失控循环与异常数据，不影响正常使用：模型逐次调用一般每秒不到 1 次，单工具每分钟 120 次、
    /// 突发 30 次与每 App 每分钟 600 次、突发 60 次都远高于此；参数 1 MiB 远大于模型生成的参数，结果 / 资源 4 MiB
    /// 约等于百万 token 级上下文，再大模型也无法使用；均低于 WebSocket 单条消息 64 MiB 的上限（超过那个上限连接会被断开）。
    fn default() -> Self {
        Self {
            tool_rate: RateLimit { per_minute: 120, burst: 30 },
            app_rate: RateLimit { per_minute: 600, burst: 60 },
            max_arguments_bytes: MIB,
            max_result_bytes: 4 * MIB,
            max_resource_bytes: 4 * MIB,
        }
    }
}

impl LimitPolicy {
    /// 不限流、不限大小。
    pub fn unlimited() -> Self {
        Self {
            tool_rate: RateLimit::UNLIMITED,
            app_rate: RateLimit::UNLIMITED,
            max_arguments_bytes: 0,
            max_result_bytes: 0,
            max_resource_bytes: 0,
        }
    }

    /// 校验配置（`Hub::start` 调用）；不合法时返回中文说明。
    pub fn validate(&self) -> Result<(), String> {
        for (name, r) in [("tool", self.tool_rate), ("app", self.app_rate)] {
            if !r.is_unlimited() && r.burst == 0 {
                return Err(format!("limits.{name}RateBurst 必须 ≥ 1（{name}RatePerMinute 为 0 时表示不限）"));
            }
        }
        Ok(())
    }
}

/// [`LimitPolicy`] 的 JSON 配置形式（各绑定与 `app-mcp-host` 配置文件共用，spec/hub-api.md 3.11）：
/// `{"toolRatePerMinute", "toolRateBurst", "appRatePerMinute", "appRateBurst", "maxArgumentsBytes", "maxResultBytes",
/// "maxResourceBytes"}`，缺省字段取默认值；`/status` 的 `limits` 也是这一形式（全部字段给出）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct LimitOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_rate_per_minute: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_rate_burst: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_rate_per_minute: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_rate_burst: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_arguments_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_result_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_resource_bytes: Option<u64>,
}

impl LimitOverrides {
    /// 把给出的字段写入策略。
    pub fn apply(&self, p: &mut LimitPolicy) {
        let set32 = |t: &mut u32, v: Option<u32>| {
            if let Some(v) = v {
                *t = v;
            }
        };
        let set64 = |t: &mut u64, v: Option<u64>| {
            if let Some(v) = v {
                *t = v;
            }
        };
        set32(&mut p.tool_rate.per_minute, self.tool_rate_per_minute);
        set32(&mut p.tool_rate.burst, self.tool_rate_burst);
        set32(&mut p.app_rate.per_minute, self.app_rate_per_minute);
        set32(&mut p.app_rate.burst, self.app_rate_burst);
        set64(&mut p.max_arguments_bytes, self.max_arguments_bytes);
        set64(&mut p.max_result_bytes, self.max_result_bytes);
        set64(&mut p.max_resource_bytes, self.max_resource_bytes);
    }

    /// 合并：`other` 中给出的字段覆盖本对象（配置文件 ← 命令行）。
    pub fn merge(&mut self, other: &LimitOverrides) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f; } )* };
        }
        take!(
            tool_rate_per_minute,
            tool_rate_burst,
            app_rate_per_minute,
            app_rate_burst,
            max_arguments_bytes,
            max_result_bytes,
            max_resource_bytes
        );
    }

    /// 策略的完整 JSON 形式（全部字段给出）。
    pub fn from_policy(p: &LimitPolicy) -> Self {
        Self {
            tool_rate_per_minute: Some(p.tool_rate.per_minute),
            tool_rate_burst: Some(p.tool_rate.burst),
            app_rate_per_minute: Some(p.app_rate.per_minute),
            app_rate_burst: Some(p.app_rate.burst),
            max_arguments_bytes: Some(p.max_arguments_bytes),
            max_result_bytes: Some(p.max_result_bytes),
            max_resource_bytes: Some(p.max_resource_bytes),
        }
    }
}

/// 超出的是哪一级限流。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RateScope {
    Tool,
    App,
}

impl RateScope {
    fn as_str(self) -> &'static str {
        match self {
            RateScope::Tool => "tool",
            RateScope::App => "app",
        }
    }
}

/// 一次限流拒绝。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RateLimited {
    pub scope: RateScope,
    pub limit: RateLimit,
    pub retry_after: Duration,
}

impl RateLimited {
    /// 返回给调用方的错误（`RATE_LIMITED`，spec/protocol.md 第 4 节）。
    pub fn to_error(self, app_id: &str, tool: &str) -> ToolError {
        let retry_ms = u64::try_from(self.retry_after.as_millis()).unwrap_or(u64::MAX).max(1);
        let secs = retry_ms.div_ceil(1000);
        let target = match self.scope {
            RateScope::Tool => format!("工具「{app_id}.{tool}」"),
            RateScope::App => format!("App「{app_id}」"),
        };
        ToolError::new(
            ErrorKind::RateLimited,
            format!(
                "对{target}的调用过于频繁（上限每分钟 {} 次、突发 {} 次），本次调用未执行。请约 {secs} 秒后重试。",
                self.limit.per_minute, self.limit.burst
            ),
        )
        .with_details(json!({
            "appId": app_id,
            "tool": tool,
            "scope": self.scope.as_str(),
            "retryAfterMs": retry_ms,
            "perMinute": self.limit.per_minute,
            "burst": self.limit.burst,
        }))
    }
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    /// 补充到 `now` 后的令牌数（不修改桶）。
    fn level(&self, limit: &RateLimit, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        (self.tokens + elapsed * limit.per_sec()).min(limit.capacity())
    }
}

/// 每 App 的拒绝计数（`/status` 的 `rateLimited` / `tooLarge`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LimitCounters {
    pub rate_limited: u64,
    pub too_large: u64,
}

type BucketKey = (String, Option<String>);

/// 限流状态与拒绝计数。
#[derive(Debug, Default)]
pub(crate) struct RateBook {
    buckets: HashMap<BucketKey, Bucket>,
    counters: HashMap<String, LimitCounters>,
}

impl RateBook {
    /// 尝试为一次调用取得令牌（两级都通过才扣）。拒绝时计入该 App 的 `rate_limited`。
    pub fn acquire(&mut self, policy: &LimitPolicy, app_id: &str, tool: &str, now: Instant) -> Result<(), RateLimited> {
        let levels = [
            (RateScope::Tool, policy.tool_rate, (app_id.to_owned(), Some(tool.to_owned()))),
            (RateScope::App, policy.app_rate, (app_id.to_owned(), None)),
        ];
        let mut denied: Option<RateLimited> = None;
        for (scope, limit, key) in &levels {
            if limit.is_unlimited() {
                continue;
            }
            let level = self.buckets.get(key).map_or(limit.capacity(), |b| b.level(limit, now));
            if level < 1.0 {
                let retry_after = Duration::from_secs_f64((1.0 - level) / limit.per_sec());
                if denied.is_none_or(|d| retry_after > d.retry_after) {
                    denied = Some(RateLimited { scope: *scope, limit: *limit, retry_after });
                }
            }
        }
        if let Some(d) = denied {
            self.counter(app_id).rate_limited += 1;
            return Err(d);
        }
        for (_, limit, key) in levels {
            if limit.is_unlimited() {
                continue;
            }
            let level = self.buckets.get(&key).map_or(limit.capacity(), |b| b.level(&limit, now));
            if !self.buckets.contains_key(&key) {
                self.evict_bucket(policy, now);
            }
            self.buckets.insert(key, Bucket { tokens: level - 1.0, at: now });
        }
        Ok(())
    }

    /// 记一次大小超限。
    pub fn record_too_large(&mut self, app_id: &str) {
        self.counter(app_id).too_large += 1;
    }

    pub fn counters(&self, app_id: &str) -> LimitCounters {
        self.counters.get(app_id).copied().unwrap_or_default()
    }

    fn counter(&mut self, app_id: &str) -> &mut LimitCounters {
        if !self.counters.contains_key(app_id) && self.counters.len() >= MAX_COUNTED_APPS {
            let least = self
                .counters
                .iter()
                .min_by_key(|(_, c)| c.rate_limited.saturating_add(c.too_large))
                .map(|(k, _)| k.clone());
            if let Some(k) = least {
                self.counters.remove(&k);
            }
        }
        self.counters.entry(app_id.to_owned()).or_default()
    }

    fn evict_bucket(&mut self, policy: &LimitPolicy, now: Instant) {
        if self.buckets.len() < MAX_RATE_BUCKETS {
            return;
        }
        self.buckets.retain(|(_, tool), b| {
            let limit = if tool.is_some() { &policy.tool_rate } else { &policy.app_rate };
            b.level(limit, now) < limit.capacity()
        });
        if self.buckets.len() >= MAX_RATE_BUCKETS {
            let oldest = self.buckets.iter().min_by_key(|(_, b)| b.at).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.buckets.remove(&k);
            }
        }
    }

    #[cfg(test)]
    fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
}

/// 受大小上限约束的数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Payload {
    Arguments,
    Result,
    Resource,
}

impl Payload {
    fn as_str(self) -> &'static str {
        match self {
            Payload::Arguments => "arguments",
            Payload::Result => "result",
            Payload::Resource => "resource",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Payload::Arguments => "调用参数",
            Payload::Result => "调用结果",
            Payload::Resource => "资源内容",
        }
    }

    fn advice(self) -> &'static str {
        match self {
            Payload::Arguments => "请缩小参数（例如分批提交）后重试。",
            Payload::Result => "调用可能已在 App 内执行，但结果未返回；请让 App 分页或精简返回内容，确认状态后再决定是否重试。",
            Payload::Resource => "请让 App 分页或精简资源内容。",
        }
    }

    /// 检查大小：`limit = 0` 表示不限。超出时返回 `PAYLOAD_TOO_LARGE`（spec/protocol.md 第 4 节）。
    pub fn check(self, size: usize, limit: u64, target: &str) -> Result<(), ToolError> {
        let size = u64::try_from(size).unwrap_or(u64::MAX);
        if limit == 0 || size <= limit {
            return Ok(());
        }
        Err(ToolError::new(
            ErrorKind::PayloadTooLarge,
            format!(
                "{target}的{}有 {size} 字节，超过 Host 的上限 {limit} 字节，未{}（不截断）。{}",
                self.label(),
                if self == Payload::Arguments { "转发" } else { "返回" },
                self.advice()
            ),
        )
        .with_details(json!({ "part": self.as_str(), "sizeBytes": size, "limitBytes": limit })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(tool: (u32, u32), app: (u32, u32)) -> LimitPolicy {
        LimitPolicy {
            tool_rate: RateLimit { per_minute: tool.0, burst: tool.1 },
            app_rate: RateLimit { per_minute: app.0, burst: app.1 },
            ..LimitPolicy::default()
        }
    }

    #[test]
    fn burst_then_refill() {
        let p = policy((60, 2), (0, 0));
        let mut b = RateBook::default();
        let t0 = Instant::now();
        assert!(b.acquire(&p, "a", "x", t0).is_ok());
        assert!(b.acquire(&p, "a", "x", t0).is_ok());
        let e = b.acquire(&p, "a", "x", t0).unwrap_err();
        assert_eq!((e.scope, e.retry_after), (RateScope::Tool, Duration::from_secs(1)));
        // 每秒补 1 个
        assert!(b.acquire(&p, "a", "x", t0 + Duration::from_secs(1)).is_ok());
        assert!(b.acquire(&p, "a", "x", t0 + Duration::from_secs(1)).is_err());
        // 其他工具不受影响；攒满后不超过 burst
        assert!(b.acquire(&p, "a", "y", t0).is_ok());
        let later = t0 + Duration::from_secs(3600);
        assert!(b.acquire(&p, "a", "x", later).is_ok());
        assert!(b.acquire(&p, "a", "x", later).is_ok());
        assert!(b.acquire(&p, "a", "x", later).is_err());
        assert_eq!(b.counters("a"), LimitCounters { rate_limited: 3, too_large: 0 });
    }

    #[test]
    fn app_level_limits_all_tools_and_denial_consumes_nothing() {
        let p = policy((60, 5), (60, 2));
        let mut b = RateBook::default();
        let t0 = Instant::now();
        assert!(b.acquire(&p, "a", "x", t0).is_ok());
        assert!(b.acquire(&p, "a", "y", t0).is_ok());
        let e = b.acquire(&p, "a", "z", t0).unwrap_err();
        assert_eq!(e.scope, RateScope::App);
        // 被拒的调用没有扣工具级令牌：z 仍有 5 个
        let t1 = t0 + Duration::from_secs(1);
        assert!(b.acquire(&p, "a", "z", t1).is_ok());
        // 其他 App 不受影响
        assert!(b.acquire(&p, "b", "x", t0).is_ok());
    }

    #[test]
    fn longer_wait_wins_and_unlimited_skips() {
        // 工具级缺 1 个需 1 s，App 级缺 1 个需 2 s → 报 App 级
        let p = policy((60, 1), (30, 1));
        let mut b = RateBook::default();
        let t0 = Instant::now();
        assert!(b.acquire(&p, "a", "x", t0).is_ok());
        let e = b.acquire(&p, "a", "x", t0).unwrap_err();
        assert_eq!((e.scope, e.retry_after), (RateScope::App, Duration::from_secs(2)));
        let mut b = RateBook::default();
        let unlimited = LimitPolicy::unlimited();
        for _ in 0..1000 {
            assert!(b.acquire(&unlimited, "a", "x", t0).is_ok());
        }
        assert_eq!(b.bucket_count(), 0);
    }

    #[test]
    fn rate_error_has_retry_after() {
        let e = RateLimited { scope: RateScope::Tool, limit: RateLimit { per_minute: 60, burst: 2 }, retry_after: Duration::from_millis(1500) }
            .to_error("shop", "cart.add");
        assert_eq!(e.kind, ErrorKind::RateLimited);
        let d = e.details.unwrap();
        assert_eq!((d["retryAfterMs"].as_u64(), d["scope"].as_str()), (Some(1500), Some("tool")));
        assert!(e.message.contains("shop.cart.add") && e.message.contains("2 秒"), "{}", e.message);
        let e = RateLimited { scope: RateScope::App, limit: RateLimit { per_minute: 1, burst: 1 }, retry_after: Duration::ZERO }
            .to_error("shop", "x");
        assert_eq!(e.details.unwrap()["retryAfterMs"], 1);
        assert!(e.message.contains("App「shop」"));
    }

    #[test]
    fn bucket_table_is_bounded() {
        let p = policy((60, 1), (0, 0));
        let mut b = RateBook::default();
        let t0 = Instant::now();
        for i in 0..MAX_RATE_BUCKETS {
            assert!(b.acquire(&p, "a", &format!("t{i}"), t0).is_ok());
        }
        assert_eq!(b.bucket_count(), MAX_RATE_BUCKETS);
        // 都未攒满：移除最久未用的一个
        assert!(b.acquire(&p, "a", "new", t0).is_ok());
        assert_eq!(b.bucket_count(), MAX_RATE_BUCKETS);
        // 一小时后都已攒满（与新建等价）：全部移除
        assert!(b.acquire(&p, "a", "later", t0 + Duration::from_secs(3600)).is_ok());
        assert_eq!(b.bucket_count(), 1);
    }

    #[test]
    fn counter_table_is_bounded() {
        let mut b = RateBook::default();
        for i in 0..MAX_COUNTED_APPS + 5 {
            b.record_too_large(&format!("app{i}"));
        }
        assert!(b.counters.len() <= MAX_COUNTED_APPS);
        b.record_too_large("x");
        b.record_too_large("x");
        assert_eq!(b.counters("x").too_large, 2);
        assert_eq!(b.counters("unknown"), LimitCounters::default());
    }

    #[test]
    fn size_check() {
        assert!(Payload::Arguments.check(10, 10, "工具「a.b」").is_ok());
        assert!(Payload::Result.check(usize::MAX, 0, "x").is_ok(), "0 = 不限");
        let e = Payload::Resource.check(11, 10, "资源「r」").unwrap_err();
        assert_eq!(e.kind, ErrorKind::PayloadTooLarge);
        let d = e.details.unwrap();
        assert_eq!((d["part"].as_str(), d["sizeBytes"].as_u64(), d["limitBytes"].as_u64()), (Some("resource"), Some(11), Some(10)));
        let e = Payload::Arguments.check(11, 10, "工具「a.b」").unwrap_err();
        assert!(e.message.contains("未转发"), "{}", e.message);
        let e = Payload::Result.check(11, 10, "工具「a.b」").unwrap_err();
        assert!(e.message.contains("未返回") && e.message.contains("可能已在 App 内执行"), "{}", e.message);
    }

    #[test]
    fn policy_validation_and_overrides() {
        assert!(LimitPolicy::default().validate().is_ok());
        assert!(LimitPolicy::unlimited().validate().is_ok());
        let bad = policy((10, 0), (0, 0));
        assert!(bad.validate().unwrap_err().contains("toolRateBurst"));
        let bad = policy((0, 0), (10, 0));
        assert!(bad.validate().unwrap_err().contains("appRateBurst"));

        let o: LimitOverrides = serde_json::from_str(r#"{"toolRatePerMinute":0,"maxResultBytes":5}"#).unwrap();
        let mut p = LimitPolicy::default();
        o.apply(&mut p);
        assert!(p.tool_rate.is_unlimited());
        assert_eq!((p.max_result_bytes, p.max_arguments_bytes), (5, MIB));
        let mut a = o.clone();
        a.merge(&LimitOverrides { max_result_bytes: Some(7), app_rate_burst: Some(3), ..Default::default() });
        assert_eq!((a.max_result_bytes, a.app_rate_burst, a.tool_rate_per_minute), (Some(7), Some(3), Some(0)));
        let full = LimitOverrides::from_policy(&LimitPolicy::default());
        let mut back = LimitPolicy::unlimited();
        full.apply(&mut back);
        assert_eq!(back, LimitPolicy::default());
        assert!(serde_json::from_str::<LimitOverrides>(r#"{"bogus":1}"#).is_err());
        assert_eq!(serde_json::to_value(OutputValidation::Reject).unwrap(), "reject");
        assert_eq!(OutputValidation::default(), OutputValidation::Log);
    }
}
