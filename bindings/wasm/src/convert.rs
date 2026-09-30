//! JS 对象 ↔ 核心类型的转换。与 wasm-bindgen 无关，可在原生目标上测试。

use app_mcp_core::{
    Activation, AppOverview, CallOutput, CancelReason, ClientConfig, ClientKind, ConnectionState, Event, HeartbeatPolicy,
    LifecycleMode, LifecyclePolicy, ReconnectPolicy, Residency, ResourceDef, Risk, ScopeId, SleepReason, ToolDef,
    ToolError, ToolUpdate, Visibility, WakeDescriptor, WakeReason,
};
use app_mcp_protocol::ErrorKind;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// JS 能精确表示的最大整数（2^53 - 1）。
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// 区分"字段缺省"（外层 `None`）与"字段为 null"（`Some(None)`）。
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

fn scope_handle(scope: Option<f64>) -> Result<Option<ScopeId>, String> {
    match scope {
        None => Ok(None),
        Some(s) if s.is_finite() && s >= 0.0 && s.fract() == 0.0 && s <= MAX_SAFE_INTEGER as f64 => {
            Ok(Some(ScopeId(s as u64)))
        }
        Some(s) => Err(format!("无效的 scope 句柄：{s}")),
    }
}

pub fn parse_wake_reason(s: &str) -> Result<WakeReason, String> {
    match s {
        "os-activation" => Ok(WakeReason::OsActivation),
        "app" => Ok(WakeReason::App),
        "visible" => Ok(WakeReason::Visible),
        "cold-start" => Ok(WakeReason::ColdStart),
        other => Err(format!("无效的唤醒原因：{other:?}")),
    }
}

pub fn parse_sleep_reason(s: &str) -> Result<SleepReason, String> {
    match s {
        "idle" => Ok(SleepReason::Idle),
        "grace" => Ok(SleepReason::Grace),
        "background" => Ok(SleepReason::Background),
        "app" => Ok(SleepReason::App),
        other => Err(format!("无效的休眠原因：{other:?}")),
    }
}

pub fn parse_visibility(s: &str) -> Result<Visibility, String> {
    match s {
        "visible" => Ok(Visibility::Visible),
        "hidden" => Ok(Visibility::Hidden),
        "frozen" => Ok(Visibility::Frozen),
        other => Err(format!("无效的可见性：{other:?}")),
    }
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsReconnect {
    pub initial_delay_ms: Option<u64>,
    pub max_delay_ms: Option<u64>,
    pub multiplier: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsHeartbeat {
    pub interval_ms: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub hidden_timeout_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JsLifecycleMode {
    Persistent,
    Idle,
    OnDemand,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JsResidency {
    Keep,
    ExitWhenIdle,
    ExitAlways,
}

/// 生命周期策略（spec/lifecycle.md 第 3 节）。未提供的字段取核心默认值。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsLifecycle {
    pub mode: Option<JsLifecycleMode>,
    pub idle_timeout_ms: Option<u64>,
    pub hidden_idle_timeout_ms: Option<u64>,
    pub grace_ms: Option<u64>,
    pub residency: Option<JsResidency>,
    /// 形如 `{ kind: 'web-url', target: location.href, background: false }`。
    pub wake: Option<WakeDescriptor>,
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
        }
    }
}

/// `new WasmClient(config)` 的配置对象。未提供的可选字段取核心默认值。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
        c
    }
}

// ---------------------------------------------------------------------------
// 定义
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub input_schema: Value,
    pub risk: Option<Risk>,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    /// 缺省 true。
    pub enabled: Option<bool>,
    pub scope: Option<f64>,
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
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsToolUpdate {
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub risk: Option<Risk>,
    #[serde(default, deserialize_with = "double_option")]
    pub activation: Option<Option<Activation>>,
    #[serde(default, deserialize_with = "double_option")]
    pub title: Option<Option<String>>,
    pub enabled: Option<bool>,
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
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsResourceDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub mime_type: Option<String>,
    pub scope: Option<f64>,
}

impl JsResourceDef {
    pub fn into_core(self) -> Result<ResourceDef, String> {
        Ok(ResourceDef {
            name: self.name,
            description: self.description,
            mime_type: self.mime_type,
            scope: scope_handle(self.scope)?,
        })
    }
}

// ---------------------------------------------------------------------------
// 结果
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct JsToolError {
    pub kind: ErrorKind,
    #[serde(default)]
    pub message: String,
    pub details: Option<Value>,
}

/// handler / 资源读取的结果：`{ data, stateHints? }` 或 `{ error: { kind, message, details? } }`。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsCallOutcome {
    pub data: Option<Value>,
    pub state_hints: Option<Vec<String>>,
    pub error: Option<JsToolError>,
}

impl JsCallOutcome {
    fn error_into_core(e: JsToolError) -> ToolError {
        let err = ToolError::new(e.kind, e.message);
        match e.details {
            Some(Value::Null) | None => err,
            Some(d) => err.with_details(d),
        }
    }

    pub fn into_call(self) -> Result<CallOutput, ToolError> {
        match self.error {
            Some(e) => Err(Self::error_into_core(e)),
            None => Ok(CallOutput {
                data: self.data.unwrap_or(Value::Null),
                state_hints: self.state_hints.unwrap_or_default(),
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

/// 连接状态，形状与 `@app-mcp/web` 的 `ConnectionState` 一致（`retryAt` 为核心时钟）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum JsState {
    Idle,
    Connecting,
    Handshaking,
    PendingPairing,
    Connected,
    Backoff {
        #[serde(rename = "retryAt")]
        retry_at: u64,
    },
    Rejected {
        reason: String,
    },
    Stopped,
    Dormant,
    Waking,
}

impl JsState {
    pub fn from_core(state: &ConnectionState) -> Self {
        match state {
            ConnectionState::Idle => JsState::Idle,
            ConnectionState::Connecting => JsState::Connecting,
            ConnectionState::Handshaking => JsState::Handshaking,
            ConnectionState::PendingPairing => JsState::PendingPairing,
            ConnectionState::Connected => JsState::Connected,
            ConnectionState::Backoff { retry_at } => JsState::Backoff { retry_at: *retry_at },
            ConnectionState::Rejected { reason } => JsState::Rejected { reason: reason.clone() },
            ConnectionState::Stopped => JsState::Stopped,
            ConnectionState::Dormant => JsState::Dormant,
            ConnectionState::Waking => JsState::Waking,
        }
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

/// `pollEvent()` 返回的事件对象，`type` 为 camelCase 的事件名。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_core::{ReadId, ResourceId, ToolId};
    use serde_json::json;

    #[test]
    fn config_defaults_and_overrides() {
        let c: JsConfig = serde_json::from_value(json!({
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
    }

    #[test]
    fn config_overview() {
        let c: JsConfig = serde_json::from_value(json!({
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
        let d: JsToolDef = serde_json::from_value(json!({
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
            serde_json::from_value(json!({ "name": "x", "inputSchema": {}, "scope": 1.5 })).unwrap();
        assert!(bad.into_core().is_err());
    }

    #[test]
    fn tool_update_distinguishes_missing_and_null() {
        let u: JsToolUpdate = serde_json::from_value(json!({ "title": null, "enabled": false })).unwrap();
        let u = u.into_core();
        assert_eq!(u.title, Some(None));
        assert_eq!(u.activation, None);
        assert_eq!(u.enabled, Some(false));
        assert_eq!(u.description, None);
    }

    #[test]
    fn outcome_conversion() {
        let ok: JsCallOutcome = serde_json::from_value(json!({ "data": { "a": 1 }, "stateHints": ["cart"] })).unwrap();
        let ok = ok.into_call().unwrap();
        assert_eq!(ok.data, json!({ "a": 1 }));
        assert_eq!(ok.state_hints, vec!["cart".to_owned()]);

        let empty: JsCallOutcome = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty.into_read().unwrap(), Value::Null);

        let err: JsCallOutcome = serde_json::from_value(json!({
            "error": { "kind": "INVALID_INPUT", "message": "bad", "details": { "path": "a" } }
        }))
        .unwrap();
        let err = err.into_call().unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert_eq!(err.message, "bad");
        assert_eq!(err.details, Some(json!({ "path": "a" })));

        let bad_kind = serde_json::from_value::<JsCallOutcome>(json!({ "error": { "kind": "NOPE" } }));
        assert!(bad_kind.is_err());
    }

    #[test]
    fn state_shape() {
        let s = serde_json::to_value(JsState::from_core(&ConnectionState::Backoff { retry_at: 42 })).unwrap();
        assert_eq!(s, json!({ "status": "backoff", "retryAt": 42 }));
        let s = serde_json::to_value(JsState::from_core(&ConnectionState::PendingPairing)).unwrap();
        assert_eq!(s, json!({ "status": "pending-pairing" }));
        let s = serde_json::to_value(JsState::from_core(&ConnectionState::Rejected { reason: "r".into() })).unwrap();
        assert_eq!(s, json!({ "status": "rejected", "reason": "r" }));
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
            assert_eq!(serde_json::to_value(JsEvent::from_core(ev)).unwrap(), expected);
        }
    }

    #[test]
    fn config_lifecycle() {
        let c: JsConfig = serde_json::from_value(json!({
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

        let c: JsConfig = serde_json::from_value(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" })).unwrap();
        assert_eq!(c.into_core().lifecycle, LifecyclePolicy::default());
        let bad = serde_json::from_value::<JsConfig>(json!({
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
}
