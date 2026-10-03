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
/// 签发一个 Agent 任务句柄（spec/hub-api.md 3.6「任务句柄」，第 12 项 S8）。
pub const TOOL_APPS_TASK_BEGIN: &str = "apps.task.begin";
/// 结束一个 Agent 任务句柄：收回其租约、清除其选择（spec/hub-api.md 3.6「任务句柄」）。
pub const TOOL_APPS_TASK_END: &str = "apps.task.end";
/// 加锁 / 续期（spec/hub-api.md 3.6「对象锁」，第 16 项 N6）。
pub const TOOL_APPS_LOCK: &str = "apps.lock";
/// 解锁（spec/hub-api.md 3.6「对象锁」）。
pub const TOOL_APPS_UNLOCK: &str = "apps.unlock";
/// 列出自己的进行中调用（spec/hub-api.md 3.6「调用对象」，第 16 项 P5）。
pub const TOOL_APPS_CALLS: &str = "apps.calls";
/// 取消自己的进行中调用（spec/hub-api.md 3.6「调用对象」）。
pub const TOOL_APPS_CANCEL: &str = "apps.cancel";

/// Hub 自身状态的只读资源名（第 16 项 P7；URI `app-mcp://apps/<名>`，spec/hub-api.md 3.6「Hub 状态资源」）：
/// App 概况与对象锁。
pub const RESOURCE_HUB: &str = "hub";
/// 读取方自己的任务、锁、用量与配额余量。
pub const RESOURCE_SELF: &str = "self";

/// 任务句柄的工具参数名（与 [`META_TASK_ID`] 等价）。
pub const ARG_TASK_ID: &str = "taskId";
/// 接受 [`ARG_TASK_ID`] 参数的内置工具（持有按任务区分的状态：选择、租约、锁、进行中的调用；[`TOOL_APPS_TASK_END`] 中为必填）。
pub const TASK_SCOPED_TOOLS: [&str; 10] = [
    TOOL_APPS_LIST,
    TOOL_APPS_SELECT,
    TOOL_APPS_NAVIGATE,
    TOOL_APPS_ACTIVATE,
    TOOL_APPS_RELEASE,
    TOOL_APPS_TASK_END,
    TOOL_APPS_LOCK,
    TOOL_APPS_UNLOCK,
    TOOL_APPS_CALLS,
    TOOL_APPS_CANCEL,
];

// ---- MCP 结果 `_meta`（Hub → Agent）----

/// 结果状态（`status` 不是 `done` 时写入，spec/protocol.md 3.2）。
pub const META_STATUS: &str = "dev.appwire/status";
/// `pending` 时可读取后续状态的资源 URI。
pub const META_STATE_RESOURCE: &str = "dev.appwire/stateResource";
/// 改调了后台替代时实际调用的工具全名（spec/hub-api.md 3.14）。
pub const META_ROUTED_TO: &str = "dev.appwire/routedTo";
/// 本次调用的 callId（每个工具调用结果都带）：与转交 App 的 `ToolsInvokeParams.callId`、App handler 看到的 callId、
/// Hub 日志「转发工具调用」记录的 `call_id` 字段相同。
pub const META_CALL_ID: &str = "dev.appwire/callId";
/// 实际处理调用的 App 实例（App 工具路由到实例时带）。
pub const META_INSTANCE_ID: &str = "dev.appwire/instanceId";
/// Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App；每个工具调用结果都带）。
pub const META_DURATION_MS: &str = "dev.appwire/durationMs";
/// 本次 App 工具调用是否经历了唤醒（调用时目标未连接，唤醒回连后才送达；只在 App 工具结果中出现）。
pub const META_WOKE: &str = "dev.appwire/woke";

// ---- MCP 请求 `_meta`（Agent → Hub，`tools/call`）----

/// Agent 的截止时间：从 Hub 收到请求起还愿意等待的毫秒数（正整数）。Hub 取它与配置的 `response_timeout` 的较小者。
pub const META_TIMEOUT_MS: &str = "dev.appwire/timeoutMs";
/// Agent 的幂等键：原样进入 `ToolsInvokeParams.idempotencyKey`（spec/protocol.md 3.3）。
pub const META_IDEMPOTENCY_KEY: &str = "dev.appwire/idempotencyKey";
/// Agent 给出的调用优先级（第 16 项 P6）：`interactive` / `normal` / `background`，进入 `ToolsInvokeParams.priority`。
pub const META_PRIORITY: &str = "dev.appwire/priority";

/// 可选的任务句柄通道：与工具参数 [`ARG_TASK_ID`] 等价，且对任何工具调用（含 App 工具）生效
/// （供自己实现客户端的 Agent 宿主；模型写不进 `_meta`，见 spec/hub-api.md 3.6「任务句柄」）。
pub const META_TASK_ID: &str = "dev.appwire/taskId";

/// 弃用期内仍接受的旧请求键（[`META_TIMEOUT_MS`] 的旧名；与新键同时出现且取值不同时显式失败）。
pub const LEGACY_META_TIMEOUT_MS: &str = "app-mcp/timeoutMs";
/// 弃用期内仍接受的旧请求键（[`META_IDEMPOTENCY_KEY`] 的旧名；规则同 [`LEGACY_META_TIMEOUT_MS`]）。
pub const LEGACY_META_IDEMPOTENCY_KEY: &str = "app-mcp/idempotencyKey";
