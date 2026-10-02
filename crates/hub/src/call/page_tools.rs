//! 页面工具（spec/hub-api.md 3.14）：页面目录摘要 / 详情，以及调用不在当前页面的工具时的导航到达。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolError};
use serde_json::{Value, json};

use crate::hub::HubShared;
use crate::navigate::PageTool;
use crate::schema::{self, SchemaCheck};
use crate::types::{Availability, HubTool};

use super::{CallCtx, CancelFut};
use super::tool_convert::app_hub_tool;

impl HubShared {
    /// 页面工具的到达：`navigable: false` 拒绝 → 按目录定义校验并审批（未审批时）→ App 没有连接时唤醒 → 导航并等待
    /// 工具注册。返回注册了该工具的实例，以及本步是否唤醒了 App。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reach_page_tool(
        self: &Arc<Self>,
        call_id: &str,
        app_id: &str,
        target: &PageTool,
        arguments: &Value,
        ctx: &CallCtx,
        selected: Option<&str>,
        woken: Option<String>,
        approved: bool,
        mut cancel: CancelFut<'_>,
    ) -> Result<(String, bool), ToolError> {
        let tool_name = target.tool.name.as_str();
        if !target.navigable {
            return Err(HubShared::not_navigable(app_id, Some(tool_name), &target.page));
        }
        if !approved {
            if let SchemaCheck::Invalid(msg) = schema::check_json(target.tool.input_schema_json(), arguments) {
                return Err(ToolError::new(
                    ErrorKind::InvalidInput,
                    format!("参数不符合工具「{app_id}.{tool_name}」的 inputSchema：{msg}"),
                ));
            }
            let hub_tool = app_hub_tool(app_id, &target.tool, Availability::NotRegistered);
            let req = self.approval_request(call_id, &hub_tool, arguments, ctx);
            self.approve(req, cancel.as_mut()).await?;
        }
        let page_woken = self.wake_app_if_disconnected(app_id, selected, Some(tool_name), ctx.agent(), cancel.as_mut()).await?;
        let woke = page_woken.is_some();
        let woken = page_woken.or(woken);
        // 唤醒后实例可能已停在该页面。
        if let Some(id) = woken.as_deref().filter(|id| self.registry().instance_has_tool(app_id, id, tool_name)) {
            return Ok((id.to_owned(), woke));
        }
        let strict = ctx.instance_id.is_some();
        let prefer = ctx.instance_id.as_deref().or(woken.as_deref()).or(selected);
        self.navigate_for_tool(app_id, &target.page, tool_name, prefer, strict, cancel).await.map(|id| (id, woke))
    }

    /// `apps.tools` 的页面摘要（spec/hub-api.md 3.14 L2）：`{name, title?, description?, navigable, current, toolCount}`。
    pub(super) fn page_summaries(&self, app_id: &str) -> Vec<Value> {
        let pages = self.page_catalog(app_id);
        let reg = self.registry();
        pages
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "title": p.title,
                    "description": p.description,
                    "navigable": p.navigable,
                    "current": p.tools.keys().any(|t| reg.tool_registered(app_id, t)),
                    "toolCount": p.tools.len(),
                })
            })
            .collect()
    }

    /// `apps.page` 的结果（L3）；页面不存在（或其工具全部被隐藏）时为 `None`。
    pub(super) fn page_detail(&self, app_id: &str, page: &str) -> Option<Value> {
        let p = self.page_catalog(app_id).into_iter().find(|p| p.name == page)?;
        let reg = self.registry();
        let current = p.tools.keys().any(|t| reg.tool_registered(app_id, t));
        let tools: Vec<HubTool> = p
            .tools
            .values()
            .map(|t| {
                let availability =
                    if reg.tool_registered(app_id, &t.name) { Availability::Available } else { Availability::NotRegistered };
                let mut tool = app_hub_tool(app_id, t, availability);
                tool.page.get_or_insert_with(|| p.name.clone());
                tool
            })
            .collect();
        drop(reg);
        let message = if !p.navigable {
            format!("页面「{page}」不允许由 Agent 导航：其上的工具只在用户自行打开该页面后可用。")
        } else if current {
            format!("页面「{page}」当前已打开，其上的工具可直接按全名调用。")
        } else {
            format!("可直接按全名调用这些工具：Hub 会先让 App 切换到页面「{page}」（改变用户看到的界面；App 可能拒绝）。")
        };
        Some(json!({
            "appId": app_id,
            "page": {
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "route": p.route,
                "params": p.params,
                "navigable": p.navigable,
                "current": current,
            },
            "tools": tools,
            "message": message,
        }))
    }

}
