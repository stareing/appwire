//! 内置工具的定义（MCP 形式与 Hub API 形式）。

use std::sync::Arc;

use app_mcp_protocol::Activation;
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use crate::mcp_convert;
use crate::types::{Availability, HubTool};
use crate::names::{
    BUILTIN_APP_ID, TOOL_APPS_ACTIVATE, TOOL_APPS_LIST, TOOL_APPS_LOCK, TOOL_APPS_NAVIGATE, TOOL_APPS_OVERVIEW,
    TOOL_APPS_PAGE, TOOL_APPS_RELEASE, TOOL_APPS_SELECT, TOOL_APPS_TASK_BEGIN, TOOL_APPS_TASK_END, TOOL_APPS_TOOLS,
    TOOL_APPS_UNLOCK, TOOL_APPS_CALLS, TOOL_APPS_CANCEL, TOOL_APPS_EVENTS, TOOL_APPS_EVENTS_SUBSCRIBE,
    TOOL_APPS_EVENTS_UNSUBSCRIBE, TOOL_APPS_SEARCH, TOOL_APPS_INTENTS,
};
use crate::intents::MAX_INTENT_ARG_CHARS;
use crate::events::{MAX_EVENTS_PER_FETCH};
use crate::search::{DEFAULT_SEARCH_LIMIT, MAX_QUERY_CHARS, MAX_SEARCH_LIMIT};
use crate::object_lock::{MAX_LOCK_KEY_LEN, MAX_LOCK_TTL_MS, MIN_LOCK_TTL_MS};
use crate::names::{ARG_TASK_ID, TASK_SCOPED_TOOLS};

use super::tool_convert::upstream_risk;

pub(super) fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// 列出哪些可选的内置工具（不列出的除 `apps.lock` / `apps.unlock` 在 `locks` 关闭时外，也都可调用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BuiltinSet {
    /// `apps.tools`：只在渐进暴露生效时列出。
    pub apps_tools: bool,
    /// `apps.page` 与 `apps.navigate`：只在有页面目录时列出。
    pub apps_page: bool,
    /// `apps.task.*` 与各工具的 `taskId` 参数：只对可用任务句柄的无会话请求列出（[`HubShared::task_handles_for`]）；不列出时
    /// legacy 会话与 Hub API 的定义与句柄出现之前逐字节相同。
    pub tasks: bool,
    /// `apps.lock` / `apps.unlock`：启用对象锁时列出（[`HubShared::locks_enabled`]）。
    pub locks: bool,
}

/// 内置工具（MCP 形式），按 [`BuiltinSet`] 取舍。
pub(crate) fn builtin_tools(set: BuiltinSet) -> Vec<Tool> {
    let mut tools = all_builtin_tools();
    let page_tool = |n: &str| n == TOOL_APPS_PAGE || n == TOOL_APPS_NAVIGATE;
    let task_tool = |n: &str| n == TOOL_APPS_TASK_BEGIN || n == TOOL_APPS_TASK_END;
    let lock_tool = |n: &str| n == TOOL_APPS_LOCK || n == TOOL_APPS_UNLOCK;
    tools.retain(|t| {
        (set.apps_tools || t.name != TOOL_APPS_TOOLS)
            && (set.apps_page || !page_tool(&t.name))
            && (set.tasks || !task_tool(&t.name))
            && (set.locks || !lock_tool(&t.name))
    });
    if !set.tasks {
        for t in &mut tools {
            let mut schema = (*t.input_schema).clone();
            if let Some(Value::Object(props)) = schema.get_mut("properties")
                && props.remove(ARG_TASK_ID).is_some()
            {
                t.input_schema = Arc::new(schema);
            }
        }
    }
    tools
}

/// 全部内置工具；[`TASK_SCOPED_TOOLS`] 的 inputSchema 带可选 `taskId`（`apps.task.end` 中必填）。
fn all_builtin_tools() -> Vec<Tool> {
    let mut tools = base_builtin_tools();
    for t in tools.iter_mut().filter(|t| TASK_SCOPED_TOOLS.contains(&&*t.name)) {
        let mut schema = (*t.input_schema).clone();
        if let Some(Value::Object(props)) = schema.get_mut("properties") {
            props.insert(
                ARG_TASK_ID.to_owned(),
                json!({
                    "type": "string",
                    "description": "任务句柄（apps.task.begin 返回）：在该任务中操作，实例选择与租约与其他任务互不影响。省略 = 不带任务的默认调用方"
                }),
            );
        }
        t.input_schema = Arc::new(schema);
    }
    tools
}

fn base_builtin_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            TOOL_APPS_LIST,
            "列出本机已知的 App（静态清单与已连接实例）：appId、名称、简介、是否已连接、各实例的标题 / 地址 / \
             可见性 / 焦点 / 注册的工具、休眠中的实例（dormantInstances，调用其工具时会自动唤醒），以及当前会话选定的实例。",
            obj(json!({ "type": "object", "properties": {}, "additionalProperties": false })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_SELECT,
            "在当前会话中指定某个 App 的目标实例（例如用户开了多个标签页时）。之后对该 App 的调用优先路由到此实例\
             （仅当它注册了被调用的工具）。instanceId 可从 apps.list 获取。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "instanceId": { "type": "string", "description": "实例 ID（见 apps.list）" }
                },
                "required": ["appId", "instanceId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_OVERVIEW,
            "查看某个 App 的完整总览（能力范围、典型流程、不支持的操作等）。",
            obj(json!({
                "type": "object",
                "properties": { "appId": { "type": "string", "description": "App 标识" } },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_TOOLS,
            "列出某个 App 的全部工具（全名、说明、参数 inputSchema、风险、可用性）与页面目录摘要（pages）。工具较多时工具列表\
             只含 apps.* 与本会话用过的 App；调用本工具后该 App 的工具会加入本会话的工具列表，也可以直接按全名 \
             <appId>.<工具名> 调用。不在当前页面的工具用 apps.page 查看。appId 可从 apps.list 获取。",
            obj(json!({
                "type": "object",
                "properties": { "appId": { "type": "string", "description": "App 标识" } },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_PAGE,
            "查看某个 App 某个页面上的工具（全名、说明、参数 inputSchema）。页面名见 apps.tools 的 pages。这些工具可以直接\
             按全名调用：不在当前页面时 Hub 会先让 App 切换到该页面再执行（会改变用户看到的界面；App 可以拒绝）。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "page": { "type": "string", "description": "页面名（见 apps.tools 的 pages）" }
                },
                "required": ["appId", "page"],
                "additionalProperties": false
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            TOOL_APPS_NAVIGATE,
            "让 App 打开某个页面（会改变用户看到的界面；App 可以拒绝，在后台时可能需要用户先切到 App）。params 为页面参数\
             （格式见 apps.page 返回的 page.params）。App 未运行时先唤醒。调用不在当前页面的工具时 Hub 会自动导航，\
             只在需要页面参数或只想打开页面时使用本工具。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "page": { "type": "string", "description": "页面名（见 apps.tools 的 pages）" },
                    "params": { "type": "object", "description": "页面参数（按该页面的 params schema）" }
                },
                "required": ["appId", "page"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_ACTIVATE,
            "预先唤醒 App（不调用任何工具），例如即将连续使用它时。App 已在运行则不做任何事。返回实例与是否唤醒。\
             不再需要时可调用 apps.release。",
            obj(json!({
                "type": "object",
                "properties": { "appId": { "type": "string", "description": "App 标识" } },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_RELEASE,
            "告诉 Hub 本会话暂时不再使用某个 App：收回本会话对它的保活（租约），App 之后可按自己的设置休眠以节省资源。\
             不影响其他会话；之后再调用其工具时会照常唤醒。",
            obj(json!({
                "type": "object",
                "properties": { "appId": { "type": "string", "description": "App 标识" } },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_TASK_BEGIN,
            "开始一个独立的任务，返回任务句柄 taskId。同时进行多件互不相关的事（如分别操作同一 App 的两个实例）时，在 apps.select / \
             apps.list / apps.activate / apps.release / apps.navigate 的参数中带各自的 taskId，实例选择与保活（租约）按任务分开、\
             互不影响；apps.lock 加的锁也归该任务。任务空闲一段时间后自动回收，用完可调用 apps.task.end。",
            obj(json!({ "type": "object", "properties": {}, "additionalProperties": false })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(false).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_TASK_END,
            "结束 apps.task.begin 开始的任务：收回它对 App 的保活（租约）并清除它的实例选择，句柄随后失效。任务不存在时什么也不做。",
            obj(json!({
                "type": "object",
                "properties": {},
                "required": [ARG_TASK_ID],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_LOCK,
            "在一段连续操作期间锁定某个 App，避免其他 Agent 同时修改：锁定后其他 Agent 对该 App 的写操作会收到 LOCKED 错误\
             （只读工具不受影响；用户在 App 里的直接操作不受影响）。带 key 时只锁 App 内的一个对象（如文档 ID），只与其他 Agent \
             对同一对象加锁互斥、不拦截调用。锁在 ttlMs（默认 60000）后到期，再次调用即续期；用完请调用 apps.unlock。\
             已被其他 Agent 锁定时返回 LOCKED（含剩余时间 retryAfterMs）。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "key": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_LOCK_KEY_LEN,
                        "description": "App 内对象的名字（省略 = 锁整个 App 的写操作）"
                    },
                    "ttlMs": {
                        "type": "integer",
                        "minimum": MIN_LOCK_TTL_MS,
                        "maximum": MAX_LOCK_TTL_MS,
                        "description": "有效期（毫秒），默认 60000"
                    }
                },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_UNLOCK,
            "释放 apps.lock 加的锁（appId 与 key 与加锁时相同）。只能释放自己的锁；没有持有时什么也不做。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "key": { "type": "string", "description": "加锁时的 key（锁整个 App 时省略）" }
                },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_CALLS,
            "列出你自己（含你的任务句柄）进行中的调用：callId、工具、阶段（created 已受理 / approving 等待用户确认 / activating \
             唤醒或打开页面中 / running 执行中）、已耗时与最近进度。调用结束即不再列出（结果只交给发起方）。",
            obj(json!({ "type": "object", "properties": {}, "additionalProperties": false })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_CANCEL,
            "取消你自己（含你的任务句柄）的一个进行中调用（callId 见 apps.calls）；发起方会收到 CANCELLED。已在 App 内开始的操作\
             是否回滚由 App 决定。",
            obj(json!({
                "type": "object",
                "properties": {
                    "callId": { "type": "string", "minLength": 1, "description": "要取消的调用（apps.calls 列出的 callId）" }
                },
                "required": ["callId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_EVENTS_SUBSCRIBE,
            "订阅某个 App 的事件（如订单已发货、下载完成）。事件到达后放进你的信箱，用 apps.events 取出；Hub 只投递事件，\
             不会替你调用工具。事件目录见 apps.tools 的 events。event 省略 = 该 App 的全部事件；filter 为对象时只收载荷顶层字段与之\
             逐一相等的事件。相同的订阅重复调用返回已有订阅。",
            obj(json!({
                "type": "object",
                "properties": {
                    "appId": { "type": "string", "description": "App 标识" },
                    "event": { "type": "string", "description": "事件名（见 apps.tools 的 events）；省略 = 全部事件" },
                    "filter": { "type": "object", "description": "载荷顶层字段 → 期望值（只做相等匹配）" }
                },
                "required": ["appId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_EVENTS_UNSUBSCRIBE,
            "退订你自己的一个事件订阅（subscriptionId 见 apps.events.subscribe 或 apps.events 的 subscriptions）。信箱中已有的事件\
             仍可取出。",
            obj(json!({
                "type": "object",
                "properties": {
                    "subscriptionId": { "type": "string", "minLength": 1, "description": "要退订的订阅" }
                },
                "required": ["subscriptionId"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_EVENTS,
            "从你的信箱按到达顺序取出事件（取出即移出），并列出你的订阅。dropped 为上次取件以来因信箱已满或超出频率上限而丢弃的\
             事件数。只读不移出可读取资源 app-mcp://apps/events。",
            obj(json!({
                "type": "object",
                "properties": {
                    "max": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_EVENTS_PER_FETCH,
                        "description": "最多取出的条数，默认 100"
                    }
                },
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(false).destructive(false).idempotent(false).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_SEARCH,
            "按要做的事检索全部 App 的工具（含未列出的、休眠 App 的与不在当前页面的），返回最相关的工具及其参数 inputSchema，\
             可直接按全名调用。query 用关键词描述要做的事（中英文皆可，如「导出订单」「send email」）；appId 只在某个 App 中找。\
             不会启动 App。",
            obj(json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_QUERY_CHARS,
                        "description": "要做的事（关键词）"
                    },
                    "appId": { "type": "string", "description": "只在该 App 中检索（省略 = 全部 App）" },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_SEARCH_LIMIT,
                        "description": format!("最多返回的条数，默认 {DEFAULT_SEARCH_LIMIT}")
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false),
        ),
        Tool::new(
            TOOL_APPS_INTENTS,
            "按通用动作（标准意图，如 message.send 发消息、calendar.create 建日程、media.play 播放、file.share 分享文件、\
             link.open 打开链接、navigation.start 导航）列出声明实现它的工具，供你选择用哪个 App。default 为机主设置的默认 App\
             （只是提示）；选定后按工具全名调用。intent 省略 = 列出全部；可写 message.send（任意版本）或 message.send@1。\
             不会启动 App。",
            obj(json!({
                "type": "object",
                "properties": {
                    "intent": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_INTENT_ARG_CHARS,
                        "description": "标准意图，如 message.send 或 message.send@1（省略 = 全部）"
                    }
                },
                "additionalProperties": false
            })),
        )
        .with_annotations(
            ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false),
        ),
    ]
}

pub(super) fn builtin_schema(name: &str) -> Option<Value> {
    all_builtin_tools()
        .into_iter()
        .find(|t| t.name == name)
        .map(|t| Value::Object((*t.input_schema).clone()))
}

pub(crate) fn builtin_hub_tools(set: BuiltinSet) -> Vec<HubTool> {
    builtin_tools(set)
        .into_iter()
        .map(|t| {
            let name = t.name.to_string();
            HubTool {
                tool: name
                    .strip_prefix("apps.")
                    .unwrap_or(&name)
                    .to_owned(),
                name,
                app_id: BUILTIN_APP_ID.to_owned(),
                title: None,
                description: t.description.as_deref().unwrap_or_default().to_owned(),
                input_schema: Value::Object((*t.input_schema).clone()),
                // 只读的为 read，其余（apps.navigate / activate / release）为 write，与按注解推导的规则相同。
                risk: upstream_risk(&t),
                activation: Activation::Headless,
                availability: Availability::Available,
                annotations: t
                    .annotations
                    .as_ref()
                    .map(mcp_convert::from_mcp_tool_annotations)
                    .unwrap_or_default(),
                output_schema: None,
                surface: None,
                page: None,
                implements: Vec::new(),
                schema_hash: None,
                deprecated: None,
            }
        })
        .collect()
}
