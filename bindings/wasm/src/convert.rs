//! JS 对象 ↔ 核心类型的转换。与 wasm-bindgen 无关，可在原生目标上测试。
//!
//! 输入（配置、定义、结果）由 [`FromJson`] 从 JSON 值手工读取：serde 的 `derive(Deserialize)` 会为每个结构
//! 各生成一套对象 / 数组两种形式的反序列化代码，是 WASM 体积的主要来源之一。协议类型（`AppOverview`、
//! `WakeDescriptor`、各枚举）仍用其 serde 定义，保持单一来源。输出（状态、事件）用 serde 序列化。

use app_mcp_core::{
    CallDedupPolicy, CallOutput, CancelReason, ClientConfig, ClientKind, ConnectionState, Event, HeartbeatMode,
    HeartbeatPolicy, LifecycleMode, LifecyclePolicy, ReconnectPolicy, Residency, ResourceDef, ScopeId, SleepReason, ToolDef, ToolError,
    ToolUpdate, TransportKind, Visibility, WakeReason,
};
use app_mcp_core::{
    Activation, AppOverview, Audience, ContentAnnotations, ResultStatus, Risk, ToolAnnotations, WakeDescriptor,
};
use app_mcp_protocol::ErrorKind;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// JS 能精确表示的最大整数（2^53 - 1）。
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// JS 数字句柄 → u64。非整数、负数或超出安全整数范围视为无效。
///
/// @error `无效的<what>：<数值>`。
/// @why 数值经 serde_json 的 `Number` 格式化（复用已链接的 zmij），不引入 `f64` 的 `Display`（体积）。
pub fn handle(id: f64, what: &str) -> Result<u64, String> {
    if id.is_finite() && id >= 0.0 && id.fract() == 0.0 && id <= MAX_SAFE_INTEGER as f64 {
        return Ok(id as u64);
    }
    let shown = match serde_json::Number::from_f64(id) {
        Some(n) => n.to_string(),
        None if id.is_nan() => "NaN".to_owned(),
        None if id > 0.0 => "Infinity".to_owned(),
        None => "-Infinity".to_owned(),
    };
    Err(format!("无效的{what}：{shown}"))
}

fn scope_handle(scope: Option<f64>) -> Result<Option<ScopeId>, String> {
    scope.map(|s| handle(s, " scope 句柄").map(ScopeId)).transpose()
}

pub fn parse_wake_reason(s: &str) -> Result<WakeReason, String> {
    match s {
        "os-activation" => Ok(WakeReason::OsActivation),
        "app" => Ok(WakeReason::App),
        "visible" => Ok(WakeReason::Visible),
        "cold-start" => Ok(WakeReason::ColdStart),
        other => Err(format!("无效的唤醒原因：\"{other}\"")),
    }
}

pub fn parse_sleep_reason(s: &str) -> Result<SleepReason, String> {
    match s {
        "idle" => Ok(SleepReason::Idle),
        "grace" => Ok(SleepReason::Grace),
        "background" => Ok(SleepReason::Background),
        "app" => Ok(SleepReason::App),
        other => Err(format!("无效的休眠原因：\"{other}\"")),
    }
}

pub fn parse_visibility(s: &str) -> Result<Visibility, String> {
    match s {
        "visible" => Ok(Visibility::Visible),
        "hidden" => Ok(Visibility::Hidden),
        "frozen" => Ok(Visibility::Frozen),
        other => Err(format!("无效的可见性：\"{other}\"")),
    }
}

// ---------------------------------------------------------------------------
// JSON 输入读取
// ---------------------------------------------------------------------------

/// 从 JS 传入的 JSON 值构造。
///
/// @error 返回中文说明（不含"格式错误"前缀，由调用方加上所在参数名）。
pub trait FromJson: Sized {
    fn from_json(value: Value) -> Result<Self, String>;
}

/// JSON 对象的字段读取器：逐个取走字段；`null` 与缺省等同（与 serde 的 `Option` 一致），未知字段忽略。
/// 读取失败时记下第一条错误并返回缺省值，读完后由 [`Fields::finish`] 统一返回。
///
/// @error 类型不符：`字段 <key> 应为<类型>`；必填字段缺失：`缺少字段 <key>`；嵌套对象的错误前加 `<key>.`。
/// @why 不在每个字段上 `?` 提前返回：每个提前返回点都要生成一份"释放已读字段"的代码（WASM 体积）。
struct Fields {
    map: Map<String, Value>,
    error: Option<String>,
}

impl Fields {
    fn new(value: Value) -> Result<Self, String> {
        match value {
            Value::Object(map) => Ok(Fields { map, error: None }),
            _ => Err("应为对象".to_owned()),
        }
    }

    /// 记录错误（只保留第一条）。
    fn fail(&mut self, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
    }

    /// 读完所有字段后调用：有错误时返回第一条，否则返回 `value`。
    fn finish<T>(self, value: T) -> Result<T, String> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(value),
        }
    }

    fn take(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key).filter(|v| !v.is_null())
    }

    fn expect<T>(&mut self, key: &str, kind: &str, read: impl FnOnce(Value) -> Option<T>) -> Option<T> {
        let read = read(self.take(key)?);
        if read.is_none() {
            self.fail(format!("字段 {key} 应为{kind}"));
        }
        read
    }

    fn string(&mut self, key: &str) -> Option<String> {
        self.expect(key, "字符串", |v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
    }

    /// 缺失时记录错误并返回空字符串。
    fn required_string(&mut self, key: &str) -> String {
        let Some(s) = self.string(key) else {
            self.fail(format!("缺少字段 {key}"));
            return String::new();
        };
        s
    }

    fn u64(&mut self, key: &str) -> Option<u64> {
        self.expect(key, "非负整数", |v| v.as_u64())
    }

    fn f64(&mut self, key: &str) -> Option<f64> {
        self.expect(key, "数字", |v| v.as_f64())
    }

    fn bool(&mut self, key: &str) -> Option<bool> {
        self.expect(key, "布尔值", |v| v.as_bool())
    }

    fn strings(&mut self, key: &str) -> Option<Vec<String>> {
        self.expect(key, "字符串数组", |v| match v {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect(),
            _ => None,
        })
    }

    /// 任意 JSON 值（`null` 视为缺省）。
    fn value(&mut self, key: &str) -> Option<Value> {
        self.take(key)
    }

    /// 区分缺省（`None`）与显式 `null`（`Some(None)`）。
    fn nullable(&mut self, key: &str) -> Option<Option<Value>> {
        self.map.remove(key).map(|v| (!v.is_null()).then_some(v))
    }

    /// 嵌套对象，由 `T` 读取。
    fn object<T: FromJson>(&mut self, key: &str) -> Option<T> {
        match T::from_json(self.take(key)?) {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(format!("{key}.{e}"));
                None
            }
        }
    }

    /// 协议类型（枚举、`AppOverview`、`WakeDescriptor`）：沿用其 serde 定义。
    fn protocol<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        let value = self.take(key)?;
        self.protocol_value(key, value)
    }

    fn protocol_value<T: DeserializeOwned>(&mut self, key: &str, value: Value) -> Option<T> {
        match serde_json::from_value(value) {
            Ok(v) => Some(v),
            Err(e) => {
                self.fail(format!("字段 {key}：{e}"));
                None
            }
        }
    }

    /// 字符串枚举：`parse` 不认识时记录 `<无效说明>："<值>"`。
    fn keyword<T>(&mut self, key: &str, invalid: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
        let s = self.string(key)?;
        let parsed = parse(&s);
        if parsed.is_none() {
            self.fail(format!("{invalid}：\"{s}\""));
        }
        parsed
    }
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsReconnect {
    pub initial_delay_ms: Option<u64>,
    pub max_delay_ms: Option<u64>,
    pub multiplier: Option<f64>,
}

impl FromJson for JsReconnect {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let r = JsReconnect {
            initial_delay_ms: f.u64("initialDelayMs"),
            max_delay_ms: f.u64("maxDelayMs"),
            multiplier: f.f64("multiplier"),
        };
        f.finish(r)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsHeartbeat {
    /// `'auto' | 'always' | 'off'`（spec/lifecycle.md 第 11 节）。
    pub mode: Option<HeartbeatMode>,
    pub interval_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub hidden_timeout_ms: Option<u64>,
}

impl FromJson for JsHeartbeat {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let h = JsHeartbeat {
            mode: f.keyword("mode", "无效的心跳策略", parse_heartbeat_mode),
            interval_ms: f.u64("intervalMs"),
            timeout_ms: f.u64("timeoutMs"),
            hidden_timeout_ms: f.u64("hiddenTimeoutMs"),
        };
        f.finish(h)
    }
}

/// 调用去重（spec/protocol.md 3.3）：`{ ttlMs?, maxEntries? }`，缺省字段取默认值；任一为 0 关闭。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsCallDedup {
    pub ttl_ms: Option<u64>,
    pub max_entries: Option<u64>,
}

impl FromJson for JsCallDedup {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let d = JsCallDedup { ttl_ms: f.u64("ttlMs"), max_entries: f.u64("maxEntries") };
        f.finish(d)
    }
}

impl JsCallDedup {
    fn into_core(self) -> CallDedupPolicy {
        let d = CallDedupPolicy::default();
        CallDedupPolicy {
            ttl_ms: self.ttl_ms.unwrap_or(d.ttl_ms),
            max_entries: self.max_entries.map_or(d.max_entries, |n| usize::try_from(n).unwrap_or(usize::MAX)),
        }
    }
}

fn parse_heartbeat_mode(s: &str) -> Option<HeartbeatMode> {
    match s {
        "auto" => Some(HeartbeatMode::Auto),
        "always" => Some(HeartbeatMode::Always),
        "off" => Some(HeartbeatMode::Off),
        _ => None,
    }
}

/// `'ipc' | 'loopback' | 'remote'`：驱动层按 Host 地址判定的传输类别（spec/lifecycle.md 第 11 节）。
fn parse_transport(s: &str) -> Option<TransportKind> {
    match s {
        "ipc" => Some(TransportKind::Ipc),
        "loopback" => Some(TransportKind::Loopback),
        "remote" => Some(TransportKind::Remote),
        _ => None,
    }
}

/// `'persistent' | 'idle' | 'on-demand'`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsLifecycleMode {
    Persistent,
    Idle,
    OnDemand,
}

impl JsLifecycleMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "persistent" => Some(JsLifecycleMode::Persistent),
            "idle" => Some(JsLifecycleMode::Idle),
            "on-demand" => Some(JsLifecycleMode::OnDemand),
            _ => None,
        }
    }
}

/// `'keep' | 'exit-when-idle' | 'exit-always'`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsResidency {
    Keep,
    ExitWhenIdle,
    ExitAlways,
}

impl JsResidency {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "keep" => Some(JsResidency::Keep),
            "exit-when-idle" => Some(JsResidency::ExitWhenIdle),
            "exit-always" => Some(JsResidency::ExitAlways),
            _ => None,
        }
    }
}

/// 生命周期策略（spec/lifecycle.md 第 3 节）。未提供的字段取核心默认值。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsLifecycle {
    pub mode: Option<JsLifecycleMode>,
    pub idle_timeout_ms: Option<u64>,
    pub hidden_idle_timeout_ms: Option<u64>,
    pub grace_ms: Option<u64>,
    pub residency: Option<JsResidency>,
    /// 形如 `{ kind: 'web-url', target: location.href, background: false }`。
    pub wake: Option<WakeDescriptor>,
    pub host_absent_retries: Option<u64>,
    pub legacy_timers: Option<bool>,
    pub merge_window_ms: Option<u64>,
    pub sleep_on_background: Option<bool>,
}

impl FromJson for JsLifecycle {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let l = JsLifecycle {
            mode: f.keyword("mode", "无效的生命周期模式", JsLifecycleMode::parse),
            residency: f.keyword("residency", "无效的驻留策略", JsResidency::parse),
            idle_timeout_ms: f.u64("idleTimeoutMs"),
            hidden_idle_timeout_ms: f.u64("hiddenIdleTimeoutMs"),
            grace_ms: f.u64("graceMs"),
            wake: f.protocol("wake"),
            host_absent_retries: f.u64("hostAbsentRetries"),
            legacy_timers: f.bool("legacyTimers"),
            merge_window_ms: f.u64("mergeWindowMs"),
            sleep_on_background: f.bool("sleepOnBackground"),
        };
        f.finish(l)
    }
}

impl JsLifecycle {
    pub fn into_core(self) -> LifecyclePolicy {
        let d = LifecyclePolicy::default();
        LifecyclePolicy {
            mode: match self.mode {
                None => d.mode,
                Some(JsLifecycleMode::Persistent) => LifecycleMode::Persistent,
                Some(JsLifecycleMode::Idle) => LifecycleMode::Idle,
                Some(JsLifecycleMode::OnDemand) => LifecycleMode::OnDemand,
            },
            idle_timeout_ms: self.idle_timeout_ms.unwrap_or(d.idle_timeout_ms),
            hidden_idle_timeout_ms: self.hidden_idle_timeout_ms.unwrap_or(d.hidden_idle_timeout_ms),
            grace_ms: self.grace_ms.unwrap_or(d.grace_ms),
            residency: match self.residency {
                None => d.residency,
                Some(JsResidency::Keep) => Residency::Keep,
                Some(JsResidency::ExitWhenIdle) => Residency::ExitWhenIdle,
                Some(JsResidency::ExitAlways) => Residency::ExitAlways,
            },
            wake: self.wake,
            host_absent_retries: self
                .host_absent_retries
                .map_or(d.host_absent_retries, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            legacy_timers: self.legacy_timers.unwrap_or(d.legacy_timers),
            merge_window_ms: self.merge_window_ms.unwrap_or(d.merge_window_ms),
            sleep_on_background: self.sleep_on_background.unwrap_or(d.sleep_on_background),
        }
    }
}

/// `new WasmClient(config)` 的配置对象。未提供的可选字段取核心默认值。
#[derive(Clone, Debug, PartialEq)]
pub struct JsConfig {
    pub app_id: String,
    pub app_name: String,
    pub instance_id: String,
    /// 缺省 `"web"`。
    pub client_kind: Option<ClientKind>,
    pub sdk_version: Option<String>,
    pub app_version: Option<String>,
    pub origin: Option<String>,
    pub instance_title: Option<String>,
    pub instance_url: Option<String>,
    pub token: Option<String>,
    pub launch_token: Option<String>,
    pub reconnect: Option<JsReconnect>,
    pub heartbeat: Option<JsHeartbeat>,
    pub max_concurrent_calls: Option<usize>,
    pub resource_update_throttle_ms: Option<u64>,
    /// App 总览，随 `app/hello` 发送（spec/protocol.md 第 7 节）。
    pub overview: Option<AppOverview>,
    /// 握手超时（毫秒），缺省 10000；0 表示不限。
    pub handshake_timeout_ms: Option<u64>,
    /// 生命周期策略，缺省 `persistent`。
    pub lifecycle: Option<JsLifecycle>,
    /// 传输类别，缺省未知（按远程处理，发心跳）。
    pub transport: Option<TransportKind>,
    /// 调用去重，缺省保留 5 分钟、最多 64 条。
    pub call_dedup: Option<JsCallDedup>,
}

impl FromJson for JsConfig {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let max_concurrent_calls = f.u64("maxConcurrentCalls").and_then(|n| {
            let n = usize::try_from(n).ok();
            if n.is_none() {
                f.fail("字段 maxConcurrentCalls 超出范围".to_owned());
            }
            n
        });
        let c = JsConfig {
            app_id: f.required_string("appId"),
            app_name: f.required_string("appName"),
            instance_id: f.required_string("instanceId"),
            client_kind: f.protocol("clientKind"),
            sdk_version: f.string("sdkVersion"),
            app_version: f.string("appVersion"),
            origin: f.string("origin"),
            instance_title: f.string("instanceTitle"),
            instance_url: f.string("instanceUrl"),
            token: f.string("token"),
            launch_token: f.string("launchToken"),
            reconnect: f.object("reconnect"),
            heartbeat: f.object("heartbeat"),
            max_concurrent_calls,
            resource_update_throttle_ms: f.u64("resourceUpdateThrottleMs"),
            overview: f.protocol("overview"),
            handshake_timeout_ms: f.u64("handshakeTimeoutMs"),
            lifecycle: f.object("lifecycle"),
            transport: f.keyword("transport", "无效的传输类别", parse_transport),
            call_dedup: f.object("callDedup"),
        };
        f.finish(c)
    }
}

impl JsConfig {
    pub fn into_core(self) -> ClientConfig {
        let mut c = ClientConfig::new(
            self.app_id,
            self.app_name,
            self.instance_id,
            self.client_kind.unwrap_or(ClientKind::Web),
        );
        if let Some(v) = self.sdk_version {
            c.sdk_version = v;
        }
        c.app_version = self.app_version;
        c.origin = self.origin;
        c.instance_title = self.instance_title;
        c.instance_url = self.instance_url;
        c.token = self.token;
        c.launch_token = self.launch_token;
        if let Some(r) = self.reconnect {
            let d = ReconnectPolicy::default();
            c.reconnect = ReconnectPolicy {
                initial_delay_ms: r.initial_delay_ms.unwrap_or(d.initial_delay_ms),
                max_delay_ms: r.max_delay_ms.unwrap_or(d.max_delay_ms),
                multiplier: r.multiplier.unwrap_or(d.multiplier),
            };
        }
        if let Some(h) = self.heartbeat {
            let d = HeartbeatPolicy::default();
            c.heartbeat = HeartbeatPolicy {
                mode: h.mode.unwrap_or(d.mode),
                interval_ms: h.interval_ms.unwrap_or(d.interval_ms),
                timeout_ms: h.timeout_ms.unwrap_or(d.timeout_ms),
                hidden_timeout_ms: h.hidden_timeout_ms.unwrap_or(d.hidden_timeout_ms),
            };
        }
        if let Some(n) = self.max_concurrent_calls {
            c.max_concurrent_calls = n.max(1);
        }
        if let Some(ms) = self.resource_update_throttle_ms {
            c.resource_update_throttle_ms = ms;
        }
        c.overview = self.overview;
        if let Some(ms) = self.handshake_timeout_ms {
            c.handshake_timeout_ms = ms;
        }
        if let Some(l) = self.lifecycle {
            c.lifecycle = l.into_core();
        }
        if let Some(t) = self.transport {
            c.transport = t;
        }
        if let Some(d) = self.call_dedup {
            c.call_dedup = d.into_core();
        }
        c
    }
}

// ---------------------------------------------------------------------------
// 定义
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct JsToolDef {
    pub name: String,
    /// 缺省为空字符串。
    pub description: String,
    pub input_schema: Value,
    pub risk: Option<Risk>,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    pub annotations: Option<ToolAnnotations>,
    pub output_schema: Option<Value>,
    /// 缺省 true。
    pub enabled: Option<bool>,
    pub scope: Option<f64>,
}

impl FromJson for JsToolDef {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let name = f.required_string("name");
        let description = f.string("description").unwrap_or_default();
        let input_schema = f.value("inputSchema").unwrap_or_else(|| {
            f.fail("缺少字段 inputSchema".to_owned());
            Value::Null
        });
        let d = JsToolDef {
            name,
            description,
            input_schema,
            risk: f.protocol("risk"),
            activation: f.protocol("activation"),
            title: f.string("title"),
            annotations: f.object("annotations"),
            output_schema: f.value("outputSchema"),
            enabled: f.bool("enabled"),
            scope: f.f64("scope"),
        };
        f.finish(d)
    }
}

impl JsToolDef {
    pub fn into_core(self) -> Result<ToolDef, String> {
        Ok(ToolDef {
            name: self.name,
            description: self.description,
            input_schema: self.input_schema,
            risk: self.risk.unwrap_or_default(),
            activation: self.activation,
            title: self.title,
            enabled: self.enabled.unwrap_or(true),
            scope: scope_handle(self.scope)?,
            annotations: self.annotations,
            output_schema: self.output_schema,
        })
    }
}

/// 标准 MCP 工具注解（spec/protocol.md 第 3 节）。
///
/// @why 用 [`Fields`] 逐字段读取而不是 serde 派生：派生的反序列化代码让 WASM gzip 增加约 1.7 KB。
impl FromJson for ToolAnnotations {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let a = ToolAnnotations {
            title: f.string("title"),
            read_only_hint: f.bool("readOnlyHint"),
            destructive_hint: f.bool("destructiveHint"),
            idempotent_hint: f.bool("idempotentHint"),
            open_world_hint: f.bool("openWorldHint"),
        };
        f.finish(a)
    }
}

fn parse_result_status(s: &str) -> Option<ResultStatus> {
    Some(match s {
        "done" => ResultStatus::Done,
        "pending" => ResultStatus::Pending,
        "partial" => ResultStatus::Partial,
        "noop" => ResultStatus::Noop,
        _ => return None,
    })
}

/// 标准 MCP 内容注解（结果的 `annotations`）。
impl FromJson for ContentAnnotations {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let audience = f.strings("audience").map(|roles| {
            roles
                .iter()
                .filter_map(|r| match r.as_str() {
                    "user" => Some(Audience::User),
                    "assistant" => Some(Audience::Assistant),
                    other => {
                        f.fail(format!("字段 audience 的取值应为 user / assistant：\"{other}\""));
                        None
                    }
                })
                .collect()
        });
        let a = ContentAnnotations {
            audience,
            priority: f.f64("priority"),
            last_modified: f.string("lastModified"),
        };
        f.finish(a)
    }
}

/// 部分更新：缺省字段不变；`activation` / `title` 区分缺省（外层 `None`）与 `null`（`Some(None)`，表示清除）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsToolUpdate {
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub risk: Option<Risk>,
    pub activation: Option<Option<Activation>>,
    pub title: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// `null` 清除声明的注解。
    pub annotations: Option<Option<ToolAnnotations>>,
    /// `null` 清除声明的输出 schema。
    pub output_schema: Option<Option<Value>>,
}

impl FromJson for JsToolUpdate {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let activation = match f.nullable("activation") {
            None => None,
            Some(None) => Some(None),
            Some(Some(v)) => f.protocol_value("activation", v).map(Some),
        };
        let title = match f.nullable("title") {
            None => None,
            Some(None) => Some(None),
            Some(Some(Value::String(s))) => Some(Some(s)),
            Some(Some(_)) => {
                f.fail("字段 title 应为字符串".to_owned());
                None
            }
        };
        let annotations = match f.nullable("annotations") {
            None => None,
            Some(None) => Some(None),
            Some(Some(v)) => match ToolAnnotations::from_json(v) {
                Ok(a) => Some(Some(a)),
                Err(e) => {
                    f.fail(format!("annotations.{e}"));
                    None
                }
            },
        };
        let u = JsToolUpdate {
            annotations,
            output_schema: f.nullable("outputSchema"),
            description: f.string("description"),
            input_schema: f.value("inputSchema"),
            risk: f.protocol("risk"),
            activation,
            title,
            enabled: f.bool("enabled"),
        };
        f.finish(u)
    }
}

impl JsToolUpdate {
    pub fn into_core(self) -> ToolUpdate {
        ToolUpdate {
            description: self.description,
            input_schema: self.input_schema,
            risk: self.risk,
            activation: self.activation,
            title: self.title,
            enabled: self.enabled,
            annotations: self.annotations,
            output_schema: self.output_schema,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JsResourceDef {
    pub name: String,
    /// 缺省为空字符串。
    pub description: String,
    pub mime_type: Option<String>,
    pub scope: Option<f64>,
    /// 缺省 `false`（spec/lifecycle.md 第 13 节 B3）。
    pub realtime: bool,
}

impl FromJson for JsResourceDef {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let d = JsResourceDef {
            name: f.required_string("name"),
            description: f.string("description").unwrap_or_default(),
            mime_type: f.string("mimeType"),
            scope: f.f64("scope"),
            realtime: f.bool("realtime").unwrap_or(false),
        };
        f.finish(d)
    }
}

impl JsResourceDef {
    pub fn into_core(self) -> Result<ResourceDef, String> {
        Ok(ResourceDef {
            name: self.name,
            description: self.description,
            mime_type: self.mime_type,
            scope: scope_handle(self.scope)?,
            realtime: self.realtime,
        })
    }
}

// ---------------------------------------------------------------------------
// 结果
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct JsToolError {
    pub kind: ErrorKind,
    /// 缺省为空字符串。
    pub message: String,
    pub details: Option<Value>,
}

impl FromJson for JsToolError {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let kind = f.protocol("kind");
        let message = f.string("message").unwrap_or_default();
        let details = f.value("details");
        match kind {
            Some(kind) => f.finish(JsToolError { kind, message, details }),
            None => {
                f.fail("缺少字段 kind".to_owned());
                Err(f.error.unwrap_or_default())
            }
        }
    }
}

/// handler / 资源读取的结果：`{ data, stateHints?, annotations?, status?, stateResource?, summary? }` 或
/// `{ error: { kind, message, details? } }`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsCallOutcome {
    pub data: Option<Value>,
    pub state_hints: Option<Vec<String>>,
    pub annotations: Option<ContentAnnotations>,
    pub status: Option<ResultStatus>,
    pub state_resource: Option<String>,
    pub summary: Option<String>,
    pub error: Option<JsToolError>,
}

impl FromJson for JsCallOutcome {
    fn from_json(value: Value) -> Result<Self, String> {
        let mut f = Fields::new(value)?;
        let o = JsCallOutcome {
            data: f.value("data"),
            state_hints: f.strings("stateHints"),
            annotations: f.object("annotations"),
            status: f.keyword("status", "无效的 status", parse_result_status),
            state_resource: f.string("stateResource"),
            summary: f.string("summary"),
            error: f.object("error"),
        };
        f.finish(o)
    }
}

impl JsCallOutcome {
    fn error_into_core(e: JsToolError) -> ToolError {
        let err = ToolError::new(e.kind, e.message);
        match e.details {
            Some(d) => err.with_details(d),
            None => err,
        }
    }

    pub fn into_call(self) -> Result<CallOutput, ToolError> {
        match self.error {
            Some(e) => Err(Self::error_into_core(e)),
            None => Ok(CallOutput {
                data: self.data.unwrap_or(Value::Null),
                state_hints: self.state_hints.unwrap_or_default(),
                annotations: self.annotations,
                state_resource: self.state_resource,
                status: self.status.unwrap_or_default(),
                summary: self.summary,
            }),
        }
    }

    pub fn into_read(self) -> Result<Value, ToolError> {
        match self.error {
            Some(e) => Err(Self::error_into_core(e)),
            None => Ok(self.data.unwrap_or(Value::Null)),
        }
    }
}

// ---------------------------------------------------------------------------
// 状态与事件
// ---------------------------------------------------------------------------

/// 连接状态，形状与 `@app-mcp/web` 的 `ConnectionState` 一致（`retryAt` 为核心时钟），见 [`JsState::to_value`]。
#[derive(Clone, Debug, PartialEq)]
pub struct JsState(ConnectionState);

/// 由 `(键, 值)` 构造 JSON 对象。
///
/// @why 手工构造输出对象而不用 `derive(Serialize)`：省去每个类型一套序列化代码（WASM 体积）。
/// 逐个插入而不是 `collect`：`collect` 会引入 `BTreeMap` 的批量构建与稳定排序代码。
fn object(fields: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (k, v) in fields {
        map.insert(k.to_owned(), v);
    }
    Value::Object(map)
}


impl JsState {
    pub fn from_core(state: &ConnectionState) -> Self {
        Self(state.clone())
    }

    /// `{ status: <kebab-case>, retryAt?, reason?, code? }`（状态名取自 [`ConnectionState::name`]）。
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("status".to_owned(), self.0.name().into());
        if let ConnectionState::Backoff { retry_at, .. } = &self.0 {
            map.insert("retryAt".to_owned(), (*retry_at).into());
        }
        if let Some(reason) = self.0.reason() {
            map.insert("reason".to_owned(), reason.into());
        }
        if let Some(code) = self.0.code() {
            map.insert("code".to_owned(), code.as_str().into());
        }
        Value::Object(map)
    }
}

fn cancel_reason(r: CancelReason) -> &'static str {
    match r {
        CancelReason::Requested => "requested",
        CancelReason::Timeout => "timeout",
        CancelReason::Disconnected => "disconnected",
        CancelReason::Stopped => "stopped",
    }
}

/// `pollEvent()` 返回的事件，见 [`JsEvent::to_value`]（`type` 为 camelCase 的事件名，字段 camelCase）。
#[derive(Clone, Debug, PartialEq)]
pub enum JsEvent {
    Connect,
    Disconnect,
    Send { text: String },
    InvokeTool { call_id: String, tool: u64, name: String, arguments: Value },
    CancelTool { call_id: String, reason: &'static str },
    ReadResource { read: u64, resource: u64, name: String },
    StateChanged { state: JsState },
    Paired { token: String },
    Warning { message: String },
    IdleExit,
}

impl JsEvent {
    pub fn from_core(ev: Event) -> Self {
        match ev {
            Event::Connect => JsEvent::Connect,
            Event::Disconnect => JsEvent::Disconnect,
            Event::Send(text) => JsEvent::Send { text },
            Event::InvokeTool { call_id, tool, name, arguments } => {
                JsEvent::InvokeTool { call_id, tool: tool.0, name, arguments }
            }
            Event::CancelTool { call_id, reason } => JsEvent::CancelTool { call_id, reason: cancel_reason(reason) },
            Event::ReadResource { read, resource, name } => {
                JsEvent::ReadResource { read: read.0, resource: resource.0, name }
            }
            Event::StateChanged(state) => JsEvent::StateChanged { state: JsState::from_core(&state) },
            Event::Paired { token } => JsEvent::Paired { token },
            Event::Warning(message) => JsEvent::Warning { message },
            Event::IdleExit => JsEvent::IdleExit,
        }
    }

    /// `{ type: <camelCase>, ... }`。
    pub fn to_value(self) -> Value {
        match self {
            JsEvent::Connect => object(vec![("type", "connect".into())]),
            JsEvent::Disconnect => object(vec![("type", "disconnect".into())]),
            JsEvent::Send { text } => object(vec![("type", "send".into()), ("text", text.into())]),
            JsEvent::InvokeTool { call_id, tool, name, arguments } => object(vec![
                ("type", "invokeTool".into()),
                ("callId", call_id.into()),
                ("tool", tool.into()),
                ("name", name.into()),
                ("arguments", arguments),
            ]),
            JsEvent::CancelTool { call_id, reason } => {
                object(vec![("type", "cancelTool".into()), ("callId", call_id.into()), ("reason", reason.into())])
            }
            JsEvent::ReadResource { read, resource, name } => object(vec![
                ("type", "readResource".into()),
                ("read", read.into()),
                ("resource", resource.into()),
                ("name", name.into()),
            ]),
            JsEvent::StateChanged { state } => object(vec![("type", "stateChanged".into()), ("state", state.to_value())]),
            JsEvent::Paired { token } => object(vec![("type", "paired".into()), ("token", token.into())]),
            JsEvent::Warning { message } => object(vec![("type", "warning".into()), ("message", message.into())]),
            JsEvent::IdleExit => object(vec![("type", "idleExit".into())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_core::{ConnectionErrorCode, ReadId, ResourceId, ToolId};
    use serde_json::json;

    #[test]
    fn config_defaults_and_overrides() {
        let c: JsConfig = JsConfig::from_json(json!({
            "appId": "shop", "appName": "商城", "instanceId": "i1",
            "sdkVersion": "9.9.9", "token": "t", "maxConcurrentCalls": 0,
            "heartbeat": { "timeoutMs": 5 }
        }))
        .unwrap();
        let c = c.into_core();
        assert_eq!(c.client_kind, ClientKind::Web);
        assert_eq!(c.sdk_version, "9.9.9");
        assert_eq!(c.token.as_deref(), Some("t"));
        assert_eq!(c.max_concurrent_calls, 1);
        assert_eq!(c.heartbeat.timeout_ms, 5);
        assert_eq!(c.heartbeat.interval_ms, HeartbeatPolicy::default().interval_ms);
        assert_eq!(c.reconnect, ReconnectPolicy::default());
        assert_eq!(c.resource_update_throttle_ms, 100);
        assert_eq!(c.overview, None);
        assert_eq!(c.call_dedup, CallDedupPolicy::default());
    }

    #[test]
    fn config_call_dedup() {
        let base = || json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" });
        let with = |d: Value| {
            let mut v = base();
            v["callDedup"] = d;
            JsConfig::from_json(v).map(JsConfig::into_core)
        };
        assert_eq!(with(json!({ "ttlMs": 0 })).unwrap().call_dedup, CallDedupPolicy { ttl_ms: 0, ..CallDedupPolicy::default() });
        assert!(!with(json!({ "maxEntries": 0 })).unwrap().call_dedup.enabled());
        assert_eq!(with(json!({ "ttlMs": 10, "maxEntries": 3 })).unwrap().call_dedup, CallDedupPolicy { ttl_ms: 10, max_entries: 3 });
        assert!(with(json!({ "ttlMs": -1 })).is_err());
        assert_eq!(with(json!({ "bogus": 1 })).unwrap().call_dedup, CallDedupPolicy::default(), "未知字段忽略");
    }

    #[test]
    fn config_power_fields() {
        let c = JsConfig::from_json(json!({
            "appId": "shop", "appName": "商城", "instanceId": "i1", "transport": "loopback",
            "heartbeat": { "mode": "always" },
            "lifecycle": { "mode": "idle", "hostAbsentRetries": 5, "legacyTimers": true,
                           "mergeWindowMs": 500, "sleepOnBackground": true }
        }))
        .unwrap()
        .into_core();
        assert_eq!(c.transport, TransportKind::Loopback);
        assert_eq!(c.heartbeat.mode, HeartbeatMode::Always);
        assert_eq!((c.lifecycle.host_absent_retries, c.lifecycle.legacy_timers), (5, true));
        assert_eq!((c.lifecycle.merge_window_ms, c.lifecycle.sleep_on_background), (500, true));
        let d = JsConfig::from_json(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" })).unwrap().into_core();
        assert_eq!(d.transport, TransportKind::Unknown);
        assert_eq!((d.lifecycle.host_absent_retries, d.lifecycle.legacy_timers), (3, false));
        assert_eq!((d.lifecycle.merge_window_ms, d.lifecycle.sleep_on_background), (2_000, false));
        let r = JsResourceDef::from_json(json!({ "name": "order", "realtime": true })).unwrap().into_core().unwrap();
        assert!(r.realtime);
        let r = JsResourceDef::from_json(json!({ "name": "cart" })).unwrap().into_core().unwrap();
        assert!(!r.realtime);
        let bad = JsConfig::from_json(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1", "transport": "x" }));
        assert!(bad.is_err_and(|e| e.contains("无效的传输类别")));
    }

    #[test]
    fn config_overview() {
        let c: JsConfig = JsConfig::from_json(json!({
            "appId": "shop", "appName": "商城", "instanceId": "i1",
            "overview": { "summary": "演示商城", "body": "## 能力范围", "locale": "zh-CN" }
        }))
        .unwrap();
        let o = c.into_core().overview.unwrap();
        assert_eq!(o.summary, "演示商城");
        assert_eq!(o.body.as_deref(), Some("## 能力范围"));
        assert_eq!(o.locale.as_deref(), Some("zh-CN"));
    }

    #[test]
    fn tool_def_defaults() {
        let d: JsToolDef = JsToolDef::from_json(json!({
            "name": "cart.add", "description": "加入购物车",
            "inputSchema": { "type": "object" }, "risk": "os-sensitive", "scope": 3
        }))
        .unwrap();
        let d = d.into_core().unwrap();
        assert_eq!(d.risk, Risk::OsSensitive);
        assert!(d.enabled);
        assert_eq!(d.scope, Some(ScopeId(3)));
        assert_eq!(d.activation, None);

        let bad: JsToolDef =
            JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "scope": 1.5 })).unwrap();
        assert!(bad.into_core().is_err());
    }

    #[test]
    fn tool_update_distinguishes_missing_and_null() {
        let u: JsToolUpdate = JsToolUpdate::from_json(json!({ "title": null, "enabled": false })).unwrap();
        let u = u.into_core();
        assert_eq!(u.title, Some(None));
        assert_eq!(u.activation, None);
        assert_eq!(u.enabled, Some(false));
        assert_eq!(u.description, None);
    }

    #[test]
    fn outcome_conversion() {
        let ok: JsCallOutcome = JsCallOutcome::from_json(json!({ "data": { "a": 1 }, "stateHints": ["cart"] })).unwrap();
        let ok = ok.into_call().unwrap();
        assert_eq!(ok.data, json!({ "a": 1 }));
        assert_eq!(ok.state_hints, vec!["cart".to_owned()]);

        let empty: JsCallOutcome = JsCallOutcome::from_json(json!({})).unwrap();
        assert_eq!(empty.into_read().unwrap(), Value::Null);

        let err: JsCallOutcome = JsCallOutcome::from_json(json!({
            "error": { "kind": "INVALID_INPUT", "message": "bad", "details": { "path": "a" } }
        }))
        .unwrap();
        let err = err.into_call().unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert_eq!(err.message, "bad");
        assert_eq!(err.details, Some(json!({ "path": "a" })));

        let bad_kind = JsCallOutcome::from_json(json!({ "error": { "kind": "NOPE" } }));
        assert!(bad_kind.is_err());

        // USER_ACTION_REQUIRED（@app-mcp/web 的 ToolCallError.userActionRequired）：详情原样进入核心错误
        let action = JsCallOutcome::from_json(json!({
            "error": { "kind": "USER_ACTION_REQUIRED", "message": "请先登录", "details": { "reason": "login", "uri": "shop://login" } }
        }))
        .unwrap()
        .into_call()
        .unwrap_err();
        assert_eq!(action, ToolError::user_action_required("请先登录", Some("login"), Some("shop://login")));
        let bare = JsCallOutcome::from_json(json!({ "error": { "kind": "USER_ACTION_REQUIRED", "message": "切到前台" } }))
            .unwrap()
            .into_call()
            .unwrap_err();
        assert_eq!(bare, ToolError::user_action_required("切到前台", None, None));
        let denied = JsCallOutcome::from_json(json!({ "error": { "kind": "POLICY_DENIED" } })).unwrap();
        assert_eq!(denied.into_call().unwrap_err().kind, ErrorKind::PolicyDenied);
    }

    /// 第 14 项 S1 / 第 19 项 R2：工具注解与输出 schema；更新时 `null` 清除。
    #[test]
    fn tool_annotations_and_output_schema() {
        let d = JsToolDef::from_json(json!({
            "name": "order.cancel", "inputSchema": { "type": "object" },
            "annotations": { "destructiveHint": true, "idempotentHint": true, "title": "取消" },
            "outputSchema": { "type": "array" }
        }))
        .unwrap()
        .into_core()
        .unwrap();
        let a = d.annotations.unwrap();
        assert_eq!((a.destructive_hint, a.idempotent_hint, a.read_only_hint), (Some(true), Some(true), None));
        assert_eq!(a.title.as_deref(), Some("取消"));
        assert_eq!(d.output_schema, Some(json!({ "type": "array" })));
        let e = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "annotations": { "readOnlyHint": 1 } }))
            .unwrap_err();
        assert_eq!(e, "annotations.字段 readOnlyHint 应为布尔值");
        let u = JsToolUpdate::from_json(json!({ "annotations": null, "outputSchema": null })).unwrap().into_core();
        assert_eq!((u.annotations, u.output_schema), (Some(None), Some(None)));
        let u = JsToolUpdate::from_json(json!({ "annotations": { "openWorldHint": false } })).unwrap().into_core();
        assert_eq!(u.annotations.unwrap().unwrap().open_world_hint, Some(false));
        assert_eq!(u.output_schema, None);
        assert!(JsToolUpdate::from_json(json!({ "annotations": { "title": 1 } })).is_err());
    }

    /// 第 19 项 R1 / R3 与第 14 项 S2：结果状态、状态资源、摘要、内容标注。
    #[test]
    fn outcome_status_summary_annotations() {
        let o = JsCallOutcome::from_json(json!({
            "data": null, "status": "pending", "stateResource": "order.state", "summary": "已提交",
            "annotations": { "audience": ["user", "assistant"], "priority": 0.5, "lastModified": "2026-10-02T00:00:00Z" }
        }))
        .unwrap()
        .into_call()
        .unwrap();
        assert_eq!(o.status, ResultStatus::Pending);
        assert_eq!((o.state_resource.as_deref(), o.summary.as_deref()), (Some("order.state"), Some("已提交")));
        let a = o.annotations.unwrap();
        assert_eq!(a.audience, Some(vec![Audience::User, Audience::Assistant]));
        assert_eq!((a.priority, a.last_modified.as_deref()), (Some(0.5), Some("2026-10-02T00:00:00Z")));
        for (s, st) in [("done", ResultStatus::Done), ("partial", ResultStatus::Partial), ("noop", ResultStatus::Noop)] {
            let o = JsCallOutcome::from_json(json!({ "status": s })).unwrap().into_call().unwrap();
            assert_eq!(o.status, st);
        }
        assert_eq!(JsCallOutcome::from_json(json!({ "status": "maybe" })).unwrap_err(), "无效的 status：\"maybe\"");
        assert!(JsCallOutcome::from_json(json!({ "annotations": { "audience": ["robot"] } })).is_err());
        let plain = JsCallOutcome::from_json(json!({ "data": 1 })).unwrap().into_call().unwrap();
        assert_eq!((plain.status, plain.summary, plain.annotations), (ResultStatus::Done, None, None));
    }

    #[test]
    fn state_shape() {
        let cases = [
            (ConnectionState::Idle, json!({ "status": "idle" })),
            (ConnectionState::Connecting, json!({ "status": "connecting" })),
            (ConnectionState::Handshaking, json!({ "status": "handshaking" })),
            (ConnectionState::PendingPairing, json!({ "status": "pending-pairing" })),
            (ConnectionState::Connected, json!({ "status": "connected" })),
            (
                ConnectionState::Backoff { retry_at: 42, reason: None, code: None },
                json!({ "status": "backoff", "retryAt": 42 }),
            ),
            (
                ConnectionState::Backoff {
                    retry_at: 42,
                    reason: Some("无法连接".into()),
                    code: Some(ConnectionErrorCode::ConnectFailed),
                },
                json!({ "status": "backoff", "retryAt": 42, "reason": "无法连接", "code": "CONNECT_FAILED" }),
            ),
            (
                ConnectionState::Rejected { reason: "r".into(), code: ConnectionErrorCode::OriginNotAllowed },
                json!({ "status": "rejected", "reason": "r", "code": "ORIGIN_NOT_ALLOWED" }),
            ),
            (ConnectionState::Stopped, json!({ "status": "stopped" })),
            (ConnectionState::Dormant, json!({ "status": "dormant" })),
            (ConnectionState::Waking, json!({ "status": "waking" })),
            (
                ConnectionState::HostMismatch { reason: "m".into(), code: ConnectionErrorCode::HostNotAppMcp },
                json!({ "status": "host-mismatch", "reason": "m", "code": "HOST_NOT_APP_MCP" }),
            ),
        ];
        for (state, expected) in cases {
            assert_eq!(JsState::from_core(&state).to_value(), expected);
        }
    }

    #[test]
    fn event_shape() {
        let cases = [
            (Event::Connect, json!({ "type": "connect" })),
            (Event::Disconnect, json!({ "type": "disconnect" })),
            (Event::Send("x".into()), json!({ "type": "send", "text": "x" })),
            (
                Event::InvokeTool { call_id: "c1".into(), tool: ToolId(7), name: "t".into(), arguments: json!({ "a": 1 }) },
                json!({ "type": "invokeTool", "callId": "c1", "tool": 7, "name": "t", "arguments": { "a": 1 } }),
            ),
            (
                Event::CancelTool { call_id: "c1".into(), reason: CancelReason::Timeout },
                json!({ "type": "cancelTool", "callId": "c1", "reason": "timeout" }),
            ),
            (
                Event::ReadResource { read: ReadId(2), resource: ResourceId(3), name: "r".into() },
                json!({ "type": "readResource", "read": 2, "resource": 3, "name": "r" }),
            ),
            (
                Event::StateChanged(ConnectionState::Connected),
                json!({ "type": "stateChanged", "state": { "status": "connected" } }),
            ),
            (Event::Paired { token: "tk".into() }, json!({ "type": "paired", "token": "tk" })),
            (Event::Warning("w".into()), json!({ "type": "warning", "message": "w" })),
            (Event::IdleExit, json!({ "type": "idleExit" })),
            (
                Event::StateChanged(ConnectionState::Dormant),
                json!({ "type": "stateChanged", "state": { "status": "dormant" } }),
            ),
            (
                Event::StateChanged(ConnectionState::Waking),
                json!({ "type": "stateChanged", "state": { "status": "waking" } }),
            ),
        ];
        for (ev, expected) in cases {
            assert_eq!(JsEvent::from_core(ev).to_value(), expected);
        }
    }

    #[test]
    fn config_lifecycle() {
        let c: JsConfig = JsConfig::from_json(json!({
            "appId": "shop", "appName": "商城", "instanceId": "i1", "handshakeTimeoutMs": 0,
            "lifecycle": {
                "mode": "on-demand", "graceMs": 3000, "residency": "exit-when-idle",
                "wake": { "kind": "web-url", "target": "http://localhost:5173/" }
            }
        }))
        .unwrap();
        let c = c.into_core();
        assert_eq!(c.handshake_timeout_ms, 0);
        let l = c.lifecycle;
        assert_eq!(l.mode, LifecycleMode::OnDemand);
        assert_eq!(l.grace_ms, 3000);
        assert_eq!(l.idle_timeout_ms, LifecyclePolicy::default().idle_timeout_ms);
        assert_eq!(l.residency, Residency::ExitWhenIdle);
        let w = l.wake.unwrap();
        assert_eq!(w.kind, app_mcp_core::WakeKind::WebUrl);
        assert!(!w.background);

        let c: JsConfig = JsConfig::from_json(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" })).unwrap();
        assert_eq!(c.into_core().lifecycle, LifecyclePolicy::default());
        let bad = JsConfig::from_json(json!({
            "appId": "shop", "appName": "商城", "instanceId": "i1", "lifecycle": { "mode": "sometimes" }
        }));
        assert!(bad.is_err());
    }

    #[test]
    fn reason_parse() {
        assert_eq!(parse_wake_reason("visible").unwrap(), WakeReason::Visible);
        assert!(parse_wake_reason("x").is_err());
        assert_eq!(parse_sleep_reason("background").unwrap(), SleepReason::Background);
        assert!(parse_sleep_reason("x").is_err());
    }

    #[test]
    fn visibility_parse() {
        assert_eq!(parse_visibility("frozen").unwrap(), Visibility::Frozen);
        assert!(parse_visibility("gone").is_err());
    }

    #[test]
    fn json_reader_errors_and_null_handling() {
        let err = |v: Value| JsConfig::from_json(v).unwrap_err();
        assert_eq!(err(json!([])), "应为对象");
        assert_eq!(err(json!({ "appName": "a", "instanceId": "i" })), "缺少字段 appId");
        assert_eq!(err(json!({ "appId": 1, "appName": "a", "instanceId": "i" })), "字段 appId 应为字符串");
        let base = json!({ "appId": "a", "appName": "a", "instanceId": "i" });
        let with = |k: &str, v: Value| {
            let mut o = base.clone();
            o[k] = v;
            o
        };
        assert_eq!(err(with("maxConcurrentCalls", json!(-1))), "字段 maxConcurrentCalls 应为非负整数");
        assert_eq!(err(with("heartbeat", json!({ "timeoutMs": "x" }))), "heartbeat.字段 timeoutMs 应为非负整数");
        assert_eq!(err(with("reconnect", json!({ "multiplier": true }))), "reconnect.字段 multiplier 应为数字");
        assert!(err(with("clientKind", json!("tv"))).starts_with("字段 clientKind："));
        assert_eq!(err(with("lifecycle", json!({ "residency": "x" }))), "lifecycle.无效的驻留策略：\"x\"");
        // 多处出错时只报读取顺序上的第一条
        assert_eq!(
            err(json!({ "appId": 1, "appName": 2, "instanceId": "i", "heartbeat": { "timeoutMs": "x" } })),
            "字段 appId 应为字符串"
        );
        assert_eq!(err(with("lifecycle", json!({ "mode": 1, "residency": "x" }))), "lifecycle.字段 mode 应为字符串");
        // null 等同缺省；未知字段忽略
        let c = JsConfig::from_json(with("token", Value::Null)).unwrap();
        assert_eq!(c.token, None);
        assert!(JsConfig::from_json(with("unknownField", json!(1))).is_ok());

        assert_eq!(JsToolDef::from_json(json!({ "name": "t" })).unwrap_err(), "缺少字段 inputSchema");
        assert_eq!(
            JsToolDef::from_json(json!({ "name": "t", "inputSchema": {}, "enabled": 1 })).unwrap_err(),
            "字段 enabled 应为布尔值"
        );
        assert_eq!(JsResourceDef::from_json(json!({ "name": 3 })).unwrap_err(), "字段 name 应为字符串");
        assert_eq!(
            JsToolUpdate::from_json(json!({ "title": 1 })).unwrap_err(),
            "字段 title 应为字符串"
        );
        let u = JsToolUpdate::from_json(json!({ "activation": null, "title": "x", "risk": "write" })).unwrap();
        assert_eq!(u.activation, Some(None));
        assert_eq!(u.title, Some(Some("x".into())));
        assert_eq!(u.risk, Some(Risk::Write));
        assert_eq!(
            JsCallOutcome::from_json(json!({ "stateHints": ["a", 1] })).unwrap_err(),
            "字段 stateHints 应为字符串数组"
        );
        assert_eq!(JsCallOutcome::from_json(json!({ "error": {} })).unwrap_err(), "error.缺少字段 kind");
    }

    #[test]
    fn handle_validation() {
        assert_eq!(handle(7.0, "句柄"), Ok(7));
        assert_eq!(handle(1.5, "句柄").unwrap_err(), "无效的句柄：1.5");
        assert_eq!(handle(-1.0, "句柄").unwrap_err(), "无效的句柄：-1.0");
        assert_eq!(handle(f64::NAN, "句柄").unwrap_err(), "无效的句柄：NaN");
        assert_eq!(handle(f64::NEG_INFINITY, "句柄").unwrap_err(), "无效的句柄：-Infinity");
        assert!(handle(MAX_SAFE_INTEGER as f64 + 2.0, "句柄").is_err());
    }
}
