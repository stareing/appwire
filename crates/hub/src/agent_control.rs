//! Agent 显式控制 App 的内置工具（spec/hub-api.md 3.15）：`apps.navigate`（显式导航，可带页面参数）、`apps.activate`
//! （只唤醒不调用）、`apps.release`（收回本会话在该 App 上的租约）。
//!
//! 只提供机制：何时导航、预热或释放由 Agent 决定；是否允许导航由 App 决定（`NAVIGATION_DENIED`）。策略挂点（3.13）、
//! 资源保护（3.11）与唤醒速率上限（3.5）照常生效。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::call::{CallCtx, CancelFut, TOOL_APPS_ACTIVATE, TOOL_APPS_NAVIGATE, TOOL_APPS_RELEASE, json_result};
use crate::hub::HubShared;
use crate::schema::{self, SchemaCheck};
use crate::types::AppState;

fn unknown_app(app_id: &str) -> ToolError {
    ToolError::new(
        ErrorKind::ToolNotFound,
        format!("没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"),
    )
}

fn str_arg(args: &Value, key: &str) -> String {
    args.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

impl HubShared {
    /// 本模块的内置工具；`name` 不是其中之一时返回 `None`。参数已按内置 inputSchema 校验。
    pub(crate) async fn call_control_builtin(
        self: &Arc<Self>,
        ctx: &CallCtx,
        name: &str,
        args: &Value,
        cancel: CancelFut<'_>,
    ) -> Option<Result<CallToolResult, ToolError>> {
        let app_id = str_arg(args, "appId");
        Some(match name {
            TOOL_APPS_NAVIGATE => self.builtin_navigate(ctx, &app_id, args, cancel).await,
            TOOL_APPS_ACTIVATE => self.builtin_activate(ctx, &app_id, cancel).await,
            TOOL_APPS_RELEASE => self.builtin_release(ctx, &app_id),
            _ => return None,
        })
    }

    /// App 已知且未被 `hide` 整体隐藏（上游 MCP 服务器不适用这些操作，按未知处理）。
    fn control_target(&self, app_id: &str) -> Result<(), ToolError> {
        if self.app_hidden(app_id) || !self.registry().has_app(app_id) {
            return Err(unknown_app(app_id));
        }
        Ok(())
    }

    /// App 没有已连接实例时按唤醒规则唤醒（选定 / 最近活跃的休眠实例，否则按清单冷启动），返回被唤醒的实例；已连接时为
    /// `None`。`tool` 为触发唤醒的工具（策略 `wake` 执行点按它匹配；`None` 只匹配 App 级规则）。
    ///
    /// @error `APP_DISCONNECTED`（不能唤醒 / `waker: none`，带 `launchUrl`）、`POLICY_DENIED`、`LAUNCH_FAILED`（含唤醒速率上限）、
    /// `APP_NOT_RESPONDING`、`CANCELLED`。
    pub(crate) async fn wake_app_if_disconnected(
        self: &Arc<Self>,
        app_id: &str,
        selected: Option<&str>,
        tool: Option<&str>,
        cancel: CancelFut<'_>,
    ) -> Result<Option<String>, ToolError> {
        if self.registry().has_connected(app_id) {
            return Ok(None);
        }
        let plan = self
            .registry()
            .wake_plan_app(app_id, selected);
        let Some(plan) = plan.filter(|p| self.wake_reachable(p)) else {
            return Err(self.registry().disconnected_error(app_id));
        };
        self.check_wake_policy(app_id, tool)?;
        self.wake_and_wait(&plan, cancel).await.map(Some)
    }

    /// `apps.navigate {appId, page, params?}`：页面须在 Agent 可见的页面目录中；`navigable: false` → `NAVIGATION_DENIED`；
    /// `params`（缺省按 `{}`）按页面的 `params` schema 校验；策略 `call`（App 级规则）→ 资源保护（按 App 与
    /// `apps.navigate` 计数）→ 必要时唤醒 → `app/navigate {page, params}` → 等 App 回复（`navigate_timeout`）。
    async fn builtin_navigate(
        self: &Arc<Self>,
        ctx: &CallCtx,
        app_id: &str,
        args: &Value,
        mut cancel: CancelFut<'_>,
    ) -> Result<CallToolResult, ToolError> {
        self.control_target(app_id)?;
        let page_name = str_arg(args, "page");
        let page = self.page_catalog(app_id).into_iter().find(|p| p.name == page_name).ok_or_else(|| {
            ToolError::new(
                ErrorKind::ToolNotFound,
                format!("App「{app_id}」没有页面「{page_name}」。可调用 apps.tools 查看该 App 的页面（pages）。"),
            )
        })?;
        if !page.navigable {
            return Err(HubShared::not_navigable(app_id, None, &page_name));
        }
        let params = args.get("params").cloned();
        if let Some(params_schema) = &page.params {
            let checked = params.clone().unwrap_or_else(|| json!({}));
            match schema::check(params_schema, &checked) {
                SchemaCheck::Invalid(msg) => {
                    return Err(ToolError::new(
                        ErrorKind::InvalidInput,
                        format!("页面参数不符合 App「{app_id}」页面「{page_name}」的 params schema：{msg}"),
                    ));
                }
                SchemaCheck::BadSchema(e) => {
                    tracing::warn!(app_id, page = %page_name, error = %e, "页面的 params schema 无法编译，跳过 Hub 侧校验");
                }
                SchemaCheck::Valid | SchemaCheck::Unchecked => {}
            }
        }
        self.check_app_call_policy(app_id)?;
        self.guard_call(app_id, TOOL_APPS_NAVIGATE, args)?;
        let selected = self.selected_for(&ctx.caller, app_id);
        let woken = self.wake_app_if_disconnected(app_id, selected.as_deref(), None, cancel.as_mut()).await?;
        let prefer = woken.as_deref().or(selected.as_deref());
        let instance_id = self.navigate_to_page(app_id, &page_name, params, prefer, cancel).await?;
        Ok(json_result(json!({
            "appId": app_id,
            "instanceId": instance_id,
            "page": page_name,
            "ok": true,
            "woke": woken.is_some(),
            "message": format!(
                "App「{app_id}」已打开页面「{page_name}」。该页面的工具注册后出现在工具列表中（也可用 apps.page 查看）。"
            ),
        })))
    }

    /// `apps.activate {appId}`：只唤醒不调用。已连接 → 不唤醒；否则按唤醒规则唤醒（策略 `wake` 执行点、唤醒速率上限、
    /// `waker: none` 与资源读取触发的唤醒相同）。随后向首选实例发一次本会话的租约（与调用完成后相同），实例在租约内不休眠。
    async fn builtin_activate(
        self: &Arc<Self>,
        ctx: &CallCtx,
        app_id: &str,
        cancel: CancelFut<'_>,
    ) -> Result<CallToolResult, ToolError> {
        self.control_target(app_id)?;
        let selected = self.selected_for(&ctx.caller, app_id);
        let woken = self.wake_app_if_disconnected(app_id, selected.as_deref(), None, cancel).await?;
        let prefer = woken.as_deref().or(selected.as_deref());
        let target = self.registry().preferred_instance(app_id, prefer);
        let Some((instance_id, conn)) = target else {
            // 唤醒后实例尚未就绪即断开等边界情况：如实报告。
            return Err(self.registry().disconnected_error(app_id));
        };
        self.grant_lease(&ctx.caller, app_id, &conn);
        let woke = woken.is_some();
        Ok(json_result(json!({
            "appId": app_id,
            "instanceId": instance_id,
            "state": AppState::Connected,
            "woke": woke,
            "message": if woke {
                format!("已唤醒 App「{app_id}」（实例 {instance_id}）。不再需要时可调用 apps.release。")
            } else {
                format!("App「{app_id}」已在运行（实例 {instance_id}），未重复唤醒。")
            },
        })))
    }

    /// `apps.release {appId}`：收回本会话在该 App 各已连接实例上的租约（`app/lease {ttlMs: 0}`，其他会话的未到期租约随后补发）。
    /// 不断开、不要求休眠：之后何时休眠由 App 的生命周期设置决定；再次调用其工具时照常唤醒 / 续租。
    fn builtin_release(&self, ctx: &CallCtx, app_id: &str) -> Result<CallToolResult, ToolError> {
        self.control_target(app_id)?;
        let conns = self.registry().connection_ids(app_id);
        let released = self.release_leases_on(&ctx.caller, &conns);
        Ok(json_result(json!({
            "appId": app_id,
            "released": released,
            "message": if released == 0 {
                format!("本会话没有 App「{app_id}」的租约，无需释放。")
            } else {
                format!("已收回本会话在 App「{app_id}」上的 {released} 个租约；App 之后按自己的生命周期设置休眠。")
            },
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_arg_defaults_to_empty() {
        assert_eq!(str_arg(&json!({ "appId": "shop" }), "appId"), "shop");
        assert_eq!(str_arg(&json!({ "appId": 1 }), "appId"), "");
        assert_eq!(str_arg(&json!({}), "page"), "");
    }
}
