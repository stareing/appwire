//! 连接状态机：握手、消息分发、心跳、重连、调用与资源请求处理。
//!
//! 这里是 [`Client`] 的私有实现部分；公开方法在 `lib.rs` 中，只做参数检查与转发。

use crate::vec_map::VecMap;

use app_mcp_protocol as proto;
use proto::{
    ErrorKind, HelloParams, HelloResult, Message, NavigateParams, NavigateResult, Notification, PairingResultParams,
    PairingStatus, Request, RequestId, ResourceSubscribeParams, ResourceUpdatedParams, ResourcesReadParams, Response,
    RpcError, ToolAnnotations, ToolError, ToolsCancelParams, ToolsInvokeParams, ToolsProgressParams, VisibilityParams,
    method,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::calls::Call;
use crate::dedup::Outcome;
use crate::{
    BusyPolicy, CallOutput, CancelReason, Client, ConnectionErrorCode, ConnectionIssue, ConnectionState, Event, LifecycleMode,
    Millis, NavigateId, ReadId, ResourceId, SleepReason, ToolDef, Visibility,
};

/// Client 发出、等待响应的请求。
#[derive(Clone, Copy, Debug)]
pub(crate) enum Outgoing {
    Hello,
    Ping,
    Sleep,
}

/// 心跳状态（仅 `Connected` 期间有效）。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Heartbeat {
    /// 下一次发送 ping 的时刻。已有未响应的 ping 时为 `None`。
    pub next_ping_at: Option<Millis>,
    /// 未响应的 ping：`(请求 ID, 发送时刻)`。
    pub outstanding: Option<(i64, Millis)>,
}

/// 进行中的资源读取。
#[derive(Clone, Debug)]
pub(crate) struct PendingRead {
    pub request_id: RequestId,
    pub mime_type: Option<String>,
}

/// 已订阅资源的节流状态。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Subscription {
    pub last_sent: Option<Millis>,
    /// 节流期内有未发送的变化。
    pub pending: bool,
}

/// 连接相关的内部状态，断线时整体重置。
#[derive(Debug, Default)]
pub(crate) struct Session {
    pub pending_requests: VecMap<i64, Outgoing>,
    pub heartbeat: Heartbeat,
    pub reads: VecMap<ReadId, PendingRead>,
    /// 进行中的导航（[`Event::Navigate`]）→ Host 的请求 ID。
    pub navigations: VecMap<NavigateId, RequestId>,
    pub subscriptions: VecMap<String, Subscription>,
    /// 握手超时时刻（`Handshaking` 期间）。
    pub handshake_deadline: Option<Millis>,
    /// 本次 `app/hello` 携带了恢复令牌。
    pub resume_sent: bool,
    /// 已发送 `app/sleep`、等待结果（对外仍为 `Connected`）。
    pub sleeping: Option<SleepReason>,
    /// App 显式请求的休眠被拒绝后等待重试（不看空闲条件）。
    pub forced_sleep: Option<SleepReason>,
    /// 休眠被拒绝后按 `retryAfterMs` 重试的时刻。
    pub sleep_retry_at: Option<Millis>,
    /// 空闲条件（租约除外）开始成立的时刻。
    pub idle_anchor: Option<Millis>,
    /// Host 租约截止时刻（两种租约中较晚者）。
    pub lease_until: Option<Millis>,
    /// 其中自适应租约（`app/lease.adaptive`）的截止时刻：后台连接只看它（spec/lifecycle.md 第 13 节 B4）。
    pub adaptive_lease_until: Option<Millis>,
    /// Host 不支持 `app/sleep`：本连接内不再尝试自动休眠。
    pub sleep_unsupported: bool,
    /// 休眠握手期间收到唤醒 / 持有：休眠完成后立即回连。
    pub rewake: bool,
    /// Host 为本连接分配的连接 ID（`HelloResult.connectionId`，spec/protocol.md 10.3）。
    pub connection_id: Option<String>,
    /// 本连接处理过调用 / 资源读取：空闲时长改用合并窗口（spec/lifecycle.md 第 13 节 B1）。
    pub served_call: bool,
    /// 进入后台后待立即休眠（B4）：空闲条件一成立就休眠，不等租约与空闲时长。
    pub background_sleep: bool,
    /// 后台连接（B4）：握手完成时不可见且启用后台休眠——只认自适应租约，回到可见时清除。
    pub background_connection: bool,
}

fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// 成功回复的文本，`result_json` 为 `result` 成员的 JSON 文本。
///
/// @invariant 与 `Message::result(id, v).to_json()` 逐字节相同：`Value` 对象按键的字节序输出（id < jsonrpc < result）。
fn result_message(id: &RequestId, result_json: &str) -> String {
    let id = match id {
        RequestId::Number(n) => Value::from(*n),
        RequestId::String(s) => Value::String(s.clone()),
    }
    .to_string();
    let mut out = String::with_capacity(id.len() + result_json.len() + 32);
    out.push_str(r#"{"id":"#);
    out.push_str(&id);
    out.push_str(r#","jsonrpc":"2.0","result":"#);
    out.push_str(result_json);
    out.push('}');
    out
}

/// 把 `v` 的紧凑 JSON 追加到 `out`。
///
/// @why 统一经 `Value` 的 `Display`（`Message::to_json` 已用到的同一份序列化代码）：按引用序列化、不深拷贝，
/// 且不为每种结果类型实例化一套 serde 序列化器（WASM 体积）。
fn push_json(out: &mut String, v: &Value) {
    use std::fmt::Write;
    // `Value` 的序列化不会失败，写入 String 也不会失败。
    let _ = write!(out, "{v}");
}

fn push_json_str(out: &mut String, s: &str) {
    push_json(out, &Value::String(s.to_owned()));
}

/// `tools/invoke` 成功结果（[`proto::ToolsInvokeResult`] 的线上形式）的 JSON 文本。
///
/// @invariant 与 `to_value(&ToolsInvokeResult { .. }).to_string()` 逐字节相同：键按字节序
/// （annotations < data < stateHints < stateResource < status < summary），省略规则同 `ToolsInvokeResult` 的 serde 属性。
fn invoke_result_json(out: &CallOutput) -> String {
    let mut s = String::new();
    s.push('{');
    if let Some(a) = &out.annotations {
        s.push_str(r#""annotations":"#);
        push_json(&mut s, &to_value(a));
        s.push(',');
    }
    s.push_str(r#""data":"#);
    push_json(&mut s, &out.data);
    if !out.state_hints.is_empty() {
        s.push_str(r#","stateHints":["#);
        for (i, hint) in out.state_hints.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            push_json_str(&mut s, hint);
        }
        s.push(']');
    }
    if let Some(r) = &out.state_resource {
        s.push_str(r#","stateResource":"#);
        push_json_str(&mut s, r);
    }
    if !out.status.is_done() {
        s.push_str(r#","status":"#);
        push_json(&mut s, &to_value(&out.status));
    }
    if let Some(summary) = &out.summary {
        s.push_str(r#","summary":"#);
        push_json_str(&mut s, summary);
    }
    s.push('}');
    s
}

/// 资源读取结果（[`proto::ResourcesReadResult`] 的线上形式）的 JSON 文本，与经 `Value` 逐字节相同（contents < mimeType）。
fn read_result_json(contents: &Value, mime_type: Option<&str>) -> String {
    let mut s = String::from(r#"{"contents":"#);
    push_json(&mut s, contents);
    if let Some(m) = mime_type {
        s.push_str(r#","mimeType":"#);
        push_json_str(&mut s, m);
    }
    s.push('}');
    s
}

/// `tools/invoke` 参数：各字段类型都合法时直接从 `params` 中移出（参数子树不经 `from_value` 重建）；
/// 否则返回 `None`、`params` 不变，由 serde 解析给出原有的错误回复。
///
/// @invariant 接受的输入与 `serde_json::from_value::<ToolsInvokeParams>` 结果相同（未知字段忽略、缺省 / null 的可选字段为 `None`、
/// 缺省的 `arguments` 为 `null`）；结构体字面量列出全部字段，`ToolsInvokeParams` 增加字段时这里编译失败。
fn take_invoke_params(params: &mut Value) -> Option<ToolsInvokeParams> {
    let obj = params.as_object_mut()?;
    let call_id = obj.get("callId")?.as_str()?.to_owned();
    let name = obj.get("name")?.as_str()?.to_owned();
    let timeout_ms = match obj.get("timeoutMs").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64()?),
        None => None,
    };
    let idempotency_key = match obj.get("idempotencyKey").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_str()?.to_owned()),
        None => None,
    };
    // 宽松：不认识的优先级当作 normal（[`ToolsInvokeParams::priority`] 的 @compat）。
    let priority = obj.get("priority").and_then(Value::as_str).and_then(proto::CallPriority::parse).unwrap_or_default();
    let arguments = obj.remove("arguments").unwrap_or(Value::Null);
    Some(ToolsInvokeParams { call_id, name, arguments, timeout_ms, idempotency_key, priority })
}

fn tool_error(kind: ErrorKind, message: impl Into<String>) -> RpcError {
    ToolError::new(kind, message).into()
}

/// 执行中被断线 / 停止打断的调用在去重表中的结果：同一 `callId` 再次到达时如实告知"结果未知"，不再执行一次。
fn interrupted_error(call: &Call, reason: CancelReason) -> RpcError {
    let cause = match reason {
        CancelReason::Stopped => "客户端停止",
        _ => "连接断开",
    };
    ToolError::new(
        ErrorKind::Cancelled,
        format!(
            "工具 {} 的这次调用（callId {}）此前执行时因{cause}被中断，结果未知：App 内可能已部分或全部执行。请先确认状态，再决定是否以新的调用重试。",
            call.name, call.call_id
        ),
    )
    .with_details(json!({ "callId": call.call_id, "interrupted": true }))
    .into()
}

impl Client {
    // ---- 事件输出 -------------------------------------------------------

    pub(crate) fn warn(&mut self, message: impl Into<String>) {
        self.events.push_back(Event::Warning(message.into()));
    }

    pub(crate) fn set_state(&mut self, state: ConnectionState) {
        // @invariant 名字服务通道标记只在连接建立中 / 已连接期间有效（停止、拒绝、休眠等任何离开连接的路径都清除）。
        if !state.is_link_up() {
            self.life.inbound = false;
        }
        if self.state != state {
            self.state = state.clone();
            self.events.push_back(Event::StateChanged(state));
        }
    }

    fn send(&mut self, msg: Message) {
        self.events.push_back(Event::Send(msg.to_json()));
    }

    pub(crate) fn notify<T: Serialize>(&mut self, method: &str, params: &T) {
        self.send(Message::notification(method, to_value(params)));
    }

    pub(crate) fn request(&mut self, method: &str, params: Value, kind: Outgoing) -> i64 {
        self.next_request_id += 1;
        let id = self.next_request_id;
        self.session.pending_requests.insert(id, kind);
        self.send(Message::request(id, method, params));
        id
    }

    fn respond(&mut self, id: RequestId, outcome: Result<Value, RpcError>) {
        let msg = match outcome {
            Ok(v) => Message::result(id, v),
            Err(e) => Message::error(id, e),
        };
        self.send(msg);
    }

    /// 以已序列化的调用结果回复（[`Outcome`]）。
    fn respond_outcome(&mut self, id: RequestId, outcome: &Outcome) {
        match outcome {
            Ok(text) => self.events.push_back(Event::Send(result_message(&id, text))),
            Err(e) => self.respond(id, Err(e.clone())),
        }
    }

    /// 连接已建立（包括握手中）。
    pub(crate) fn link_up(&self) -> bool {
        self.state.is_link_up()
    }

    pub(crate) fn connected(&self) -> bool {
        self.state == ConnectionState::Connected
    }

    // ---- 连接生命周期 ---------------------------------------------------

    pub(crate) fn send_hello(&mut self) {
        let c = &self.config;
        let params = HelloParams {
            app_id: c.app_id.clone(),
            app_name: c.app_name.clone(),
            protocol_version: proto::PROTOCOL_VERSION.to_owned(),
            sdk_version: c.sdk_version.clone(),
            client_kind: c.client_kind,
            instance_id: c.instance_id.clone(),
            app_version: c.app_version.clone(),
            origin: c.origin.clone(),
            instance_title: c.instance_title.clone(),
            instance_url: c.instance_url.clone(),
            token: self.token.clone(),
            launch_token: self.launch_token.clone(),
            overview: c.overview.clone(),
            resume_token: None,
            tools_hash: None,
            wake_reason: self.life.wake_reason,
            // 旧行为（legacy_timers）不声明：Host 照旧发 ping 并按无消息断开（spec/lifecycle.md 第 11 节）。
            heartbeat_ms: (!c.lifecycle.legacy_timers).then(|| self.heartbeat_interval().unwrap_or(0)),
            lifecycle_mode: Some(c.lifecycle.mode),
            capabilities: c.navigation.then_some(proto::SdkCapabilities { navigate: true }),
            wake: c.lifecycle.wake.clone().filter(|w| w.kind != proto::WakeKind::None),
        };
        let params = match self.life.resume_token.clone() {
            Some(resume) => HelloParams { resume_token: Some(resume), tools_hash: Some(self.registry.tools_hash()), ..params },
            None => params,
        };
        self.session.resume_sent = params.resume_token.is_some();
        self.request(method::HELLO, to_value(&params), Outgoing::Hello);
    }

    /// 丢弃与当前连接相关的一切：调用、读取、订阅、心跳、增量追踪。
    ///
    /// `flush` 为 true 时（主动停止 / 休眠 / 拒绝）保留已排队、尚未取走的消息，驱动层在处理随后的
    /// [`Event::Disconnect`] 之前会把它们发出；为 false 时（连接已断）一并丢弃。
    pub(crate) fn teardown(&mut self, reason: CancelReason, flush: bool) {
        if !flush {
            self.events.retain(|e| !matches!(e, Event::Send(_)));
        }
        for call in self.calls.clear() {
            let outcome = Err(interrupted_error(&call, reason));
            let idem = call.idem();
            self.dedup.record(&self.config.call_dedup, &call.call_id, idem.as_deref(), outcome, self.life.last_now);
            self.events.push_back(Event::CancelTool { call_id: call.call_id, reason });
        }
        // @why 订阅随连接清空、由 Host 回连后重新订阅；记下断开时的订阅，期间的变化在重新订阅时补发（B3）。
        // 没到 Connected 的连接（握手中断开）保留上一份记录：Host 还没来得及重新订阅。
        if self.state == ConnectionState::Connected && !self.config.lifecycle.legacy_timers {
            let mut carried = std::mem::take(&mut self.session.subscriptions);
            carried.retain(|_, s| {
                *s = Subscription::default();
                true
            });
            self.life.carried_subscriptions = carried;
        }
        self.session = Session::default();
        self.registry.stop_tracking();
    }

    fn backoff_delay(&self) -> Millis {
        let p = &self.config.reconnect;
        let exp = i32::try_from(self.retry_count).unwrap_or(i32::MAX);
        let d = p.initial_delay_ms as f64 * p.multiplier.powi(exp);
        if !d.is_finite() || d >= p.max_delay_ms as f64 {
            p.max_delay_ms
        } else if d <= 0.0 {
            0
        } else {
            d as Millis
        }
    }

    /// 进入重连等待；`issue` 为本次断开 / 连接失败的原因（spec/protocol.md 10.1）。
    ///
    /// `idle` / `on-demand` 下连续 `host_absent_retries` 次以"Host 不在"失败时改为进入 `Dormant`
    /// （spec/lifecycle.md 第 11 节 A2），等可见、App 主动唤醒或 Host 唤醒再连接。
    pub(crate) fn enter_backoff(&mut self, now: Millis, issue: Option<ConnectionIssue>) {
        if self.inbound_closed() {
            return;
        }
        let absent = issue.as_ref().is_some_and(|i| i.code.means_host_absent());
        self.life.host_absent_failures = if absent { self.life.host_absent_failures.saturating_add(1) } else { 0 };
        let limit = self.config.lifecycle.host_absent_retries;
        if absent
            && limit > 0
            && self.life.host_absent_failures >= limit
            && self.config.lifecycle.mode != LifecycleMode::Persistent
            && !self.config.lifecycle.legacy_timers
        {
            self.warn(format!(
                "连续 {limit} 次连接失败（Host 未运行），停止重连并进入休眠；页面 / 界面重新可见、App 主动唤醒或 Host 唤醒时再连接"
            ));
            self.life.host_absent_failures = 0;
            self.retry_count = 0;
            self.set_state(ConnectionState::Dormant);
            return;
        }
        let delay = self.backoff_delay();
        self.retry_count = self.retry_count.saturating_add(1);
        let (reason, code) = match issue {
            Some(i) => (Some(i.message), Some(i.code)),
            None => (None, None),
        };
        self.set_state(ConnectionState::Backoff { retry_at: now.saturating_add(delay), reason, code });
    }

    /// 由核心主动断开（心跳 / 握手超时）：请求驱动层关闭连接并进入重连。
    fn drop_connection(&mut self, issue: ConnectionIssue, now: Millis) {
        self.teardown(CancelReason::Disconnected, false);
        self.events.push_back(Event::Disconnect);
        self.enter_backoff(now, Some(issue));
    }

    fn reject(&mut self, reason: String, code: ConnectionErrorCode) {
        self.teardown(CancelReason::Disconnected, true);
        self.events.push_back(Event::Disconnect);
        self.set_state(ConnectionState::Rejected { reason, code });
    }

    /// Host 给出的拒绝错误码（字符串）；缺省或不认识时为 [`ConnectionErrorCode::Rejected`]。
    fn reject_with(&mut self, reason: Option<String>, code: Option<&str>) {
        let code = code.and_then(ConnectionErrorCode::parse).unwrap_or(ConnectionErrorCode::Rejected);
        self.reject(reason.unwrap_or_else(|| "配对被拒绝".to_owned()), code);
    }

    /// 对端不是期望的 Host：断开，不再自动重连（spec/protocol.md 1.6）。
    fn host_mismatch(&mut self, issue: ConnectionIssue) {
        self.warn(issue.message.clone());
        self.teardown(CancelReason::Disconnected, true);
        self.events.push_back(Event::Disconnect);
        self.set_state(ConnectionState::HostMismatch { reason: issue.message, code: issue.code });
    }

    /// 发出此前积累的连接问题（`app/diagnostic`，spec/protocol.md 10.2）并清空。
    pub(crate) fn flush_diagnostics(&mut self) {
        for d in std::mem::take(&mut self.diagnostics) {
            self.notify(method::DIAGNOSTIC, &d);
        }
    }

    fn on_paired(&mut self, token: Option<String>, tools_current: bool, now: Millis) {
        if let Some(token) = token {
            if self.token.as_deref() != Some(token.as_str()) {
                self.token = Some(token.clone());
                self.events.push_back(Event::Paired { token });
            }
        }
        // toolsCurrent：Host 沿用休眠前的快照，跳过全量同步（spec/lifecycle.md 4.3）。
        let resumed = tools_current && self.session.resume_sent;
        if tools_current && !resumed {
            self.warn("Host 返回 toolsCurrent，但本次握手未携带恢复令牌，仍执行完整同步");
        }
        let (tools, resources) = self.registry.start_tracking();
        if !resumed {
            self.notify(method::TOOLS_SYNC, &tools);
            self.notify(method::RESOURCES_SYNC, &resources);
        }
        let vis = VisibilityParams { visibility: self.visibility, focused: self.focused };
        self.notify(method::VISIBILITY, &vis);
        self.notify(method::READY, &proto::ReadyParams {});
        self.flush_diagnostics();
        self.retry_count = 0;
        // 一次性 token 只在首次成功握手时使用。
        self.launch_token = None;
        self.life.after_handshake();
        self.session.handshake_deadline = None;
        self.life.host_absent_failures = 0;
        self.session.heartbeat =
            Heartbeat { next_ping_at: self.heartbeat_interval().map(|ms| now.saturating_add(ms)), outstanding: None };
        self.set_state(ConnectionState::Connected);
        self.mark_background_connection();
        self.refresh_idle(now);
    }
}

mod dispatch;
mod requests;
mod timers;

#[cfg(test)]
mod tests;
