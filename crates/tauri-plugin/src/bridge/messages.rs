//! 页面消息的解析与转换（工具定义、调用结果信封）、回复与错误码、连接状态映射。

use super::*;

impl ToolSpecMessage {
    /// 拆成工具定义与选项（注解、输出 schema）。
    pub(super) fn into_parts(mut self, name: String) -> (ToolSpec, ToolOptions) {
        let options = ToolOptions {
            annotations: self.annotations.take(),
            output_schema_json: self.output_schema.take().map(|s| s.to_string()),
            surface: self.surface,
            page: self.page.take(),
            background_tool: self.background_tool.take(),
            concurrency: self.concurrency,
            exclusive: self.exclusive.take(),
            implements: std::mem::take(&mut self.implements),
        };
        (self.into_spec(name), options)
    }

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

/// 出现的字段（含 `null`）为 `Some`；缺省（`#[serde(default)]`）为 `None`。
pub(super) fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// 结果状态的合法取值（spec/protocol.md 3.2）。
const RESULT_STATUSES: [&str; 4] = ["done", "pending", "partial", "noop"];

/// 信封字段的取值是否合法；与 @app-mcp/web 的 `ENVELOPE_FIELDS`（packages/web/src/result.ts，`isToolResultEnvelope`）
/// 为同一规则：缺省或 stateHints 为数组、status 为合法取值、stateResource / summary 为字符串、annotations 为对象。
pub(super) fn envelope_field_valid(key: &str, value: Option<&Value>) -> bool {
    let Some(v) = value else { return true };
    match key {
        "stateHints" => v.is_array(),
        "status" => v.as_str().is_some_and(|s| RESULT_STATUSES.contains(&s)),
        "stateResource" | "summary" => v.is_string(),
        "annotations" => v.is_object(),
        _ => false,
    }
}

impl Outcome {
    pub(super) fn data_json(&self) -> String {
        self.data.as_ref().unwrap_or(&Value::Null).to_string()
    }

    /// 信封字段（键名 → 原始值）。
    fn envelope_fields(&self) -> [(&'static str, Option<&Value>); 5] {
        [
            ("stateHints", self.state_hints.as_ref()),
            ("status", self.status.as_ref()),
            ("stateResource", self.state_resource.as_ref()),
            ("summary", self.summary.as_ref()),
            ("annotations", self.annotations.as_ref()),
        ]
    }

    /// 成功结果：信封合法时拆开；任一字段取值不合法时整个结果（`data` 与出现的信封字段）作为 `data`、状态 `done`。
    ///
    /// @error `annotations` 是对象但字段不合法时返回说明（与 node 原生层拒绝时一样以 `HANDLER_ERROR` 结束）。
    pub(super) fn call_result(self) -> Result<CallResult, String> {
        let fields = self.envelope_fields();
        if !fields.iter().all(|(k, v)| envelope_field_valid(k, *v)) {
            let mut whole = serde_json::Map::new();
            whole.insert("data".into(), self.data.clone().unwrap_or(Value::Null));
            for (k, v) in fields {
                if let Some(v) = v {
                    whole.insert(k.into(), v.clone());
                }
            }
            return Ok(CallResult { data_json: Some(Value::Object(whole).to_string()), ..CallResult::default() });
        }
        let status = match self.status.as_ref().and_then(Value::as_str) {
            None => ResultStatus::Done,
            Some(s) => serde_json::from_value::<ResultStatus>(Value::String(s.to_owned()))
                .map_err(|e| format!("结果的 status 不合法（应为 done / pending / partial / noop）：{e}"))?,
        };
        let annotations = self
            .annotations
            .map(|v| {
                serde_json::from_value::<ContentAnnotations>(v).map_err(|e| format!("结果的 annotations 不合法：{e}"))
            })
            .transpose()?;
        let state_hints = match self.state_hints {
            Some(Value::Array(items)) => items
                .into_iter()
                .map(|h| match h {
                    Value::String(s) => s,
                    other => other.to_string(),
                })
                .collect(),
            _ => Vec::new(),
        };
        let text = |v: Option<Value>| match v {
            Some(Value::String(s)) => Some(s),
            _ => None,
        };
        Ok(CallResult {
            data_json: Some(self.data.unwrap_or(Value::Null).to_string()),
            state_hints,
            status,
            state_resource: text(self.state_resource),
            summary: text(self.summary),
            annotations,
        })
    }

    /// 失败的类别与说明；类别无法识别时按 `HANDLER_ERROR`。
    pub(super) fn error(&self) -> (ErrorKind, String) {
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

    /// 失败的结构化详情（JSON 文本）；缺省或不是对象时为 `None`。
    pub(super) fn details_json(&self) -> Option<String> {
        self.details.as_ref().filter(|d| d.is_object()).map(Value::to_string)
    }
}

pub(super) fn reply_ok(value: Option<Value>) -> Value {
    match value {
        Some(value) => json!({ "ok": true, "value": value }),
        None => json!({ "ok": true }),
    }
}

pub(super) fn reply_err(code: &str, message: impl Into<String>) -> Value {
    json!({ "ok": false, "code": code, "message": message.into() })
}

/// 与 `@app-mcp/node` 原生模块一致的错误码。
pub(super) fn native_code(error: &NativeError) -> &'static str {
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

impl From<NativeError> for OpError {
    fn from(error: NativeError) -> Self {
        Self {
            code: native_code(&error),
            message: error.to_string(),
        }
    }
}

pub(super) fn op_error(code: &'static str, message: impl Into<String>) -> OpError {
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

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
