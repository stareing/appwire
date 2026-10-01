//! 页面桥接：WebView 中的页面经 Tauri IPC 把工具 / 资源登记到 Rust 侧的 [`NativeClient`]。
//!
//! 与 `@app-mcp/electron` 主进程（packages/electron/src/main.ts）是同一个 op/event 协议，消息类型的唯一定义在
//! `@app-mcp/web`（packages/web/src/electron-bridge.ts，`BRIDGE_VERSION = 1`）：
//!
//! - 页面 → Rust：`RendererOp`，返回 `OpReply`（`{ok: true, value?}` / `{ok: false, code?, message}`）；
//! - Rust → 页面：`MainEvent`（`call` / `cancel` / `read` / `state`），经 [`PageSink`] 送达。
//!
//! 每个 WebView（按 label）一个 [`Session`]，登记放在独立的 scope（`webview-<label>`）中；`hello`（页面重新加载）、
//! `reset`（页面卸载）、页面开始导航、窗口销毁或事件无法送达时整体注销，进行中的调用以 `APP_DISCONNECTED` 失败，
//! 页面持有的 hold 一并释放。
//!
//! 连接 ID（spec/protocol.md 10.3）随 `hello` 回复与 `state` 事件的可选字段 `connectionId` 送到页面（协议版本不变，
//! 见 electron-bridge.ts 的 `@compat`）。
//!
//! 本模块与 Tauri 无关（只依赖 [`PageSink`]），便于不启动 WebView 测试。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use app_mcp_native::{
    Activation, CallHandle, CancelListener, CancelReason, ErrorKind, HoldHandle, NativeClient,
    NativeError, ReadHandle, ResourceHandle, ResourceReader, ResourceSpec, Risk, ScopeHandle,
    StateInfo, StateStatus, ToolHandle, ToolHandler, ToolSpec,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// 桥接协议版本，须与 `@app-mcp/web` 的 `BRIDGE_VERSION` 及 `js/bridge.js` 一致。
pub const BRIDGE_VERSION: u32 = 1;

/// 把事件交给页面。返回 `false` 表示页面已不可达（会话随之注销）。
pub(crate) trait PageSink: Send + Sync + 'static {
    fn deliver(&self, event: &Value) -> bool;
}

/// 判断某个 WebView（按 label）是否允许登记。
pub(crate) type AcceptFn = dyn Fn(&str) -> bool + Send + Sync;

// ---------------------------------------------------------------------------
// 消息
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ToolSpecMessage {
    description: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "inputSchema")]
    input_schema: Option<Value>,
    #[serde(default)]
    risk: Option<Risk>,
    #[serde(default)]
    activation: Option<Activation>,
    #[serde(default)]
    enabled: Option<bool>,
}

impl ToolSpecMessage {
    fn into_spec(self, name: String) -> ToolSpec {
        let mut spec = ToolSpec::new(name, self.description);
        spec.title = self.title;
        spec.input_schema_json = self.input_schema.map(|s| s.to_string());
        if let Some(risk) = self.risk {
            spec.risk = risk;
        }
        spec.activation = self.activation;
        if let Some(enabled) = self.enabled {
            spec.enabled = enabled;
        }
        spec
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
enum PageOp {
    #[serde(rename = "hello")]
    Hello,
    #[serde(rename = "reset")]
    Reset,
    #[serde(rename = "tool.register")]
    ToolRegister {
        id: u64,
        #[serde(default, rename = "scopeId")]
        scope_id: Option<u64>,
        name: String,
        spec: ToolSpecMessage,
    },
    #[serde(rename = "tool.update")]
    ToolUpdate { id: u64, spec: ToolSpecMessage },
    #[serde(rename = "tool.dispose")]
    ToolDispose { id: u64 },
    #[serde(rename = "resource.register")]
    ResourceRegister {
        id: u64,
        #[serde(default, rename = "scopeId")]
        scope_id: Option<u64>,
        name: String,
        description: String,
        #[serde(default, rename = "mimeType")]
        mime_type: Option<String>,
    },
    #[serde(rename = "resource.notify")]
    ResourceNotify { id: u64 },
    #[serde(rename = "resource.dispose")]
    ResourceDispose { id: u64 },
    #[serde(rename = "scope.create")]
    ScopeCreate {
        id: u64,
        #[serde(default, rename = "scopeId")]
        scope_id: Option<u64>,
        name: String,
    },
    #[serde(rename = "scope.dispose")]
    ScopeDispose { id: u64 },
    #[serde(rename = "call.result")]
    CallResult {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(flatten)]
        outcome: Outcome,
    },
    #[serde(rename = "read.result")]
    ReadResult {
        #[serde(rename = "readId")]
        read_id: u64,
        #[serde(flatten)]
        outcome: Outcome,
    },
    #[serde(rename = "lifecycle.wake")]
    Wake,
    #[serde(rename = "lifecycle.sleep")]
    Sleep,
    #[serde(rename = "lifecycle.connectNow")]
    ConnectNow,
    #[serde(rename = "lifecycle.hold")]
    Hold {
        #[serde(rename = "holdId")]
        hold_id: u64,
    },
    #[serde(rename = "lifecycle.release")]
    Release {
        #[serde(rename = "holdId")]
        hold_id: u64,
    },
}

/// `Outcome`：`{ok: true, data, stateHints?}` / `{ok: false, kind, message}`。
#[derive(Debug, Deserialize)]
struct Outcome {
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default, rename = "stateHints")]
    state_hints: Option<Vec<String>>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl Outcome {
    fn data_json(&self) -> String {
        self.data.as_ref().unwrap_or(&Value::Null).to_string()
    }

    /// 失败的类别与说明；类别无法识别时按 `HANDLER_ERROR`。
    fn error(&self) -> (ErrorKind, String) {
        let kind = self
            .kind
            .as_ref()
            .and_then(|k| serde_json::from_value(Value::String(k.clone())).ok())
            .unwrap_or(ErrorKind::HandlerError);
        let message = self
            .message
            .clone()
            .unwrap_or_else(|| "页面 handler 执行失败".to_owned());
        (kind, message)
    }
}

fn reply_ok(value: Option<Value>) -> Value {
    match value {
        Some(value) => json!({ "ok": true, "value": value }),
        None => json!({ "ok": true }),
    }
}

fn reply_err(code: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "code": code, "message": message.into() })
}

/// 与 `@app-mcp/node` 原生模块一致的错误码。
fn native_code(error: &NativeError) -> &'static str {
    match error {
        NativeError::InvalidName(_) => "INVALID_NAME",
        NativeError::InvalidSchema(_) => "INVALID_SCHEMA",
        NativeError::DuplicateName(_) => "DUPLICATE_NAME",
        NativeError::InvalidJson(_) => "INVALID_JSON",
        NativeError::InvalidConfig(_) => "INVALID_CONFIG",
        NativeError::AlreadyCompleted => "ALREADY_COMPLETED",
        NativeError::Disposed => "DISPOSED",
        NativeError::Stopped => "STOPPED",
        NativeError::Internal(_) => "INTERNAL",
    }
}

#[derive(Debug)]
struct OpError {
    code: &'static str,
    message: String,
}

impl From<NativeError> for OpError {
    fn from(error: NativeError) -> Self {
        Self {
            code: native_code(&error),
            message: error.to_string(),
        }
    }
}

fn op_error(code: &'static str, message: impl Into<String>) -> OpError {
    OpError {
        code,
        message: message.into(),
    }
}

/// 连接状态 → `@app-mcp/web` 的 `ConnectionState`（与 `@app-mcp/node` 的映射一致）。
pub(crate) fn state_json(state: &StateInfo) -> Value {
    let reason = || state.reason.clone().unwrap_or_default();
    let code = |default: &str| state.code.clone().unwrap_or_else(|| default.to_owned());
    match state.status {
        StateStatus::Idle => json!({ "status": "idle" }),
        StateStatus::Connecting => json!({ "status": "connecting" }),
        StateStatus::Handshaking => json!({ "status": "handshaking" }),
        StateStatus::PendingPairing => json!({ "status": "pending-pairing" }),
        StateStatus::Connected => json!({ "status": "connected" }),
        StateStatus::Backoff => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0);
            let mut value = json!({ "status": "backoff", "retryAt": now.saturating_add(state.retry_in_ms.unwrap_or(0)) });
            if let Some(reason) = &state.reason {
                value["reason"] = json!(reason);
            }
            if let Some(code) = &state.code {
                value["code"] = json!(code);
            }
            value
        }
        StateStatus::Rejected => {
            json!({ "status": "rejected", "reason": reason(), "code": code("REJECTED") })
        }
        StateStatus::Stopped => json!({ "status": "stopped" }),
        StateStatus::Dormant => json!({ "status": "dormant" }),
        StateStatus::Waking => json!({ "status": "waking" }),
        StateStatus::HostMismatch => {
            json!({ "status": "host-mismatch", "reason": reason(), "code": code("HOST_NOT_APP_MCP") })
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// 桥接
// ---------------------------------------------------------------------------

/// 一个 App 的页面桥接：持有原生客户端与全部 WebView 会话。
pub(crate) struct Bridge {
    client: NativeClient,
    sessions: Arc<Sessions>,
    accept: Option<Arc<AcceptFn>>,
}

impl Bridge {
    pub(crate) fn new(
        client: NativeClient,
        sessions: Arc<Sessions>,
        accept: Option<Arc<AcceptFn>>,
    ) -> Self {
        sessions.attach_client(client.clone());
        Self {
            client,
            sessions,
            accept,
        }
    }

    pub(crate) fn client(&self) -> &NativeClient {
        &self.client
    }

    pub(crate) fn sessions(&self) -> &Arc<Sessions> {
        &self.sessions
    }

    /// 处理页面发来的一条操作。`sink` 只在需要为该 WebView 新建会话时调用。
    pub(crate) fn handle(
        &self,
        label: &str,
        window: &str,
        sink: impl FnOnce() -> Arc<dyn PageSink>,
        raw: Value,
    ) -> Value {
        if let Some(accept) = &self.accept
            && !accept(label)
        {
            return reply_err("FORBIDDEN", "该页面不允许登记 app-mcp 工具");
        }
        let op: PageOp = match serde_json::from_value(raw) {
            Ok(op) => op,
            Err(e) => return reply_err("INVALID_OP", format!("非法的消息：{e}")),
        };
        match op {
            PageOp::Hello => {
                self.sessions.end(label);
                let mut hello = json!({
                    "instanceId": self.client.instance_id(),
                    "state": state_json(&self.client.state()),
                });
                if let Some(cid) = self.client.connection_id() {
                    hello["connectionId"] = Value::String(cid);
                }
                reply_ok(Some(hello))
            }
            PageOp::Reset => {
                self.sessions.end(label);
                reply_ok(None)
            }
            op => {
                let session = match self.sessions.get_or_create(label, || {
                    Session::new(
                        label,
                        window,
                        &self.client,
                        sink(),
                        Arc::downgrade(&self.sessions),
                    )
                }) {
                    Ok(session) => session,
                    Err(e) => return reply_err(e.code, e.message),
                };
                match session.handle(&self.client, op) {
                    Ok(value) => reply_ok(value),
                    Err(e) => reply_err(e.code, e.message),
                }
            }
        }
    }
}

impl Drop for Bridge {
    /// @invariant 断开 `Sessions` 持有的客户端克隆（见 [`Sessions::attach_client`]），
    /// 使 `self.client` 成为最后一个所有者，客户端随插件状态一起停止。
    fn drop(&mut self) {
        self.sessions.detach_client();
    }
}

/// 全部 WebView 会话（按 label）。
#[derive(Default)]
pub(crate) struct Sessions {
    map: Mutex<HashMap<String, Arc<Session>>>,
    /// 广播状态时读取连接 ID 用。
    /// @why 状态回调（`PluginListener`）在客户端创建之前就要构造，`StateInfo` 不带连接 ID，只能回头问客户端；
    /// 客户端持有监听器、监听器持有本结构，因此这里是一个引用环，由 [`Bridge`] 的 `Drop` 断开。
    /// @invariant 只在持锁期间使用，不把克隆带出锁外：否则最后一个克隆可能在分发线程上释放，
    /// 而 `NativeClient` 的析构要等待分发线程结束。
    client: Mutex<Option<NativeClient>>,
}

impl Sessions {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn get_or_create(
        &self,
        label: &str,
        create: impl FnOnce() -> Result<Session, OpError>,
    ) -> Result<Arc<Session>, OpError> {
        let mut map = lock(&self.map);
        if let Some(session) = map.get(label) {
            return Ok(session.clone());
        }
        let session = Arc::new(create()?);
        map.insert(label.to_owned(), session.clone());
        Ok(session)
    }

    /// 当前有登记的 WebView 数量。
    pub(crate) fn count(&self) -> usize {
        lock(&self.map).len()
    }

    pub(crate) fn attach_client(&self, client: NativeClient) {
        *lock(&self.client) = Some(client);
    }

    pub(crate) fn detach_client(&self) {
        let client = lock(&self.client).take();
        drop(client);
    }

    /// 客户端当前的连接 ID；未连接、旧 Host 或未关联客户端时为 `None`。
    fn connection_id(&self) -> Option<String> {
        lock(&self.client).as_ref().and_then(NativeClient::connection_id)
    }

    /// 注销该 WebView 的全部登记。
    pub(crate) fn end(&self, label: &str) {
        let removed = lock(&self.map).remove(label);
        if let Some(session) = removed {
            session.dispose();
        }
    }

    /// 注销该窗口内全部 WebView 的登记（窗口销毁）。
    pub(crate) fn end_window(&self, window: &str) {
        let removed: Vec<Arc<Session>> = {
            let mut map = lock(&self.map);
            let labels: Vec<String> = map
                .iter()
                .filter(|(_, s)| s.window == window)
                .map(|(label, _)| label.clone())
                .collect();
            labels
                .iter()
                .filter_map(|label| map.remove(label))
                .collect()
        };
        for session in removed {
            session.dispose();
        }
    }

    /// 注销全部登记。
    pub(crate) fn end_all(&self) {
        let removed: Vec<Arc<Session>> = lock(&self.map).drain().map(|(_, s)| s).collect();
        for session in removed {
            session.dispose();
        }
    }

    fn end_session(&self, session: &Arc<Session>) {
        let removed = {
            let mut map = lock(&self.map);
            match map.get(&session.label) {
                Some(current) if Arc::ptr_eq(current, session) => map.remove(&session.label),
                _ => None,
            }
        };
        session.dispose();
        drop(removed);
    }

    /// 把连接状态转发给全部页面。
    /// 连接 ID 在分发时读取：状态在此期间又变化时可能与 `state` 不一致，随后的状态事件会更正。
    pub(crate) fn broadcast_state(&self, state: &StateInfo) {
        let mut event = json!({ "type": "state", "state": state_json(state) });
        if let Some(cid) = self.connection_id() {
            event["connectionId"] = Value::String(cid);
        }
        let sessions: Vec<Arc<Session>> = lock(&self.map).values().cloned().collect();
        for session in sessions {
            session.send(&event);
        }
    }
}

#[derive(Default)]
struct SessionState {
    disposed: bool,
    tools: HashMap<u64, ToolHandle>,
    resources: HashMap<u64, ResourceHandle>,
    scopes: HashMap<u64, ScopeHandle>,
    calls: HashMap<String, CallHandle>,
    reads: HashMap<u64, ReadHandle>,
    holds: HashMap<u64, HoldHandle>,
    next_read_id: u64,
}

/// 一个 WebView 的登记。
pub(crate) struct Session {
    label: String,
    window: String,
    scope: ScopeHandle,
    sink: Arc<dyn PageSink>,
    owner: Weak<Sessions>,
    state: Mutex<SessionState>,
}

impl Session {
    fn new(
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
    fn send(self: &Arc<Self>, event: &Value) {
        if lock(&self.state).disposed {
            return;
        }
        if !self.sink.deliver(event)
            && let Some(owner) = self.owner.upgrade()
        {
            owner.end_session(self);
        }
    }

    fn handle(
        self: &Arc<Self>,
        client: &NativeClient,
        op: PageOp,
    ) -> Result<Option<Value>, OpError> {
        match op {
            PageOp::Hello | PageOp::Reset => Ok(None),
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
                let handle = match registrar {
                    Some(scope) => scope.register_tool(spec.into_spec(name), handler)?,
                    None => self.scope.register_tool(spec.into_spec(name), handler)?,
                };
                st.tools.insert(id, handle);
                Ok(None)
            }
            PageOp::ToolUpdate { id, spec } => {
                let st = self.live()?;
                if let Some(tool) = st.tools.get(&id) {
                    tool.update(spec.into_spec(tool.name()))?;
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
                let handle = match registrar {
                    Some(scope) => scope.register_resource(spec, reader)?,
                    None => self.scope.register_resource(spec, reader)?,
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
                    call.complete(
                        Some(&outcome.data_json()),
                        outcome.state_hints.unwrap_or_default(),
                    )
                } else {
                    let (kind, message) = outcome.error();
                    call.fail(kind, &message)
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
            PageOp::ReadResult { read_id, outcome } => {
                let Some(read) = lock(&self.state).reads.remove(&read_id) else {
                    return Ok(None);
                };
                let done = if outcome.ok {
                    read.complete(&outcome.data_json())
                } else {
                    let (kind, message) = outcome.error();
                    read.fail(kind, &message)
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

    fn live(&self) -> Result<MutexGuard<'_, SessionState>, OpError> {
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
    fn forward_call(self: &Arc<Self>, tool_id: u64, call: CallHandle) {
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
        self.send(&json!({ "type": "call", "callId": call_id, "toolId": tool_id, "input": input }));
        // 先发 call 再挂取消监听：已取消时监听立即回调，页面收到的 cancel 总在 call 之后。
        call.set_cancel_listener(Arc::new(PageCancel {
            session: Arc::downgrade(self),
            call_id,
        }));
    }

    fn forward_read(self: &Arc<Self>, resource_id: u64, read: ReadHandle) {
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

    fn on_cancel(self: &Arc<Self>, call_id: &str, reason: CancelReason) {
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
    fn dispose(&self) {
        let st = {
            let mut st = lock(&self.state);
            if st.disposed {
                return;
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
    }
}

struct PageTool {
    session: Weak<Session>,
    tool_id: u64,
}

impl ToolHandler for PageTool {
    fn invoke(&self, call: CallHandle) {
        match self.session.upgrade() {
            Some(session) => session.forward_call(self.tool_id, call),
            None => {
                let _ = call.fail(ErrorKind::AppDisconnected, "页面已关闭");
            }
        }
    }
}

struct PageResource {
    session: Weak<Session>,
    resource_id: u64,
}

impl ResourceReader for PageResource {
    fn read(&self, read: ReadHandle) {
        match self.session.upgrade() {
            Some(session) => session.forward_read(self.resource_id, read),
            None => {
                let _ = read.fail(ErrorKind::AppDisconnected, "页面已关闭");
            }
        }
    }
}

struct PageCancel {
    session: Weak<Session>,
    call_id: String,
}

impl CancelListener for PageCancel {
    fn on_cancel(&self, reason: CancelReason) {
        if let Some(session) = self.session.upgrade() {
            session.on_cancel(&self.call_id, reason);
        }
    }
}
