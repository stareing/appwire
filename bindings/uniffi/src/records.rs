//! 工具 / 资源 / 事件声明与调用结果等记录：[`ToolSpec`]、[`ResourceSpec`]、[`EventInfo`]、[`CallResult`]、[`StateInfo`] 等。

use app_mcp_native as native;

use crate::enums::{Activation, Audience, ResultStatus, Risk, StateStatus, ToolSurface};
use crate::error::AppMcpError;

/// 工具定义。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ToolSpec {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    /// JSON Schema 文本，`type` 必须为 `"object"`；为空表示无参数。
    #[uniffi(default = None)]
    pub input_schema_json: Option<String>,
    /// 旧写法（优先用 `annotations`）。为空时为 `Write`。
    #[uniffi(default = None)]
    pub risk: Option<Risk>,
    #[uniffi(default = None)]
    pub activation: Option<Activation>,
    #[uniffi(default = None)]
    pub title: Option<String>,
    #[uniffi(default = true)]
    pub enabled: bool,
    /// 标准 MCP 工具注解，原样转发给 Agent；为空 = 未声明（Hub 按 `risk` 推导）。
    #[uniffi(default = None)]
    pub annotations: Option<ToolAnnotations>,
    /// 结果的 JSON Schema 文本（MCP `outputSchema`）；为空 = 未声明。
    #[uniffi(default = None)]
    pub output_schema_json: Option<String>,
    /// 对界面的依赖（spec/protocol.md 3.4）；为空 = `App`。
    #[uniffi(default = None)]
    pub surface: Option<ToolSurface>,
    /// 所在页面名；Hub 在该工具未注册时据此导航（[`AppMcpClient::set_navigation_handler`]）。
    #[uniffi(default = None)]
    pub page: Option<String>,
    /// 后台替身（只对 `View` 工具有意义）：同 App 内一个 `App` 工具的名称；App 在后台、本工具不可调用时
    /// Hub 改调该工具（spec/protocol.md 3.4「后台与前台」）。为空 = 未声明。
    #[uniffi(default = None)]
    pub background_tool: Option<String>,
    /// 本工具同时执行的调用上限（spec/protocol.md 5.3）：0 = 不单独限制（只受 `max_concurrent_calls` 约束）。只在 SDK 内生效。
    #[uniffi(default = 0)]
    pub concurrency: u32,
    /// 互斥组（spec/protocol.md 5.3，`[a-zA-Z0-9_.-]{1,64}`）：同组的工具同一时刻至多一个在执行。为空 = 不互斥。
    #[uniffi(default = None)]
    pub exclusive: Option<String>,
    /// 实现的标准意图（spec/intents.md），如 `["message.send@1"]`。
    #[uniffi(default = [])]
    pub implements: Vec<String>,
}

/// 标准 MCP 工具注解（spec/protocol.md 第 3 节）。均可选，为空 = 未声明。
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct ToolAnnotations {
    /// 给人看的工具标题。
    #[uniffi(default = None)]
    pub title: Option<String>,
    /// 不修改任何状态。
    #[uniffi(default = None)]
    pub read_only_hint: Option<bool>,
    /// 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。
    #[uniffi(default = None)]
    pub destructive_hint: Option<bool>,
    /// 以相同参数重复调用没有额外效果（只在非只读时有意义）。
    #[uniffi(default = None)]
    pub idempotent_hint: Option<bool>,
    /// 会与外部世界交互（网络、第三方、其他用户可见）。
    #[uniffi(default = None)]
    pub open_world_hint: Option<bool>,
}

impl From<ToolAnnotations> for native::ToolAnnotations {
    fn from(a: ToolAnnotations) -> Self {
        native::ToolAnnotations {
            title: a.title,
            read_only_hint: a.read_only_hint,
            destructive_hint: a.destructive_hint,
            idempotent_hint: a.idempotent_hint,
            open_world_hint: a.open_world_hint,
        }
    }
}

/// @compat 原生层把注解与 outputSchema 放在 `ToolOptions`；这里拆成两部分，注册与更新都整体传入。
impl From<ToolSpec> for (native::ToolSpec, native::ToolOptions) {
    fn from(s: ToolSpec) -> Self {
        let mut n = native::ToolSpec::new(s.name, s.description);
        n.input_schema_json = s.input_schema_json;
        if let Some(risk) = s.risk {
            n.risk = risk.into();
        }
        n.activation = s.activation.map(Into::into);
        n.title = s.title;
        n.enabled = s.enabled;
        let options = native::ToolOptions {
            annotations: s.annotations.map(Into::into),
            output_schema_json: s.output_schema_json,
            surface: s.surface.map(Into::into).unwrap_or_default(),
            page: s.page,
            background_tool: s.background_tool,
            concurrency: s.concurrency,
            exclusive: s.exclusive,
            implements: s.implements,
        };
        (n, options)
    }
}

/// 结果内容的标注（MCP 内容注解），Hub 原样转发，不据此做判断。
#[derive(Clone, Debug, Default, PartialEq, uniffi::Record)]
pub struct ContentAnnotations {
    /// 内容面向谁。
    #[uniffi(default = None)]
    pub audience: Option<Vec<Audience>>,
    /// 重要程度，0（可选）到 1（必需）。
    #[uniffi(default = None)]
    pub priority: Option<f64>,
    /// 最后修改时刻（ISO 8601）。
    #[uniffi(default = None)]
    pub last_modified: Option<String>,
}

impl From<ContentAnnotations> for native::ContentAnnotations {
    fn from(a: ContentAnnotations) -> Self {
        native::ContentAnnotations {
            audience: a.audience.map(|v| v.into_iter().map(Into::into).collect()),
            priority: a.priority,
            last_modified: a.last_modified,
        }
    }
}

/// 调用成功的完整结果（[`Call::complete_with`]，spec/protocol.md 3.2）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CallResult {
    /// 返回值 JSON 文本；为空表示无返回值（`null`，Hub 对模型输出"已完成"）。
    #[uniffi(default = None)]
    pub data_json: Option<String>,
    /// 调用后内容可能已变化的资源名。
    #[uniffi(default = [])]
    pub state_hints: Vec<String>,
    /// 业务状态；一般为 `Done`。
    pub status: ResultStatus,
    /// `Pending` 时可读取后续状态的资源名。
    #[uniffi(default = None)]
    pub state_resource: Option<String>,
    /// 一句面向模型 / 用户的结论（`Partial` 时说明完成了哪部分）。
    #[uniffi(default = None)]
    pub summary: Option<String>,
    /// 结果内容的标注。
    #[uniffi(default = None)]
    pub annotations: Option<ContentAnnotations>,
}

impl From<CallResult> for native::CallResult {
    fn from(r: CallResult) -> Self {
        native::CallResult {
            data_json: r.data_json,
            state_hints: r.state_hints,
            status: r.status.into(),
            state_resource: r.state_resource,
            summary: r.summary,
            annotations: r.annotations.map(Into::into),
        }
    }
}

/// 资源定义。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ResourceSpec {
    pub name: String,
    pub description: String,
    #[uniffi(default = None)]
    pub mime_type: Option<String>,
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被订阅时保持连接、休眠中变化时回连推送。
    /// 默认 `false`：订阅不阻止休眠，变化在下次连接时补发。
    #[uniffi(default = false)]
    pub realtime: bool,
    /// 资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上；为空表示未声明。
    #[uniffi(default = None)]
    pub annotations: Option<ContentAnnotations>,
}

impl From<ResourceSpec> for (native::ResourceSpec, native::ResourceOptions) {
    fn from(s: ResourceSpec) -> Self {
        let spec = native::ResourceSpec { name: s.name, description: s.description, mime_type: s.mime_type };
        let options = native::ResourceOptions { realtime: s.realtime, annotations: s.annotations.map(Into::into) };
        (spec, options)
    }
}

/// 连接状态信息。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct StateInfo {
    pub status: StateStatus,
    /// `Backoff` 时距下一次重连的毫秒数。
    pub retry_in_ms: Option<u64>,
    /// `Rejected` / `HostMismatch` 时的原因；`Backoff` 时为连接失败 / 断开的原因（有的话）。
    pub reason: Option<String>,
    /// 与 `reason` 对应的错误码（spec/protocol.md 10.1，如 `HOST_NOT_RUNNING`）。
    pub code: Option<String>,
}

impl From<native::StateInfo> for StateInfo {
    fn from(s: native::StateInfo) -> Self {
        StateInfo {
            status: s.status.into(),
            retry_in_ms: s.retry_in_ms,
            reason: s.reason,
            code: s.code,
        }
    }
}

/// 事件声明（spec/protocol.md 3.5，[`AppMcpClient::declare_event`]）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct EventInfo {
    /// 事件名，规则同工具名（`[a-zA-Z0-9_.-]{1,64}`），如 `order.shipped`。
    pub name: String,
    /// 面向模型：事件何时发生、载荷含义。
    pub description: String,
    /// 载荷的 JSON Schema 文本（描述用，Hub 不校验）；为空 = 无载荷或不描述。
    #[uniffi(default = None)]
    pub payload_schema_json: Option<String>,
}

/// @error `payload_schema_json` 不是合法 JSON → [`AppMcpError::InvalidJson`]。
impl TryFrom<EventInfo> for native::EventInfo {
    type Error = AppMcpError;

    fn try_from(e: EventInfo) -> Result<Self, AppMcpError> {
        let payload_schema = e
            .payload_schema_json
            .map(|text| {
                serde_json::from_str(&text)
                    .map_err(|err| AppMcpError::InvalidJson { detail: format!("事件 payloadSchema 不是合法 JSON：{err}") })
            })
            .transpose()?;
        Ok(native::EventInfo { name: e.name, description: e.description, payload_schema })
    }
}
