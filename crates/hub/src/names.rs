//! 对 Agent 可见的名称：内置工具名与 MCP `_meta` 键（唯一定义；规范见 spec/hub-api.md 3.15）。
//!
//! @compat `_meta` 键前缀为反向域名 `dev.appwire/`（MCP 规范 SHOULD，docs/plans/12-mcp-stateless.md 第 4 节、S1）；
//! 旧前缀 `app-mcp/` 的请求键在弃用期内仍接受（[`LEGACY_META_TIMEOUT_MS`]、[`LEGACY_META_IDEMPOTENCY_KEY`]），结果键只写新前缀。

// ---- 内置工具（appId `apps`）----

/// 内置工具的 appId（保留名）。
pub const BUILTIN_APP_ID: &str = "apps";
pub const TOOL_APPS_LIST: &str = "apps.list";
pub const TOOL_APPS_SELECT: &str = "apps.select";
pub const TOOL_APPS_OVERVIEW: &str = "apps.overview";
/// 渐进暴露（spec/hub-api.md 3.7）：查看某个 App 的工具并加入本会话的工具列表。
pub const TOOL_APPS_TOOLS: &str = "apps.tools";
/// 页面渐进披露（spec/hub-api.md 3.14 L3）：查看某个 App 某个页面上的工具。
pub const TOOL_APPS_PAGE: &str = "apps.page";
/// 显式导航（spec/hub-api.md 3.15）：让 App 打开某个页面，可带页面参数。
pub const TOOL_APPS_NAVIGATE: &str = "apps.navigate";
/// 只唤醒不调用（spec/hub-api.md 3.15）。
pub const TOOL_APPS_ACTIVATE: &str = "apps.activate";
/// 收回本会话在某个 App 上的租约（spec/hub-api.md 3.15）。
pub const TOOL_APPS_RELEASE: &str = "apps.release";

// ---- MCP 结果 `_meta`（Hub → Agent）----

/// 结果状态（`status` 不是 `done` 时写入，spec/protocol.md 3.2）。
pub const META_STATUS: &str = "dev.appwire/status";
/// `pending` 时可读取后续状态的资源 URI。
pub const META_STATE_RESOURCE: &str = "dev.appwire/stateResource";
/// 改调了后台替代时实际调用的工具全名（spec/hub-api.md 3.14）。
pub const META_ROUTED_TO: &str = "dev.appwire/routedTo";

// ---- MCP 请求 `_meta`（Agent → Hub，`tools/call`）----

/// Agent 的截止时间：从 Hub 收到请求起还愿意等待的毫秒数（正整数）。Hub 取它与配置的 `response_timeout` 的较小者。
pub const META_TIMEOUT_MS: &str = "dev.appwire/timeoutMs";
/// Agent 的幂等键：原样进入 `ToolsInvokeParams.idempotencyKey`（spec/protocol.md 3.3）。
pub const META_IDEMPOTENCY_KEY: &str = "dev.appwire/idempotencyKey";

/// 弃用期内仍接受的旧请求键（[`META_TIMEOUT_MS`] 的旧名；与新键同时出现且取值不同时显式失败）。
pub const LEGACY_META_TIMEOUT_MS: &str = "app-mcp/timeoutMs";
/// 弃用期内仍接受的旧请求键（[`META_IDEMPOTENCY_KEY`] 的旧名；规则同 [`LEGACY_META_TIMEOUT_MS`]）。
pub const LEGACY_META_IDEMPOTENCY_KEY: &str = "app-mcp/idempotencyKey";
