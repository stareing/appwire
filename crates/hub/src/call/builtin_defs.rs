//! 内置工具的定义（MCP 形式与 Hub API 形式）。

use std::sync::Arc;

use app_mcp_protocol::Activation;
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use crate::mcp_convert;
use crate::types::{Availability, HubTool};
use crate::names::{
    BUILTIN_APP_ID, TOOL_APPS_ACTIVATE, TOOL_APPS_LIST, TOOL_APPS_NAVIGATE, TOOL_APPS_OVERVIEW, TOOL_APPS_PAGE,
    TOOL_APPS_RELEASE, TOOL_APPS_SELECT, TOOL_APPS_TASK_BEGIN, TOOL_APPS_TASK_END, TOOL_APPS_TOOLS,
};
use crate::names::{ARG_TASK_ID, TASK_SCOPED_TOOLS};

use super::tool_convert::upstream_risk;

pub(super) fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// 内置工具（MCP 形式）。`with_apps_tools`：是否包含 `apps.tools`（只在渐进暴露生效时列出）；`with_apps_page`：是否包含
/// `apps.page` 与 `apps.navigate`（只在有页面目录时列出）。不列出时也都可调用。`with_tasks`：是否包含 `apps.task.*` 与各工具的
/// `taskId` 参数（只对可用任务句柄的无会话请求列出，[`HubShared::task_handles_for`]）；不列出时 legacy 会话与 Hub API 的定义与
/// 句柄出现之前逐字节相同。
pub(crate) fn builtin_tools(with_apps_tools: bool, with_apps_page: bool, with_tasks: bool) -> Vec<Tool> {
    let mut tools = all_builtin_tools();
    let page_tool = |n: &str| n == TOOL_APPS_PAGE || n == TOOL_APPS_NAVIGATE;
    let task_tool = |n: &str| n == TOOL_APPS_TASK_BEGIN || n == TOOL_APPS_TASK_END;
    tools.retain(|t| {
        (with_apps_tools || t.name != TOOL_APPS_TOOLS) && (with_apps_page || !page_tool(&t.name)) && (with_tasks || !task_tool(&t.name))
    });
    if !with_tasks {
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
             互不影响。任务空闲一段时间后自动回收，用完可调用 apps.task.end。",
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
    ]
}

pub(super) fn builtin_schema(name: &str) -> Option<Value> {
    all_builtin_tools()
        .into_iter()
        .find(|t| t.name == name)
        .map(|t| Value::Object((*t.input_schema).clone()))
}

pub(crate) fn builtin_hub_tools(with_apps_tools: bool, with_apps_page: bool, with_tasks: bool) -> Vec<HubTool> {
    builtin_tools(with_apps_tools, with_apps_page, with_tasks)
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
            }
        })
        .collect()
}
