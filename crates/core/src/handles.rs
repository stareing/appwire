//! 句柄与工具 / 资源定义。

use super::*;

// ---------------------------------------------------------------------------
// 句柄与定义
// ---------------------------------------------------------------------------

/// 工具句柄。在一个 [`Client`] 内唯一，不复用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolId(pub u64);

/// 资源句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u64);

/// Scope 句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeId(pub u64);

/// 一次资源读取请求的句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReadId(pub u64);

/// 进行中的导航请求（[`Event::Navigate`]），用 [`Client::complete_navigate`] 完成。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NavigateId(pub u64);

/// 阻止休眠的持有句柄（[`Client::hold`]），用 [`Client::release_hold`] 释放。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HoldId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    /// JSON Schema，`type` 必须为 `"object"`。
    pub input_schema: Value,
    pub risk: Risk,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    /// 标准 MCP 工具注解（spec/protocol.md 第 3 节），原样同步给 Host；`None` = 未声明（按 `risk` 推导）。
    pub annotations: Option<ToolAnnotations>,
    /// 结果的 JSON Schema（MCP `outputSchema`）；`None` = 未声明。
    pub output_schema: Option<Value>,
    /// 对界面的依赖（spec/protocol.md 3.4）：`App`（缺省）或 `View`。只做声明，是否注册由封装层按可见性决定。
    pub surface: ToolSurface,
    /// 所在页面（`[a-zA-Z0-9_.-]{1,64}`）；Hub 在该工具未注册时据此先导航。`None` = 未声明。
    pub page: Option<String>,
    /// 后台替代（spec/protocol.md 3.4）：同一 App 中一个 `app` 工具的局部名；本工具因 App 在后台不可调用时 Hub 改调它。
    /// 只对 `View` 工具有意义。`None` = 未声明。
    pub background_tool: Option<String>,
    /// 实现的标准意图（spec/intents.md），每项 `"<动词>@<主版本>"`，最多 4 项、不重复（格式不合法时注册返回
    /// [`CoreError::InvalidImplements`]）；动词不在词表中或不满足词表必填参数时产生 [`Event::Warning`]。空 = 未声明。
    pub implements: Vec<String>,
    /// 结果缓存声明（spec/protocol.md 3.6），原样同步给 Host；`ttlMs` 越界时注册返回 [`CoreError::InvalidCache`]，
    /// 生效注解不是只读时照常注册并产生 [`Event::Warning`]（Hub 忽略）。`None` = 未声明。
    pub cache: Option<CachePolicy>,
    /// 本工具同时执行的调用上限（spec/protocol.md 5.3）：0 = 不单独限制（只受 `maxConcurrentCalls` 约束）。只在 SDK 内生效，不同步给 Host。
    pub concurrency: u32,
    /// 互斥组（spec/protocol.md 5.3，`[a-zA-Z0-9_.-]{1,64}`）：同组的工具同一时刻至多一个在执行（如操作同一份文档的写工具）。
    /// `None` = 不互斥。只在 SDK 内生效，不同步给 Host。
    pub exclusive: Option<String>,
    /// 为 false 时不同步给 Host（等同于从 Host 的角度看不存在）。
    pub enabled: bool,
    /// 所属 scope；scope 被销毁时工具自动注销。
    pub scope: Option<ScopeId>,
}

/// 对已注册工具的部分更新；`None` 表示不变。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolUpdate {
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub risk: Option<Risk>,
    pub activation: Option<Option<Activation>>,
    pub title: Option<Option<String>>,
    pub enabled: Option<bool>,
    /// `Some(None)` 清除声明的注解。
    pub annotations: Option<Option<ToolAnnotations>>,
    /// `Some(None)` 清除声明的输出 schema。
    pub output_schema: Option<Option<Value>>,
    pub surface: Option<ToolSurface>,
    /// `Some(None)` 清除声明的页面。
    pub page: Option<Option<String>>,
    /// `Some(None)` 清除声明的后台替代。
    pub background_tool: Option<Option<String>>,
    /// 替换声明的标准意图（`Some(vec![])` 清除）。
    pub implements: Option<Vec<String>>,
    /// `Some(None)` 清除结果缓存声明。
    pub cache: Option<Option<CachePolicy>>,
    /// 本工具的并发上限（0 = 不单独限制）。只在 SDK 内生效：只改它不发 `tools/changed`。
    pub concurrency: Option<u32>,
    /// `Some(None)` 清除互斥组。只在 SDK 内生效：只改它不发 `tools/changed`。
    pub exclusive: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceDef {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
    pub scope: Option<ScopeId>,
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被 Host 订阅时阻止休眠，休眠期间变化时回连推送。
    /// `false`（常用）时订阅不阻止休眠，变化在下次连接时补发 `resources/updated`。
    pub realtime: bool,
    /// 资源内容的标注（MCP 内容注解：`audience` / `priority` / `lastModified`），原样同步给 Host，Hub 放到
    /// MCP `resources/list` 的资源注解上；`None` = 未声明（不序列化，`toolsHash` 不变）。
    pub annotations: Option<ContentAnnotations>,
    /// 读取结果缓存声明（spec/protocol.md 3.6），原样同步给 Host；`ttlMs` 越界时注册返回 [`CoreError::InvalidCache`]。
    /// `None` = 未声明（不序列化，`toolsHash` 不变）。
    pub cache: Option<CachePolicy>,
}

/// handler 成功返回的内容。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallOutput {
    pub data: Value,
    /// 调用后内容可能已变化的资源名。
    pub state_hints: Vec<String>,
    /// 结果内容的标注（MCP 内容注解），Host 原样转发。
    pub annotations: Option<ContentAnnotations>,
    /// 业务状态（缺省 `Done`，spec/protocol.md 3.2）。
    pub status: ResultStatus,
    /// `Pending` 时可读取后续状态的资源名。
    pub state_resource: Option<String>,
    /// 一句结论摘要。
    pub summary: Option<String>,
}
