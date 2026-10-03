//! 配置对象（`new WasmClient(config)`）：重连、心跳、调用去重、生命周期。

use super::*;

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
