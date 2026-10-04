//! [`Session`]：一个 WebView 的登记与页面 op 的处理。

use super::*;

impl Session {
    pub(super) fn new(
        label: &str,
        window: &str,
        client: &NativeClient,
        sink: Arc<dyn PageSink>,
        owner: Weak<Sessions>,
    ) -> Result<Self, OpError> {
        let scope = client.create_scope(&format!("webview-{label}"))?;
        Ok(Self {
            label: label.to_owned(),
            window: window.to_owned(),
            scope,
            sink,
            owner,
            state: Mutex::new(SessionState::default()),
        })
    }

    /// 发送事件（不持有会话锁）；页面不可达时注销本会话。
    pub(super) fn send(self: &Arc<Self>, event: &Value) {
        if lock(&self.state).disposed {
            return;
        }
        if !self.sink.deliver(event)
            && let Some(owner) = self.owner.upgrade()
        {
            owner.end_session(self);
        }
    }

    pub(super) fn handle(
        self: &Arc<Self>,
        client: &NativeClient,
        op: PageOp,
    ) -> Result<Option<Value>, OpError> {
        match op {
            // 由 Bridge::handle 处理，不经会话
            PageOp::Hello | PageOp::Reset | PageOp::NavigationSet { .. } | PageOp::NavigateResult { .. } => Ok(None),
            PageOp::ToolRegister {
                id,
                scope_id,
                name,
                spec,
            } => {
                let mut st = self.live()?;
                let registrar = self.registrar(&st, scope_id)?;
                if st.tools.contains_key(&id) {
                    return Err(op_error("DUPLICATE_ID", format!("工具 id {id} 已被使用")));
                }
                let handler = Arc::new(PageTool {
                    session: Arc::downgrade(self),
                    tool_id: id,
                });
                let (spec, options) = spec.into_parts(name);
                let handle = match registrar {
                    Some(scope) => scope.register_tool_with(spec, options, handler)?,
                    None => self.scope.register_tool_with(spec, options, handler)?,
                };
                st.tools.insert(id, handle);
                Ok(None)
            }
            PageOp::ToolUpdate { id, spec } => {
                let st = self.live()?;
                if let Some(tool) = st.tools.get(&id) {
                    let (spec, options) = spec.into_parts(tool.name());
                    tool.update_with(spec, options)?;
                }
                Ok(None)
            }
            PageOp::ToolDispose { id } => {
                let tool = self.live()?.tools.remove(&id);
                if let Some(tool) = tool {
                    tool.dispose();
                }
                Ok(None)
            }
            PageOp::ResourceRegister {
                id,
                scope_id,
                name,
                description,
                mime_type,
                realtime,
                annotations,
            } => {
                let mut st = self.live()?;
                let registrar = self.registrar(&st, scope_id)?;
                if st.resources.contains_key(&id) {
                    return Err(op_error("DUPLICATE_ID", format!("资源 id {id} 已被使用")));
                }
                let spec = ResourceSpec {
                    name,
                    description,
                    mime_type,
                };
                let reader = Arc::new(PageResource {
                    session: Arc::downgrade(self),
                    resource_id: id,
                });
                let options = ResourceOptions {
                    realtime,
                    annotations,
                    cache: None,
                };
                let handle = match registrar {
                    Some(scope) => scope.register_resource_with(spec, options, reader)?,
                    None => self.scope.register_resource_with(spec, options, reader)?,
                };
                st.resources.insert(id, handle);
                Ok(None)
            }
            PageOp::ResourceNotify { id } => {
                let st = self.live()?;
                if let Some(resource) = st.resources.get(&id) {
                    resource.notify_changed()?;
                }
                Ok(None)
            }
            PageOp::ResourceDispose { id } => {
                let resource = self.live()?.resources.remove(&id);
                if let Some(resource) = resource {
                    resource.dispose();
                }
                Ok(None)
            }
            PageOp::ScopeCreate { id, scope_id, name } => {
                let mut st = self.live()?;
                let registrar = self.registrar(&st, scope_id)?;
                if st.scopes.contains_key(&id) {
                    return Err(op_error("DUPLICATE_ID", format!("scope id {id} 已被使用")));
                }
                let scope = match registrar {
                    Some(parent) => parent.create_scope(&name)?,
                    None => self.scope.create_scope(&name)?,
                };
                st.scopes.insert(id, scope);
                Ok(None)
            }
            PageOp::ScopeDispose { id } => {
                let scope = self.live()?.scopes.remove(&id);
                if let Some(scope) = scope {
                    scope.dispose();
                }
                Ok(None)
            }
            PageOp::CallResult { call_id, outcome } => {
                // 已取消、超时或会话已注销：忽略。
                let Some(call) = lock(&self.state).calls.remove(&call_id) else {
                    return Ok(None);
                };
                let done = if outcome.ok {
                    match outcome.call_result() {
                        Ok(result) => call.complete_with(result),
                        Err(message) => {
                            let _ = call.fail(ErrorKind::HandlerError, &message);
                            return Err(op_error("INVALID_RESULT", message));
                        }
                    }
                } else {
                    let (kind, message) = outcome.error();
                    call.fail_with_details(kind, &message, outcome.details_json().as_deref())
                };
                match done {
                    // 结果无法提交（如数据不是合法 JSON）：调用以失败结束，并把原因告诉页面。
                    Err(NativeError::InvalidJson(e)) => {
                        let _ = call.fail(
                            ErrorKind::HandlerError,
                            &format!("返回值无法序列化为 JSON：{e}"),
                        );
                        Err(op_error("INVALID_JSON", e))
                    }
                    // 调用在此期间被取消：忽略。
                    _ => Ok(None),
                }
            }
            PageOp::CallProgress { call_id, progress, total, message } => {
                // 调用已结束 / 取消时无接收方；进度只是提示，不报错。
                let call = lock(&self.state).calls.get(&call_id).cloned();
                if let Some(call) = call {
                    let _ = call.report_progress(progress, total, message.as_deref());
                }
                Ok(None)
            }
            PageOp::ReadResult { read_id, outcome } => {
                let Some(read) = lock(&self.state).reads.remove(&read_id) else {
                    return Ok(None);
                };
                let done = if outcome.ok {
                    read.complete(&outcome.data_json())
                } else {
                    let (kind, message) = outcome.error();
                    read.fail_with_details(kind, &message, outcome.details_json().as_deref())
                };
                match done {
                    Err(NativeError::InvalidJson(e)) => {
                        let _ = read.fail(
                            ErrorKind::HandlerError,
                            &format!("资源内容无法序列化为 JSON：{e}"),
                        );
                        Err(op_error("INVALID_JSON", e))
                    }
                    _ => Ok(None),
                }
            }
            PageOp::BusySet { busy } => {
                self.live()?.busy = busy;
                if let Some(owner) = self.owner.upgrade() {
                    owner.sync_busy();
                }
                Ok(None)
            }
            PageOp::EventDeclare { event } => {
                self.declare_event(client, event)?;
                Ok(None)
            }
            PageOp::EventRemove { name } => {
                let removed = self.remove_event(&name);
                if removed && let Some(owner) = self.owner.upgrade() {
                    owner.release_events(vec![name]);
                }
                Ok(Some(Value::Bool(removed)))
            }
            PageOp::EventEmit { name, payload } => {
                Ok(Some(Value::Bool(Self::emit_event(client, &name, payload.as_ref())?)))
            }
            PageOp::Wake => Ok(Some(Value::Bool(client.wake()))),
            PageOp::Sleep => Ok(Some(Value::Bool(client.sleep()))),
            PageOp::ConnectNow => Ok(Some(Value::Bool(client.connect_now()))),
            PageOp::Hold { hold_id } => {
                let mut st = self.live()?;
                st.holds.entry(hold_id).or_insert_with(|| client.hold());
                Ok(None)
            }
            PageOp::Release { hold_id } => {
                let hold = lock(&self.state).holds.remove(&hold_id);
                if let Some(hold) = hold {
                    hold.release();
                }
                Ok(None)
            }
        }
    }

    pub(super) fn live(&self) -> Result<MutexGuard<'_, SessionState>, OpError> {
        let st = lock(&self.state);
        if st.disposed {
            return Err(op_error("DISPOSED", "页面会话已注销"));
        }
        Ok(st)
    }

    /// `scopeId` 对应的 scope；缺省为本页面的根 scope（返回 `None`）。
    fn registrar(
        &self,
        st: &SessionState,
        scope_id: Option<u64>,
    ) -> Result<Option<ScopeHandle>, OpError> {
        match scope_id {
            None => Ok(None),
            Some(id) => st
                .scopes
                .get(&id)
                .cloned()
                .map(Some)
                .ok_or_else(|| op_error("UNKNOWN_SCOPE", format!("未知的 scope {id}"))),
        }
    }

    /// Host 调用了页面的工具：转给页面执行。在原生分发线程上调用。
    pub(super) fn forward_call(self: &Arc<Self>, tool_id: u64, call: CallHandle) {
        let input: Value = match serde_json::from_str(&call.arguments_json()) {
            Ok(input) => input,
            Err(e) => {
                let _ = call.fail(ErrorKind::InvalidInput, &format!("参数不是合法 JSON：{e}"));
                return;
            }
        };
        let call_id = call.call_id();
        {
            let mut st = lock(&self.state);
            if st.disposed {
                drop(st);
                let _ = call.fail(ErrorKind::AppDisconnected, "页面已关闭");
                return;
            }
            st.calls.insert(call_id.clone(), call.clone());
        }
        let mut event = json!({ "type": "call", "callId": call_id, "toolId": tool_id, "input": input });
        // Agent 的幂等键（spec/protocol.md 3.3）：有才带，页面侧缺省即没有
        if let Some(key) = call.idempotency_key() {
            event["idempotencyKey"] = Value::String(key);
        }
        self.send(&event);
        // 先发 call 再挂取消监听：已取消时监听立即回调，页面收到的 cancel 总在 call 之后。
        call.set_cancel_listener(Arc::new(PageCancel {
            session: Arc::downgrade(self),
            call_id,
        }));
    }

    pub(super) fn forward_read(self: &Arc<Self>, resource_id: u64, read: ReadHandle) {
        let read_id = {
            let mut st = lock(&self.state);
            if st.disposed {
                drop(st);
                let _ = read.fail(ErrorKind::AppDisconnected, "页面已关闭");
                return;
            }
            st.next_read_id += 1;
            let read_id = st.next_read_id;
            st.reads.insert(read_id, read);
            read_id
        };
        self.send(&json!({ "type": "read", "readId": read_id, "resourceId": resource_id }));
    }

    pub(super) fn on_cancel(self: &Arc<Self>, call_id: &str, reason: CancelReason) {
        if lock(&self.state).calls.remove(call_id).is_none() {
            return;
        }
        let (kind, message) = match reason {
            CancelReason::Requested => ("CANCELLED", "调用已取消"),
            CancelReason::Timeout => ("TIMEOUT", "调用超时"),
            CancelReason::Disconnected => ("APP_DISCONNECTED", "与 Host 的连接已断开"),
            CancelReason::Stopped => ("CANCELLED", "SDK 已停止"),
        };
        self.send(
            &json!({ "type": "cancel", "callId": call_id, "kind": kind, "message": message }),
        );
    }

    /// 注销本页面的全部登记：进行中的调用 / 读取以 `APP_DISCONNECTED` 失败，释放 hold，销毁 scope。
    /// 本页声明用户正在操作且尚未注销。
    pub(super) fn busy(&self) -> bool {
        let st = lock(&self.state);
        !st.disposed && st.busy
    }

    /// 返回本页声明过的事件名（交给 [`Sessions::release_events`]）；已注销时为空。
    pub(super) fn dispose(&self) -> Vec<String> {
        let st = {
            let mut st = lock(&self.state);
            if st.disposed {
                return Vec::new();
            }
            st.disposed = true;
            std::mem::take(&mut *st)
        };
        for call in st.calls.into_values() {
            let _ = call.fail(ErrorKind::AppDisconnected, "页面已关闭或刷新");
        }
        for read in st.reads.into_values() {
            let _ = read.fail(ErrorKind::AppDisconnected, "页面已关闭或刷新");
        }
        for hold in st.holds.into_values() {
            hold.release();
        }
        self.scope.dispose();
        st.events.into_iter().map(|e| e.name).collect()
    }
}
