//! 配置校验与核心 ↔ 原生类型的转换。

use super::*;

/// 校验端点：格式合法，且本平台支持该传输。
fn parse_endpoint(text: &str) -> Result<app_mcp_protocol::Endpoint, NativeError> {
    use app_mcp_protocol::Endpoint;
    let endpoint =
        Endpoint::parse(text).map_err(|e| NativeError::InvalidConfig(format!("host_url 无效：{e}")))?;
    let supported = match &endpoint {
        Endpoint::WebSocket(_) => true,
        Endpoint::Unix(_) => cfg!(unix),
        Endpoint::Pipe(_) => cfg!(windows),
    };
    if !supported {
        return Err(NativeError::InvalidConfig(format!(
            "本平台不支持端点 {text:?}"
        )));
    }
    Ok(endpoint)
}

/// 校验配置并转换为核心配置；同时返回 Host 端点。
pub(crate) fn build_core_config(
    config: NativeConfig,
) -> Result<(ClientConfig, app_mcp_protocol::Endpoint, Option<names::NameRequest>), NativeError> {
    let endpoint = parse_endpoint(&config.host_url)?;
    if let Some(inst) = &config.name_instance
        && !app_mcp_protocol::naming::is_valid_instance(inst)
    {
        return Err(NativeError::InvalidConfig(format!(
            "name_instance 必须匹配 [a-z][a-z0-9-]{{0,31}} 且不是 default：{inst:?}"
        )));
    }
    let name_request = config.register_name.then(|| names::NameRequest {
        app_id: config.app_id.clone(),
        instance: config.name_instance.clone(),
        address: config.name_service_address.clone().filter(|a| !a.is_empty()),
    });
    if !app_mcp_protocol::is_valid_app_id(&config.app_id) {
        return Err(NativeError::InvalidConfig(format!(
            "app_id 必须匹配 [a-z][a-z0-9-]{{0,62}}：{:?}",
            config.app_id
        )));
    }
    if config.max_concurrent_calls == 0 {
        return Err(NativeError::InvalidConfig(
            "max_concurrent_calls 必须大于 0".to_owned(),
        ));
    }
    let instance_id = match config.instance_id {
        Some(id) if !id.is_empty() => id,
        Some(_) => {
            return Err(NativeError::InvalidConfig(
                "instance_id 不能为空".to_owned(),
            ));
        }
        None => process_instance_id(),
    };
    let launch_token = config
        .launch_token
        .or_else(|| std::env::var("APP_MCP_LAUNCH_TOKEN").ok())
        .filter(|t| !t.is_empty());

    let mut inner = ClientConfig::new(
        config.app_id,
        config.app_name,
        instance_id,
        config.client_kind,
    );
    inner.app_version = config.app_version;
    inner.instance_title = config.instance_title;
    inner.token = config.token;
    inner.launch_token = launch_token;
    inner.overview = config.overview;
    inner.lifecycle = config.lifecycle;
    inner.heartbeat.mode = config.heartbeat;
    inner.transport = endpoint.transport_kind(&app_mcp_protocol::platform::Target::CURRENT);
    inner.expected_host_user = app_mcp_protocol::identity::expected_host_user();
    inner.max_concurrent_calls = usize::try_from(config.max_concurrent_calls).unwrap_or(usize::MAX);
    inner.max_queued_calls = usize::try_from(config.max_queued_calls).unwrap_or(usize::MAX);
    inner.busy_policy = config.busy_policy;
    inner.call_dedup = config.call_dedup;
    inner.navigate_in_background = app_mcp_protocol::platform::Target::CURRENT.allows_self_foreground();
    inner.launched_by_activation = name_request.is_some()
        && std::env::args().any(|a| a == app_mcp_protocol::naming::dbus::ACTIVATION_ARG);
    Ok((inner, endpoint, name_request))
}

/// 核心错误 → 原生错误。未知句柄一律视为已注销，未知调用 / 读取视为已完成。
pub(crate) fn core_error(e: CoreError) -> NativeError {
    match e {
        CoreError::InvalidName(n) => NativeError::InvalidName(n),
        CoreError::InvalidSchema => {
            NativeError::InvalidSchema(CoreError::InvalidSchema.to_string())
        }
        CoreError::DuplicateName(n) => NativeError::DuplicateName(n),
        CoreError::UnknownTool(_) | CoreError::UnknownResource(_) | CoreError::UnknownScope(_) => {
            NativeError::Disposed
        }
        CoreError::UnknownCall(_) | CoreError::UnknownRead(_) | CoreError::UnknownNavigate(_) => {
            NativeError::AlreadyCompleted
        }
        // @compat 不新增 NativeError 变体（各绑定按变体穷尽匹配）：未声明事件归入名称错误，载荷错误归入 JSON 错误。
        CoreError::UnknownEvent(n) => NativeError::InvalidName(format!("{n}（未声明的事件，请先 declare_event）")),
        CoreError::InvalidEventPayload(m) => NativeError::InvalidJson(format!("事件载荷无效：{m}")),
        CoreError::InvalidImplements(m) => NativeError::InvalidName(m),
        CoreError::InvalidCache(m) | CoreError::InvalidDeprecation(m) => NativeError::InvalidConfig(m),
    }
}

/// 解析 inputSchema 文本；`None` 表示无参数。`type: object` 由核心校验。
pub(crate) fn parse_output_schema(text: Option<&str>) -> Result<Option<Value>, NativeError> {
    text.map(|t| {
        serde_json::from_str(t).map_err(|e| NativeError::InvalidSchema(format!("outputSchema 不是合法 JSON：{e}")))
    })
    .transpose()
}

/// [`ToolSpec`] 对应的整体更新（不含 [`ToolOptions`] 的字段）。
pub(crate) fn spec_update(spec: ToolSpec) -> Result<ToolUpdate, NativeError> {
    let input_schema = parse_schema(spec.input_schema_json.as_deref())?;
    Ok(ToolUpdate {
        description: Some(spec.description),
        input_schema: Some(input_schema),
        risk: Some(spec.risk),
        activation: Some(spec.activation),
        title: Some(spec.title),
        enabled: Some(spec.enabled),
        ..ToolUpdate::default()
    })
}

pub(crate) fn parse_schema(text: Option<&str>) -> Result<Value, NativeError> {
    match text {
        None => Ok(json!({ "type": "object", "properties": {} })),
        Some(t) => serde_json::from_str(t)
            .map_err(|e| NativeError::InvalidSchema(format!("不是合法 JSON：{e}"))),
    }
}

pub(crate) fn state_info(state: &ConnectionState, now: Millis) -> StateInfo {
    let (status, retry_in_ms) = match state {
        ConnectionState::Idle => (StateStatus::Idle, None),
        ConnectionState::Connecting => (StateStatus::Connecting, None),
        ConnectionState::Handshaking => (StateStatus::Handshaking, None),
        ConnectionState::PendingPairing => (StateStatus::PendingPairing, None),
        ConnectionState::Connected => (StateStatus::Connected, None),
        ConnectionState::Backoff { retry_at, .. } => (StateStatus::Backoff, Some(retry_at.saturating_sub(now))),
        ConnectionState::Rejected { .. } => (StateStatus::Rejected, None),
        ConnectionState::Stopped => (StateStatus::Stopped, None),
        ConnectionState::Dormant => (StateStatus::Dormant, None),
        ConnectionState::Waking => (StateStatus::Waking, None),
        ConnectionState::HostMismatch { .. } => (StateStatus::HostMismatch, None),
    };
    StateInfo {
        status,
        retry_in_ms,
        reason: state.reason().map(str::to_owned),
        code: state.code().map(|c| c.as_str().to_owned()),
    }
}

/// 连接期间的日志前缀 `[连接 ID] `（spec/protocol.md 10.3）。
pub(crate) fn cid_prefix(client: &app_mcp_core::Client) -> String {
    client.connection_id().map(|c| format!("[{c}] ")).unwrap_or_default()
}

pub(crate) fn cancel_reason(r: app_mcp_core::CancelReason) -> CancelReason {
    match r {
        app_mcp_core::CancelReason::Requested => CancelReason::Requested,
        app_mcp_core::CancelReason::Timeout => CancelReason::Timeout,
        app_mcp_core::CancelReason::Disconnected => CancelReason::Disconnected,
        app_mcp_core::CancelReason::Stopped => CancelReason::Stopped,
    }
}
