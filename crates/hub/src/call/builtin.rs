//! 内置工具（`apps.*`）的分派。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Value, json};

use crate::hub::HubShared;
use crate::schema::{self, SchemaCheck};
use crate::names::{
    TOOL_APPS_LIST, TOOL_APPS_LOCK, TOOL_APPS_OVERVIEW, TOOL_APPS_PAGE, TOOL_APPS_SELECT, TOOL_APPS_TASK_BEGIN,
    TOOL_APPS_TASK_END, TOOL_APPS_TOOLS, TOOL_APPS_UNLOCK, TOOL_APPS_CALLS, TOOL_APPS_CANCEL,
};

use super::{CallCtx, unknown_app};
use super::builtin_defs::builtin_schema;
use super::results::json_result;

impl HubShared {

    /// 内置工具；不是内置工具时返回 `None`。
    pub(super) fn call_builtin(
        self: &Arc<Self>,
        ctx: &CallCtx,
        name: &str,
        args: &Value,
    ) -> Option<Result<CallToolResult, ToolError>> {
        let key = &ctx.caller;
        let schema = builtin_schema(name)?;
        if let SchemaCheck::Invalid(msg) = schema::check(&schema, args) {
            return Some(Err(ToolError::new(
                ErrorKind::InvalidInput,
                format!("参数不符合工具「{name}」的 inputSchema：{msg}"),
            )));
        }
        let arg = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Some(match name {
            TOOL_APPS_LIST => Ok(json_result(self.apps_json(&self.merged_selection(key)))),
            TOOL_APPS_SELECT => {
                let (app_id, instance_id) = (arg("appId"), arg("instanceId"));
                if self.app_hidden(&app_id) || self.registry().instance(&app_id, &instance_id).is_none() {
                    return Some(Err(ToolError::new(
                        ErrorKind::AppDisconnected,
                        format!(
                            "App「{app_id}」没有已连接的实例「{instance_id}」。可调用 apps.list 查看当前实例。"
                        ),
                    )));
                }
                // 选定前判断是否已列出，选定后该 App 在本会话直接列出。
                let newly_listed = self
                    .exposed_apps(key)
                    .is_some_and(|e| !e.contains(&app_id));
                self.select_for_caller(key, &app_id, &instance_id);
                if newly_listed && let Some(id) = ctx.mcp_session {
                    self.notify_session_tools_changed(id);
                }
                let message = match self.selection_ttl(key) {
                    // 任务句柄（S8）：选择只属于该任务。
                    None if key.is_task_handle() => format!(
                        "在任务 {} 中，之后对 {app_id} 的调用将优先路由到实例 {instance_id}（该实例注册了对应工具且仍连接时）。只影响\
                         出示同一 taskId 的调用，随任务结束或空闲回收清除；工具列表不变。",
                        key.handle_task_id().unwrap_or_default()
                    ),
                    // 主体级选择（S6）：说明作用范围与有效期，工具列表不变。
                    Some(ttl) => format!(
                        "之后对 {app_id} 的调用将优先路由到实例 {instance_id}（该实例注册了对应工具且仍连接时）。该选择对本机所有\
                         无会话的 MCP 客户端生效，{} 秒内未再用于调用即失效（失效后按默认规则路由，可再次调用 apps.select）；\
                         工具列表不变。",
                        ttl.as_secs_f64()
                    ),
                    None if key.is_stateless() => format!(
                        "之后对 {app_id} 的调用将优先路由到实例 {instance_id}（该实例注册了对应工具且仍连接时）。该选择对本机所有\
                         无会话的 MCP 客户端生效；工具列表不变。"
                    ),
                    None => format!("本会话中对 {app_id} 的调用将优先路由到实例 {instance_id}（该实例注册了对应工具且仍连接时）。"),
                };
                Ok(json_result(json!({
                    "appId": app_id,
                    "instanceId": instance_id,
                    "message": message,
                })))
            }
            TOOL_APPS_OVERVIEW => {
                let app_id = arg("appId");
                let known = self.is_upstream(&app_id) || self.registry().has_app(&app_id);
                if !known || self.app_hidden(&app_id) {
                    return Some(Err(unknown_app(&app_id)));
                }
                match self.overview(&app_id) {
                    None => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                        "App「{app_id}」没有提供总览。"
                    ))])),
                    Some(ov) => {
                        let mut r = CallToolResult::success(vec![ContentBlock::text(ov.render())]);
                        r.structured_content = Some(ov.to_json());
                        self.mark_delivered(key, &app_id, &ov.version);
                        Ok(r)
                    }
                }
            }
            TOOL_APPS_TOOLS => {
                let app_id = arg("appId");
                let known = self.is_upstream(&app_id) || self.registry().has_app(&app_id);
                if !known || self.app_hidden(&app_id) {
                    return Some(Err(unknown_app(&app_id)));
                }
                let tools = self.app_tools(&app_id);
                let progressive = self.progressive_for(key);
                self.expose_in_session(ctx, &app_id);
                let message = if tools.is_empty() {
                    format!("App「{app_id}」当前没有工具。")
                } else if progressive && key.is_stateless() {
                    format!("App「{app_id}」的 {} 个工具见 tools，可直接按全名调用（工具列表不随本调用变化）。", tools.len())
                } else if progressive {
                    format!(
                        "App「{app_id}」的 {} 个工具已加入本会话的工具列表（客户端刷新列表后可见）；在此之前也可以直接按全名调用。",
                        tools.len()
                    )
                } else {
                    format!("App「{app_id}」的工具都已在工具列表中，可直接按全名调用。")
                };
                let pages = self.page_summaries(&app_id);
                let message = if pages.is_empty() {
                    message
                } else {
                    format!("{message} 另有 {} 个页面（pages），其上的工具用 apps.page 查看。", pages.len())
                };
                let mut body = json!({
                    "appId": app_id,
                    "tools": tools,
                    "pages": pages,
                    "message": message,
                });
                // 无会话调用方没有"首次附带"，总览随 apps.tools 应请求附带（docs/plans/12-mcp-stateless.md 3.2 H7）。
                let overview = key.is_stateless().then(|| self.overview(&app_id)).flatten();
                if let Some(ov) = &overview {
                    body["overview"] = ov.to_json();
                }
                let mut r = json_result(body);
                if let Some(ov) = &overview {
                    r.content.insert(0, ContentBlock::text(ov.render_requested()));
                }
                Ok(r)
            }
            TOOL_APPS_PAGE => {
                let (app_id, page) = (arg("appId"), arg("page"));
                let known = self.registry().has_app(&app_id);
                if !known || self.app_hidden(&app_id) {
                    return Some(Err(unknown_app(&app_id)));
                }
                Ok(match self.page_detail(&app_id, &page) {
                    Some(v) => json_result(v),
                    None => {
                        return Some(Err(ToolError::new(
                            ErrorKind::ToolNotFound,
                            format!("App「{app_id}」没有页面「{page}」。可调用 apps.tools 查看该 App 的页面（pages）。"),
                        )));
                    }
                })
            }
            TOOL_APPS_TASK_BEGIN => self.builtin_task_begin(ctx),
            TOOL_APPS_TASK_END => self.builtin_task_end(ctx),
            TOOL_APPS_LOCK | TOOL_APPS_UNLOCK if !self.locks_enabled() => Err(ToolError::new(
                ErrorKind::ToolNotFound,
                format!("工具「{name}」不存在：本 Hub 未启用对象锁（max_locks = 0）。"),
            )),
            TOOL_APPS_LOCK => self.builtin_lock(ctx, args),
            TOOL_APPS_UNLOCK => self.builtin_unlock(ctx, args),
            TOOL_APPS_CALLS => self.builtin_calls(key, ctx.call_id.as_deref()),
            TOOL_APPS_CANCEL => self.builtin_cancel(key, args),
            _ => return None,
        })
    }
}
