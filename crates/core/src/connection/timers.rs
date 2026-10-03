//! 连接状态机：心跳、节流与超时定时器。

use super::*;

impl Client {
    // ---- 定时器 ---------------------------------------------------------

    pub(super) fn heartbeat_timeout_ms(&self) -> Millis {
        match self.visibility {
            Visibility::Visible => self.config.heartbeat.timeout_ms,
            Visibility::Hidden | Visibility::Frozen => self.config.heartbeat.hidden_timeout_ms,
        }
    }

    pub(super) fn throttle_due(&self, sub: &Subscription) -> Option<Millis> {
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
                let navigation = self.session.deferred_navigations.iter().map(|d| d.deadline).min();
                [hb.next_ping_at, ping_deadline, self.calls.next_deadline(), throttle, self.sleep_deadline(), recheck, navigation]
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

    pub(super) fn on_connected_timeout(&mut self, now: Millis) {
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
        self.expire_navigations(now);

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

    pub(super) fn respond_timeout(&mut self, call: Call, started: bool) {
        let ms = call.timeout_ms.unwrap_or(0);
        let err = tool_error(
            ErrorKind::Timeout,
            format!("工具 {} 在 {ms}ms 内未完成，已取消。可以稍后重试，或检查 App 是否卡住。", call.name),
        );
        self.respond_call(call, Err(err), started);
    }

    /// 回复一次调用及挂在它上面的重复请求；`started`（handler 已开始执行）时把结果记入去重表（spec/protocol.md 3.3）。
    pub(super) fn respond_call(&mut self, call: Call, outcome: Outcome, started: bool) {
        for id in &call.waiters {
            self.respond_outcome(id.clone(), &outcome);
        }
        self.respond_outcome(call.request_id.clone(), &outcome);
        if started {
            let idem = call.idem();
            self.dedup.record(&self.config.call_dedup, &call.call_id, idem.as_deref(), outcome, self.life.last_now);
        }
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
}
