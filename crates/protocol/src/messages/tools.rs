//! 工具：描述、注解、同步 / 调用 / 取消 / 进度。

use super::*;

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// SDK 登记给 Host 的工具描述。`name` 在 App 内唯一（不含 appId 前缀）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    /// `[a-zA-Z0-9_.-]{1,64}`，如 `cart.checkout`。
    pub name: String,
    pub description: String,
    /// JSON Schema（`type` 必须为 `object`）。
    pub input_schema: Value,
    #[serde(default)]
    pub risk: Risk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 标准 MCP 工具注解，Hub 原样转发给 Agent；与 `risk` 同时存在时逐字段优先（[`ToolInfo::effective_annotations`]）。
    /// 未声明时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    /// 结果的 JSON Schema（MCP `outputSchema`）。根类型不是 `object` 时 Hub 按 MCP 规范包装为 `{ result: <schema> }`
    /// （spec/protocol.md 3.2）。未声明时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// 工具是否依赖界面（spec/protocol.md 3.4）：`app`（缺省）后台可调、可唤醒；`view` 只在所在界面可见且处于最上层时注册。
    /// 缺省时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "ToolSurface::is_app")]
    pub surface: ToolSurface,
    /// 工具所在页面（页面目录的键，[`NavigateParams::page`]）；Hub 据此在工具未注册时先导航再派发。未声明时不序列化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// 后台替代（spec/protocol.md 3.4）：只对 `view` 工具有意义，为同一 App 中一个 `app` 工具的局部名。该工具因 App 在后台
    /// 而不可调用时，Hub 改为调用这个工具（spec/hub-api.md 3.14）。未声明时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tool: Option<String>,
    /// 实现的标准意图（spec/intents.md），每项 `"<动词>@<主版本>"`，如 `message.send@1`；格式见 [`crate::intents`]。
    /// 空时不序列化（`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<String>,
    /// 结果缓存声明（spec/protocol.md 3.6）：只在生效注解 `readOnlyHint` 为 `true` 时由 Hub 执行。未声明时不序列化
    /// （`toolsHash` 不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePolicy>,
}

/// 工具对界面的依赖（spec/protocol.md 3.4）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolSurface {
    /// 不依赖界面：后台可调、可唤醒、进清单与原生意图（缺省，兼容旧 SDK）。
    #[default]
    App,
    /// 依赖界面：只在所在界面真正可见且处于最上层时启用。
    View,
}

impl ToolSurface {
    pub fn is_app(&self) -> bool {
        *self == ToolSurface::App
    }
}

impl ToolInfo {
    /// Agent 看到的注解：声明的字段原样保留，缺少的字段按 `risk` 推导（[`Risk::annotations`]）。
    pub fn effective_annotations(&self) -> ToolAnnotations {
        ToolAnnotations::effective(self.risk, self.annotations.as_ref())
    }
}

/// 标准 MCP 工具注解（MCP `ToolAnnotations`）：App 对工具行为的声明，供 Agent 决定是否确认 / 放行。
/// 本库不据此做任何判断，只原样传递（docs/plans/14-safety.md 第 1 节）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    /// 给人看的工具标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 不修改任何状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// 可能做出破坏性 / 不可撤销的修改（只在非只读时有意义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// 以相同参数重复调用没有额外效果（只在非只读时有意义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// 会与外部世界交互（网络、第三方、其他用户可见）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl ToolAnnotations {
    /// Agent 看到的注解：`declared` 中声明的字段原样保留，缺少的字段按 `risk` 推导（[`Risk::annotations`]）。
    /// [`ToolInfo::effective_annotations`] 与只保存元数据的工具定义（如 Hub 内部）共用这一定义。
    pub fn effective(risk: Risk, declared: Option<&ToolAnnotations>) -> ToolAnnotations {
        let base = risk.annotations();
        match declared {
            None => base,
            Some(a) => ToolAnnotations {
                title: a.title.clone(),
                read_only_hint: a.read_only_hint.or(base.read_only_hint),
                destructive_hint: a.destructive_hint.or(base.destructive_hint),
                idempotent_hint: a.idempotent_hint.or(base.idempotent_hint),
                open_world_hint: a.open_world_hint.or(base.open_world_hint),
            },
        }
    }
}

impl Risk {
    /// 旧写法 `risk` 对应的注解（唯一定义）：`read` → 只读；`destructive` / `payment` → 非只读 + 破坏性；
    /// `write` / `os-sensitive` → 非只读。
    pub fn annotations(self) -> ToolAnnotations {
        let (read_only, destructive) = match self {
            Risk::Read => (true, None),
            Risk::Destructive | Risk::Payment => (false, Some(true)),
            Risk::Write | Risk::OsSensitive => (false, None),
        };
        ToolAnnotations { read_only_hint: Some(read_only), destructive_hint: destructive, ..ToolAnnotations::default() }
    }
}

/// 内容的接收方（MCP `Role`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    User,
    Assistant,
}

/// 标准 MCP 内容注解（MCP `Annotations`）：App 对结果 / 资源内容的标注，Hub 原样转发，不据此做判断。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentAnnotations {
    /// 内容面向谁（`user` / `assistant`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Audience>>,
    /// 重要程度，0（可选）到 1（必需）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    /// 最后修改时刻（ISO 8601）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsSyncParams {
    pub tools: Vec<ToolInfo>,
}

/// 增量变更。同一名称在一条消息中只会出现在 `upserted` 或 `removed` 之一。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsChangedParams {
    #[serde(default)]
    pub upserted: Vec<ToolInfo>,
    #[serde(default)]
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsInvokeParams {
    /// 调用 ID，由 Host 生成，全局唯一。
    pub call_id: String,
    pub name: String,
    /// 已由 Host 按 inputSchema 校验过的参数。
    #[serde(default)]
    pub arguments: Value,
    /// SDK 侧超时（毫秒）。超时后 SDK 取消 handler 并返回 `TIMEOUT`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Agent 给出的幂等键（spec/protocol.md 3.3）：Host 原样转交（1..=[`MAX_IDEMPOTENCY_KEY_LEN`] 个字符）；SDK 去重另按
    /// （工具名, 幂等键）匹配，并在 handler 上下文中提供。省略 = 没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Agent 给出的调用优先级（第 16 项 P6，spec/protocol.md 5.3）：SDK 的调用队列先按优先级、再按到达顺序调度。省略 = `normal`。
    /// @compat 接收方把不认识的取值当作 `normal`（新 Host 加级别时旧 SDK 不拒绝调用）。
    #[serde(default, skip_serializing_if = "CallPriority::is_normal", deserialize_with = "CallPriority::lenient")]
    pub priority: CallPriority,
}

/// 调用优先级（[`ToolsInvokeParams::priority`]）：交互（用户在等结果）> 普通 > 后台。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallPriority {
    Background,
    #[default]
    Normal,
    Interactive,
}

impl CallPriority {
    pub fn is_normal(&self) -> bool {
        *self == Self::Normal
    }

    /// 宽松解析：不认识的取值与非字符串都当作 [`CallPriority::Normal`]（见 [`ToolsInvokeParams::priority`] 的 @compat）。
    fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    /// 严格解析（信任边界上校验 Agent 的取值）：`interactive` / `normal` / `background`，其他为 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "interactive" => Some(Self::Interactive),
            "normal" => Some(Self::Normal),
            "background" => Some(Self::Background),
            _ => None,
        }
    }
}

/// [`ToolsInvokeParams::idempotency_key`] 的长度上限（字符数）：Host 拒绝更长的键（spec/protocol.md 3.3）。
pub const MAX_IDEMPOTENCY_KEY_LEN: usize = 256;

/// 调用结果的业务状态（spec/protocol.md 3.2）。handler 正常返回只说明请求被处理，不一定说明业务已完成。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultStatus {
    /// 已完成（缺省）。
    #[default]
    Done,
    /// 已受理、尚未完成：等待用户在 App 内确认或异步处理；后续状态见 `stateResource`。
    Pending,
    /// 只完成了一部分，说明见 `summary`。
    Partial,
    /// 没有做任何改动（目标状态已满足或无事可做）。
    Noop,
}

impl ResultStatus {
    pub fn is_done(&self) -> bool {
        *self == ResultStatus::Done
    }
}

/// 调用成功的结果。失败通过 JSON-RPC 错误返回（见 [`crate::ErrorKind`]）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsInvokeResult {
    /// handler 返回值；无返回值时为 `null`（Hub 对模型输出"已完成"）。
    #[serde(default)]
    pub data: Value,
    /// 调用后内容可能已变化的资源名，提示模型重新读取。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_hints: Vec<String>,
    /// 结果内容的标注，Hub 原样放到结果内容块上（不含总览与资源变化提示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
    /// 业务状态；缺省 `done`，`done` 时不序列化。
    #[serde(default, skip_serializing_if = "ResultStatus::is_done")]
    pub status: ResultStatus,
    /// `pending` 时：可读取后续状态的 App 资源名（局部名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_resource: Option<String>,
    /// 一句面向模型 / 用户的结论（`partial` 时说明完成了哪部分）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCancelParams {
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `tools/progress`（SDK → Host，通知）：进行中调用的进度（spec/protocol.md 3.3）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsProgressParams {
    /// 进行中调用的 `callId`（[`ToolsInvokeParams::call_id`]）。
    pub call_id: String,
    /// 已完成的量；同一调用内应递增（MCP `notifications/progress` 的要求，Host 丢弃不递增的值）。
    pub progress: f64,
    /// 总量（已知时）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// 一句面向用户的进度说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
