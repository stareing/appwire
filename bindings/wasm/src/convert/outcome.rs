//! 调用结果（handler / 资源读取）与输出给 JS 的状态、事件。

use super::*;

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
    InvokeTool { call_id: String, tool: u64, name: String, arguments: Value, idempotency_key: Option<String> },
    CancelTool { call_id: String, reason: &'static str },
    ReadResource { read: u64, resource: u64, name: String },
    Navigate { navigate: u64, page: String, params: Value },
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
            Event::InvokeTool { call_id, tool, name, arguments, idempotency_key } => {
                JsEvent::InvokeTool { call_id, tool: tool.0, name, arguments, idempotency_key }
            }
            Event::CancelTool { call_id, reason } => JsEvent::CancelTool { call_id, reason: cancel_reason(reason) },
            Event::ReadResource { read, resource, name } => {
                JsEvent::ReadResource { read: read.0, resource: resource.0, name }
            }
            Event::Navigate { navigate, page, params } => JsEvent::Navigate { navigate: navigate.0, page, params },
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
            JsEvent::InvokeTool { call_id, tool, name, arguments, idempotency_key } => {
                let mut fields = vec![
                    ("type", "invokeTool".into()),
                    ("callId", call_id.into()),
                    ("tool", tool.into()),
                    ("name", name.into()),
                    ("arguments", arguments),
                ];
                if let Some(key) = idempotency_key {
                    fields.push(("idempotencyKey", key.into()));
                }
                object(fields)
            }
            JsEvent::CancelTool { call_id, reason } => {
                object(vec![("type", "cancelTool".into()), ("callId", call_id.into()), ("reason", reason.into())])
            }
            JsEvent::ReadResource { read, resource, name } => object(vec![
                ("type", "readResource".into()),
                ("read", read.into()),
                ("resource", resource.into()),
                ("name", name.into()),
            ]),
            JsEvent::Navigate { navigate, page, params } => object(vec![
                ("type", "navigate".into()),
                ("navigate", navigate.into()),
                ("page", page.into()),
                ("params", params),
            ]),
            JsEvent::StateChanged { state } => object(vec![("type", "stateChanged".into()), ("state", state.to_value())]),
            JsEvent::Paired { token } => object(vec![("type", "paired".into()), ("token", token.into())]),
            JsEvent::Warning { message } => object(vec![("type", "warning".into()), ("message", message.into())]),
            JsEvent::IdleExit => object(vec![("type", "idleExit".into())]),
        }
    }
}
