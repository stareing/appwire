//! 连接状态机：握手、消息分发、心跳、重连、调用与资源请求处理。
//!
//! 这里是 [`Client`] 的私有实现部分；公开方法在 `lib.rs` 中，只做参数检查与转发。

use crate::vec_map::VecMap;

use app_mcp_protocol as proto;
use proto::{
    ErrorKind, HelloParams, HelloResult, Message, NavigateParams, NavigateResult, Notification, PairingResultParams,
    PairingStatus, Request, RequestId, ResourceSubscribeParams, ResourceUpdatedParams, ResourcesReadParams, ResourcesReadResult, Response,
    RpcError, ToolError, ToolsCancelParams, ToolsInvokeParams, ToolsInvokeResult, ToolsProgressParams, VisibilityParams,
    method,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::calls::Call;
use crate::dedup::Outcome;
use crate::{
    CallOutput, CancelReason, Client, ConnectionErrorCode, ConnectionIssue, ConnectionState, Event, LifecycleMode,
    Millis, NavigateId, ReadId, ResourceId, SleepReason, Visibility,
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
    /// Host 租约截止时刻。
    pub lease_until: Option<Millis>,
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
}

fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
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
        };
        let params = match self.life.resume_token.clone() {
            Some(resume) => HelloParams { resume_token: Some(resume), tools_hash: Some(self.tools_hash()), ..params },
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
            self.dedup.record(&self.config.call_dedup, &call.call_id, outcome, self.life.last_now);
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
        self.refresh_idle(now);
    }

    // ---- 定时器 ---------------------------------------------------------

    fn heartbeat_timeout_ms(&self) -> Millis {
        match self.visibility {
            Visibility::Visible => self.config.heartbeat.timeout_ms,
            Visibility::Hidden | Visibility::Frozen => self.config.heartbeat.hidden_timeout_ms,
        }
    }

    fn throttle_due(&self, sub: &Subscription) -> Option<Millis> {
        if !sub.pending {
            return None;
        }
        Some(sub.last_sent.map_or(0, |t| t.saturating_add(self.config.resource_update_throttle_ms)))
    }

    pub(crate) fn next_timeout(&self) -> Option<Millis> {
        match self.state {
            ConnectionState::Backoff { retry_at, .. } => Some(retry_at),
            ConnectionState::Handshaking => self.session.handshake_deadline,
            ConnectionState::Connected => {
                let hb = &self.session.heartbeat;
                let ping_deadline = hb.outstanding.map(|(_, sent)| sent.saturating_add(self.heartbeat_timeout_ms()));
                let throttle = self.session.subscriptions.values().filter_map(|s| self.throttle_due(s)).min();
                let recheck = self.life.idle_recheck.then_some(self.life.last_now);
                [hb.next_ping_at, ping_deadline, self.calls.next_deadline(), throttle, self.sleep_deadline(), recheck]
                    .into_iter()
                    .flatten()
                    .min()
            }
            // Dormant / Idle / Stopped / Rejected / 等待连接建立：没有定时器。
            _ => None,
        }
    }

    pub(crate) fn on_timeout(&mut self, now: Millis) {
        match self.state {
            ConnectionState::Backoff { retry_at, .. } if now >= retry_at => {
                self.events.push_back(Event::Connect);
                self.set_state(ConnectionState::Connecting);
            }
            ConnectionState::Handshaking if self.session.handshake_deadline.is_some_and(|d| now >= d) => {
                let message = format!("握手超时：{}ms 内未收到 app/hello 的结果，断开并重连", self.config.handshake_timeout_ms);
                self.warn(message.clone());
                self.drop_connection(ConnectionIssue::new(ConnectionErrorCode::HandshakeTimeout, message), now);
            }
            ConnectionState::Connected => {
                self.on_connected_timeout(now);
                if self.state == ConnectionState::Connected {
                    self.on_idle_timeout(now);
                }
            }
            _ => {}
        }
    }

    fn on_connected_timeout(&mut self, now: Millis) {
        // 心跳超时
        if let Some((_, sent)) = self.session.heartbeat.outstanding {
            let timeout = self.heartbeat_timeout_ms();
            let deadline = sent.saturating_add(timeout);
            if now >= deadline.saturating_add(timeout) {
                // @why 到期后又过了一整个超时才被调度：进程被冻结 / 挂起（如 Flyme 冻结后台进程、浏览器限流），
                // 不是 Host 无响应。不判断开，重新发 ping 计时（spec/lifecycle.md 第 11 节）。
                self.session.heartbeat = Heartbeat { next_ping_at: Some(now), outstanding: None };
            } else if now >= deadline {
                let message = format!("心跳超时：{timeout}ms 内未收到 ping 响应，断开并重连");
                self.warn(message.clone());
                self.drop_connection(ConnectionIssue::new(ConnectionErrorCode::HeartbeatTimeout, message), now);
                return;
            }
        }

        // 调用超时
        let (running, queued) = self.calls.take_expired(now);
        for call in running {
            self.events.push_back(Event::CancelTool { call_id: call.call_id.clone(), reason: CancelReason::Timeout });
            self.respond_timeout(call, true);
        }
        for call in queued {
            self.respond_timeout(call, false);
        }
        self.pump_calls();

        // 资源节流
        let throttle = self.config.resource_update_throttle_ms;
        // 按名称升序（VecMap 的迭代顺序）
        let due: Vec<String> = self
            .session
            .subscriptions
            .iter()
            .filter(|(_, s)| s.pending && s.last_sent.is_none_or(|t| now >= t.saturating_add(throttle)))
            .map(|(n, _)| n.clone())
            .collect();
        for name in due {
            self.send_resource_updated(name, now);
        }

        // 发送心跳
        let hb = self.session.heartbeat;
        if hb.outstanding.is_none() && hb.next_ping_at.is_some_and(|t| now >= t) {
            let id = self.request(method::PING, Value::Null, Outgoing::Ping);
            self.session.heartbeat = Heartbeat { next_ping_at: None, outstanding: Some((id, now)) };
        }
    }

    fn respond_timeout(&mut self, call: Call, started: bool) {
        let ms = call.timeout_ms.unwrap_or(0);
        let err = tool_error(
            ErrorKind::Timeout,
            format!("工具 {} 在 {ms}ms 内未完成，已取消。可以稍后重试，或检查 App 是否卡住。", call.name),
        );
        self.respond_call(call, Err(err), started);
    }

    /// 回复一次调用及挂在它上面的重复请求；`started`（handler 已开始执行）时把结果记入去重表（spec/protocol.md 3.3）。
    fn respond_call(&mut self, call: Call, outcome: Outcome, started: bool) {
        if started {
            self.dedup.record(&self.config.call_dedup, &call.call_id, outcome.clone(), self.life.last_now);
        }
        for id in call.waiters {
            self.respond(id, outcome.clone());
        }
        self.respond(call.request_id, outcome);
    }

    /// 发送 `tools/progress`（调用方已确认调用在执行中）。未连接时丢弃。
    pub(crate) fn send_progress(&mut self, call_id: &str, progress: f64, total: Option<f64>, message: Option<String>) {
        if !self.connected() {
            return;
        }
        if !progress.is_finite() {
            // @why 不格式化 f64：core::fmt 的浮点格式化会给 WASM 增加约 2 KB（gzip）。
            self.warn(format!("调用 {call_id:?} 的进度不是有限数，已丢弃"));
            return;
        }
        let params =
            ToolsProgressParams { call_id: call_id.to_owned(), progress, total: total.filter(|t| t.is_finite()), message };
        self.notify(method::TOOLS_PROGRESS, &params);
    }

    // ---- 调用 -----------------------------------------------------------

    /// 在并发上限内依次启动排队中的调用。
    pub(crate) fn pump_calls(&mut self) {
        let max = self.config.max_concurrent_calls.max(1);
        while self.calls.running_len() < max {
            let Some(call) = self.calls.pop_queued() else { break };
            match self.registry.tool(call.tool) {
                None => {
                    let err = tool_error(
                        ErrorKind::ToolNotFound,
                        format!("工具 {} 在排队期间已被注销。请重新获取工具列表。", call.name),
                    );
                    self.respond_call(call, Err(err), false);
                }
                Some(def) if !def.enabled => {
                    let err = tool_error(
                        ErrorKind::ToolDisabled,
                        format!("工具 {} 在排队期间已被禁用，当前不可用。", call.name),
                    );
                    self.respond_call(call, Err(err), false);
                }
                Some(_) => {
                    self.events.push_back(Event::InvokeTool {
                        call_id: call.call_id.clone(),
                        tool: call.tool,
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    });
                    self.calls.start(call);
                }
            }
        }
    }

    pub(crate) fn finish_call(&mut self, call: Call, outcome: Result<CallOutput, ToolError>) {
        let outcome = match outcome {
            Ok(out) => Ok(to_value(&ToolsInvokeResult {
                data: out.data,
                state_hints: out.state_hints,
                annotations: out.annotations,
                status: out.status,
                state_resource: out.state_resource,
                summary: out.summary,
            })),
            Err(e) => Err(RpcError::from(e)),
        };
        self.respond_call(call, outcome, true);
        self.pump_calls();
    }

    fn on_invoke(&mut self, id: RequestId, p: ToolsInvokeParams, now: Millis) {
        self.session.served_call = true;
        // 去重（spec/protocol.md 3.3）：已开始执行过的 callId 重放首次结果；进行中 / 排队中的挂到同一次执行上。
        // @why 命中只记一条警告日志（SDK 本地可观测，spec/protocol.md 3.3），不另设计数器或上报 Host。
        if let Some(outcome) = self.dedup.lookup(&self.config.call_dedup, &p.call_id, now) {
            self.warn(format!("callId {:?} 重复到达（调用去重）：重放首次结果，不再执行", p.call_id));
            self.respond(id, outcome);
            return;
        }
        if self.calls.contains(&p.call_id) {
            if self.config.call_dedup.enabled() {
                self.warn(format!("callId {:?} 重复到达（调用去重）：挂到执行中的同一次调用", p.call_id));
                self.calls.attach(&p.call_id, id);
            } else {
                let err = RpcError::invalid_params(format!("callId {:?} 已存在", p.call_id));
                self.respond(id, Err(err));
            }
            return;
        }
        let tool = match self.registry.tool_by_name(&p.name) {
            None => Err(tool_error(
                ErrorKind::ToolNotFound,
                format!("工具 {} 不存在（可能所在页面尚未加载或已离开）。请重新获取工具列表。", p.name),
            )),
            Some((_, def)) if !def.enabled => Err(tool_error(
                ErrorKind::ToolDisabled,
                format!("工具 {} 当前被禁用，App 当前状态下不可用。", p.name),
            )),
            Some((tool, _)) => Ok(tool),
        };
        let tool = match tool {
            Ok(t) => t,
            Err(e) => {
                self.respond(id, Err(e));
                return;
            }
        };
        self.calls.enqueue(Call {
            call_id: p.call_id,
            request_id: id,
            tool,
            name: p.name,
            arguments: p.arguments,
            timeout_ms: p.timeout_ms,
            deadline: p.timeout_ms.map(|t| now.saturating_add(t)),
            waiters: Vec::new(),
        });
        self.pump_calls();
    }

    fn on_cancel(&mut self, p: ToolsCancelParams) {
        let reason = p.reason.as_deref().map(|r| format!("：{r}")).unwrap_or_default();
        let (call, started) = if let Some(call) = self.calls.take_running(&p.call_id) {
            self.events.push_back(Event::CancelTool { call_id: call.call_id.clone(), reason: CancelReason::Requested });
            (call, true)
        } else if let Some(call) = self.calls.take_queued(&p.call_id) {
            (call, false)
        } else {
            self.warn(format!("tools/cancel 指向未知或已结束的调用 {:?}", p.call_id));
            return;
        };
        let err = tool_error(ErrorKind::Cancelled, format!("工具 {} 的调用已被取消{reason}", call.name));
        self.respond_call(call, Err(err), started);
        self.pump_calls();
    }

    // ---- 资源 -----------------------------------------------------------

    fn resource_not_found(name: &str) -> RpcError {
        tool_error(ErrorKind::ResourceNotFound, format!("资源 {name} 不存在。请重新获取资源列表。"))
    }

    fn on_read(&mut self, id: RequestId, p: ResourcesReadParams) {
        self.session.served_call = true;
        let Some((resource, def)) = self.registry.resource_by_name(&p.name) else {
            self.respond(id, Err(Self::resource_not_found(&p.name)));
            return;
        };
        let mime_type = def.mime_type.clone();
        self.next_read_id += 1;
        let read = ReadId(self.next_read_id);
        self.session.reads.insert(read, PendingRead { request_id: id, mime_type });
        self.events.push_back(Event::ReadResource { read, resource, name: p.name });
    }

    pub(crate) fn finish_read(&mut self, pending: PendingRead, outcome: Result<Value, ToolError>) {
        let outcome = match outcome {
            Ok(contents) => Ok(to_value(&ResourcesReadResult { contents, mime_type: pending.mime_type })),
            Err(e) => Err(RpcError::from(e)),
        };
        self.respond(pending.request_id, outcome);
    }

    // ---- 导航（spec/protocol.md 3.4）------------------------------------

    fn on_navigate(&mut self, id: RequestId, p: NavigateParams) {
        if !self.config.navigation {
            let err = ToolError::navigation_failed(
                format!("App 不支持由 Agent 导航（页面「{}」），请让用户自行打开该页面。", p.page),
                proto::navigation_reason::UNSUPPORTED,
            );
            self.respond(id, Err(err.into()));
            return;
        }
        if !proto::is_valid_name(&p.page) {
            self.respond(id, Err(RpcError::invalid_params(format!("app/navigate 的页面名不合法：{:?}", p.page))));
            return;
        }
        self.session.served_call = true;
        self.next_navigate_id += 1;
        let navigate = NavigateId(self.next_navigate_id);
        self.session.navigations.insert(navigate, id);
        self.events.push_back(Event::Navigate { navigate, page: p.page, params: p.params.unwrap_or(Value::Null) });
    }

    pub(crate) fn finish_navigate(&mut self, request_id: RequestId, outcome: Result<(), ToolError>) {
        let outcome = outcome.map(|()| to_value(&NavigateResult { ok: true })).map_err(RpcError::from);
        self.respond(request_id, outcome);
    }

    fn on_subscribe(&mut self, id: RequestId, p: ResourceSubscribeParams, subscribe: bool, now: Millis) {
        if self.registry.resource_by_name(&p.name).is_none() {
            self.respond(id, Err(Self::resource_not_found(&p.name)));
            return;
        }
        let changed_while_away = self.life.carried_subscriptions.remove(&p.name).is_some_and(|s| s.pending);
        if !subscribe {
            self.session.subscriptions.remove(&p.name);
            self.respond(id, Ok(json!({})));
            return;
        }
        self.session.subscriptions.get_or_insert_default(p.name.clone());
        self.respond(id, Ok(json!({})));
        // 未连接期间的变化：Host 重新订阅后补发（spec/lifecycle.md 第 13 节 B3）。
        if changed_while_away {
            self.send_resource_updated(p.name, now);
        }
    }

    fn send_resource_updated(&mut self, name: String, now: Millis) {
        if let Some(sub) = self.session.subscriptions.get_mut(&name) {
            sub.last_sent = Some(now);
            sub.pending = false;
            self.notify(method::RESOURCES_UPDATED, &ResourceUpdatedParams { name });
        }
    }

    pub(crate) fn on_resource_changed(&mut self, resource: ResourceId, now: Millis) {
        let Some(def) = self.registry.resource(resource) else { return };
        let (name, realtime) = (def.name.clone(), def.realtime);
        if !self.connected() || !self.session.subscriptions.contains_key(&name) {
            self.on_carried_resource_changed(&name, realtime, now);
            return;
        }
        let throttle = self.config.resource_update_throttle_ms;
        let Some(sub) = self.session.subscriptions.get_mut(&name) else { return };
        match sub.last_sent {
            Some(t) if now < t.saturating_add(throttle) => sub.pending = true,
            _ => self.send_resource_updated(name, now),
        }
    }

    // ---- 消息分发 -------------------------------------------------------

    pub(crate) fn on_message(&mut self, text: &str, now: Millis) {
        if !self.link_up() || self.state == ConnectionState::Connecting {
            self.warn(format!("当前状态 {} 下收到消息，已忽略", self.state.name()));
            return;
        }
        match Message::parse(text) {
            Ok(Message::Request(r)) => self.on_request(r, now),
            Ok(Message::Notification(n)) => self.on_notification(n, now),
            Ok(Message::Response(r)) => self.on_response(r, now),
            Err(e) => self.warn(format!("无法解析收到的消息：{e}")),
        }
    }

    /// 解析请求参数；失败时回复 -32602 并返回 `None`。
    fn parse_params<T: DeserializeOwned>(&mut self, id: &RequestId, method: &str, params: Value) -> Option<T> {
        match serde_json::from_value(params) {
            Ok(p) => Some(p),
            Err(e) => {
                self.respond(id.clone(), Err(RpcError::invalid_params(format!("{method} 的参数无效：{e}"))));
                None
            }
        }
    }

    /// 已连接时把注册表的合并变更作为 `tools/changed` / `resources/changed` 排队。
    pub(crate) fn flush_changes(&mut self) {
        if self.state == ConnectionState::Connected {
            let (tools, resources) = self.registry.take_changes();
            if let Some(tools) = tools {
                self.notify(method::TOOLS_CHANGED, &tools);
            }
            if let Some(resources) = resources {
                self.notify(method::RESOURCES_CHANGED, &resources);
            }
        }
    }

    fn on_request(&mut self, r: Request, now: Millis) {
        let Request { id, method: m, params } = r;
        match m.as_str() {
            method::PING => self.respond(id, Ok(json!({}))),
            method::TOOLS_INVOKE
            | method::RESOURCES_READ
            | method::RESOURCES_SUBSCRIBE
            | method::RESOURCES_UNSUBSCRIBE
            | method::ACTIVATE
            | method::NAVIGATE
                if !self.connected() =>
            {
                let err = tool_error(ErrorKind::Unauthorized, "App 尚未完成配对与同步，请稍后重试。");
                self.respond(id, Err(err));
            }
            method::TOOLS_INVOKE => {
                if let Some(p) = self.parse_params(&id, &m, params) {
                    self.on_invoke(id, p, now);
                }
            }
            method::RESOURCES_READ => {
                if let Some(p) = self.parse_params(&id, &m, params) {
                    self.on_read(id, p);
                }
            }
            method::RESOURCES_SUBSCRIBE | method::RESOURCES_UNSUBSCRIBE => {
                if let Some(p) = self.parse_params(&id, &m, params) {
                    self.on_subscribe(id, p, m == method::RESOURCES_SUBSCRIBE, now);
                }
            }
            method::ACTIVATE => {
                if let Some(p) = self.parse_params::<proto::ActivateParams>(&id, &m, params) {
                    self.respond(id, Ok(json!({})));
                    self.warn(format!("app/activate（mode = {:?}）在 M1 中尚未实现，已直接返回成功", p.mode));
                }
            }
            method::NAVIGATE => {
                if let Some(p) = self.parse_params(&id, &m, params) {
                    self.on_navigate(id, p);
                }
            }
            _ => {
                self.warn(format!("收到未知方法的请求 {m:?}，已返回 -32601"));
                self.respond(id, Err(RpcError::method_not_found(&m)));
            }
        }
    }

    fn on_notification(&mut self, n: Notification, now: Millis) {
        match n.method.as_str() {
            method::LEASE => {
                if !self.connected() {
                    self.warn("未连接时收到 app/lease，已忽略");
                    return;
                }
                match serde_json::from_value::<proto::LeaseParams>(n.params) {
                    Ok(p) => self.on_lease(p.ttl_ms, now),
                    Err(e) => self.warn(format!("app/lease 参数无效：{e}")),
                }
            }
            method::PAIRING_RESULT => {
                if self.state != ConnectionState::PendingPairing {
                    self.warn(format!("当前状态 {} 下收到 app/pairingResult，已忽略", self.state.name()));
                    return;
                }
                match serde_json::from_value::<PairingResultParams>(n.params) {
                    Ok(p) => match p.status {
                        PairingStatus::Paired => self.on_paired(p.token, false, now),
                        PairingStatus::Rejected => self.reject_with(p.reason, p.code.as_deref()),
                        PairingStatus::Pending => self.warn("app/pairingResult 的 status 为 pending，已忽略"),
                    },
                    Err(e) => self.warn(format!("app/pairingResult 参数无效：{e}")),
                }
            }
            method::TOOLS_CANCEL => {
                if !self.connected() {
                    self.warn("未连接时收到 tools/cancel，已忽略");
                    return;
                }
                match serde_json::from_value::<ToolsCancelParams>(n.params) {
                    Ok(p) => self.on_cancel(p),
                    Err(e) => self.warn(format!("tools/cancel 参数无效：{e}")),
                }
            }
            other => self.warn(format!("收到未知通知 {other:?}，已忽略")),
        }
    }

    fn on_response(&mut self, r: Response, now: Millis) {
        let kind = match &r.id {
            RequestId::Number(n) => self.session.pending_requests.remove(n).map(|k| (*n, k)),
            RequestId::String(_) => None,
        };
        let Some((id, kind)) = kind else {
            self.warn(format!("收到未知 ID {} 的响应，已忽略", r.id));
            return;
        };
        match kind {
            Outgoing::Hello => self.on_hello_response(r.outcome, now),
            Outgoing::Sleep => self.on_sleep_response(r.outcome, now),
            Outgoing::Ping => {
                if let Err(e) = &r.outcome {
                    self.warn(format!("ping 返回错误：{e}"));
                }
                let hb = self.session.heartbeat;
                if let Some((pid, sent)) = hb.outstanding {
                    if pid == id {
                        let next = self.heartbeat_interval().map(|ms| sent.saturating_add(ms).max(now));
                        self.session.heartbeat = Heartbeat { next_ping_at: next, outstanding: None };
                    }
                }
            }
        }
    }

    fn on_hello_response(&mut self, outcome: Result<Value, RpcError>, now: Millis) {
        if self.state != ConnectionState::Handshaking {
            self.warn(format!("当前状态 {} 下收到 app/hello 响应，已忽略", self.state.name()));
            return;
        }
        let result = match outcome {
            Ok(v) => match serde_json::from_value::<HelloResult>(v) {
                Ok(r) => r,
                Err(e) => {
                    self.host_mismatch(ConnectionIssue::new(
                        ConnectionErrorCode::HostNotAppMcp,
                        format!("对端不是 app-mcp Host：app/hello 结果无法解析（{e}）"),
                    ));
                    return;
                }
            },
            Err(e) if e.code == app_mcp_protocol::RpcError::METHOD_NOT_FOUND => {
                self.host_mismatch(ConnectionIssue::new(
                    ConnectionErrorCode::HostNotAppMcp,
                    format!("对端不是 app-mcp Host：不支持 app/hello（{}）", e.message),
                ));
                return;
            }
            Err(e) => {
                self.reject(format!("握手失败：{}", e.message), ConnectionErrorCode::Rejected);
                return;
            }
        };
        if let Err(issue) =
            app_mcp_protocol::identity::check_hello(&result, self.config.expected_host_user.as_deref())
        {
            self.host_mismatch(issue);
            return;
        }
        self.session.connection_id = result.connection_id.clone().filter(|c| !c.is_empty());
        match result.status {
            PairingStatus::Paired => self.on_paired(result.token, result.tools_current, now),
            PairingStatus::Pending => {
                self.session.handshake_deadline = None;
                self.set_state(ConnectionState::PendingPairing);
            }
            PairingStatus::Rejected => self.reject_with(result.reason, result.code.as_deref()),
        }
    }
}
