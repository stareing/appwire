//! 连接状态机：消息分发与握手回复。

use super::*;

impl Client {
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
    pub(super) fn parse_params<T: DeserializeOwned>(&mut self, id: &RequestId, method: &str, params: Value) -> Option<T> {
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

    pub(super) fn on_request(&mut self, r: Request, now: Millis) {
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
                let mut params = params;
                if let Some(p) = take_invoke_params(&mut params) {
                    self.on_invoke(id, p, now);
                } else if let Some(p) = self.parse_params(&id, &m, params) {
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
                    self.on_navigate(id, p, now);
                }
            }
            _ => {
                self.warn(format!("收到未知方法的请求 {m:?}，已返回 -32601"));
                self.respond(id, Err(RpcError::method_not_found(&m)));
            }
        }
    }

    pub(super) fn on_notification(&mut self, n: Notification, now: Millis) {
        match n.method.as_str() {
            method::LEASE => {
                if !self.connected() {
                    self.warn("未连接时收到 app/lease，已忽略");
                    return;
                }
                match serde_json::from_value::<proto::LeaseParams>(n.params) {
                    Ok(p) => self.on_lease(p.ttl_ms, p.adaptive, now),
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

    pub(super) fn on_response(&mut self, r: Response, now: Millis) {
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

    pub(super) fn on_hello_response(&mut self, outcome: Result<Value, RpcError>, now: Millis) {
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
