//! 调用不在当前页面的工具时先导航（第 4c 项，spec/hub-api.md 3.14）：`app/navigate` → 等待目标工具注册；
//! App 在后台时改调 `view` 工具声明的后台替代（`backgroundTool`）。
//!
//! 与唤醒同构：复用 `HubConfig::wake_timeout`（导航回复与等待工具注册合计）与调用的取消信号；导航期间目标连接
//! 记为有进行中的工作（`app/sleep` 被拒绝）。是否允许导航由 App 决定（拒绝时回 `NAVIGATION_DENIED`），Hub 不加确认。

use std::sync::Arc;

use app_mcp_protocol::{
    ErrorKind, NavigateParams, NavigateResult, RpcError, ToolError, method, navigation_reason, user_action_reason,
};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::call::CancelFut;
use crate::hub::HubShared;
use crate::tool_def::SharedTool;
use crate::pages;
use crate::schema::{self, SchemaCheck};

/// 页面目录中某个工具所在的页面（[`HubShared::page_of_tool`]）。
#[derive(Debug, Clone)]
pub(crate) struct PageTool {
    pub page: String,
    pub navigable: bool,
    /// 目录中的定义（唤醒 / 导航前的 schema 校验与审批用）。
    pub tool: SharedTool,
}

fn with_page(e: ToolError, app_id: &str, page: &str) -> ToolError {
    let mut details = match e.details {
        Some(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    details.insert("appId".into(), json!(app_id));
    details.insert("page".into(), json!(page));
    ToolError { details: Some(serde_json::Value::Object(details)), ..e }
}

fn cancelled() -> ToolError {
    ToolError::new(ErrorKind::Cancelled, "调用已被取消。")
}

/// 失败是否因为 App 不能自行回到前台（`USER_ACTION_REQUIRED`，`reason: "foreground"`，spec/protocol.md 3.4）。
pub(crate) fn needs_foreground(e: &ToolError) -> bool {
    e.kind == ErrorKind::UserActionRequired
        && e.details.as_ref().and_then(|d| d.get("reason")).and_then(Value::as_str) == Some(user_action_reason::FOREGROUND)
}

impl HubShared {
    /// 后台替代（spec/hub-api.md 3.14）：`tool` 是没有实例注册、页面目录中有的 `view` 工具，且声明的 `backgroundTool`
    /// 可用（同一 App 中已知、`surface` 为 app、`args` 符合其 inputSchema）时返回该工具名；声明不可用时记 warn 日志并返回 `None`。
    pub(crate) fn background_alternative(&self, app_id: &str, tool: &str, args: &Value) -> Option<String> {
        let view = self.page_of_tool(app_id, tool)?.tool;
        let alt = view.background_tool.clone().filter(|_| !view.surface.is_app())?;
        let problem = if alt == tool {
            Some("指向自身".to_owned())
        } else {
            match self.registry().app_tool(app_id, &alt) {
                None => Some("App 中没有该工具".to_owned()),
                Some(def) if !def.surface.is_app() => Some("不是 app 工具".to_owned()),
                Some(def) => match schema::check_json(def.input_schema_json(), args) {
                    SchemaCheck::Invalid(msg) => Some(format!("参数不符合其 inputSchema：{msg}")),
                    _ => None,
                },
            }
        };
        match problem {
            None => Some(alt),
            Some(why) => {
                tracing::warn!(app_id, tool, background_tool = %alt, "后台替代不可用，按导航处理：{why}");
                None
            }
        }
    }

    /// 导航前是否已知 App 在后台：将被导航的实例不可见，或没有可导航的已连接实例（休眠 / 未运行 / 不支持导航）。
    pub(crate) fn app_in_background(&self, app_id: &str, prefer: Option<&str>, strict: bool) -> bool {
        !self.registry().navigation_target_in_foreground(app_id, prefer, strict)
    }

    /// 没有已连接实例注册 `tool`、而页面目录中有时，返回它所在的页面。
    pub(crate) fn page_of_tool(&self, app_id: &str, tool: &str) -> Option<PageTool> {
        let reg = self.registry();
        if reg.tool_registered(app_id, tool) {
            return None;
        }
        let catalog = reg.pages(app_id);
        let (page, info) = pages::find_tool(&catalog, tool)?;
        Some(PageTool { page: page.name.clone(), navigable: page.navigable, tool: info.clone() })
    }

    /// 清单声明 `navigable: false` 的页面：不导航。
    pub(crate) fn not_navigable(app_id: &str, tool: &str, page: &str) -> ToolError {
        with_page(
            ToolError::new(
                ErrorKind::NavigationDenied,
                format!(
                    "工具「{app_id}.{tool}」位于页面「{page}」，该页面不允许由 Agent 导航打开。请让用户自行打开该页面后重试。"
                ),
            )
            .with_details(json!({ "reason": navigation_reason::NOT_NAVIGABLE })),
            app_id,
            page,
        )
    }

    /// 请实例导航到 `page`，再等待它注册 `tool`；返回目标实例 ID。
    ///
    /// `prefer` 为优先的实例；`strict` 时只能是该实例（调用方指定了 `instanceId`）。
    /// @error `NAVIGATION_FAILED`（不支持 / 出错 / 超时 / 导航后工具未出现）、`NAVIGATION_DENIED`（App 拒绝）、
    /// `APP_DISCONNECTED`（导航中断开）、`CANCELLED`。
    pub(crate) async fn navigate_for_tool(
        self: &Arc<Self>,
        app_id: &str,
        page: &str,
        tool: &str,
        prefer: Option<&str>,
        strict: bool,
        mut cancel: CancelFut<'_>,
    ) -> Result<String, ToolError> {
        let deadline = Instant::now() + self.config.wake_timeout;
        let target = self.registry().navigation_target(app_id, prefer);
        let Some((instance_id, conn)) = target.filter(|(id, _)| !strict || Some(id.as_str()) == prefer) else {
            return Err(with_page(
                ToolError::navigation_failed(
                    format!(
                        "工具「{app_id}.{tool}」位于页面「{page}」，当前不在该页面，且 App 不支持由 Agent 导航。\
                         请让用户在 App 中打开该页面后重试。"
                    ),
                    navigation_reason::UNSUPPORTED,
                ),
                app_id,
                page,
            ));
        };
        let _work = conn.begin_work();
        let mut tools_rev = self.tools_rev.subscribe();
        let params = serde_json::to_value(NavigateParams { page: page.to_owned(), params: None })
            .map_err(|e| ToolError::new(ErrorKind::HandlerError, e.to_string()))?;
        let disconnected = || {
            ToolError::new(
                ErrorKind::AppDisconnected,
                format!("导航到页面「{page}」时 App「{app_id}」的实例断开了连接，调用未执行。"),
            )
        };
        tracing::info!(app_id, instance_id = %instance_id, page, tool, "导航到工具所在页面");
        let (req_id, rx) = conn.start_request(method::NAVIGATE, params).map_err(|_| disconnected())?;
        let reply = tokio::select! {
            r = tokio::time::timeout_at(deadline, rx) => r,
            _ = cancel.as_mut() => {
                conn.forget(&req_id);
                return Err(cancelled());
            }
        };
        match reply {
            Ok(Ok(Ok(v))) => {
                if serde_json::from_value::<NavigateResult>(v).is_ok_and(|r| !r.ok) {
                    return Err(with_page(
                        ToolError::navigation_failed(format!("App 没有完成到页面「{page}」的导航。"), navigation_reason::ERROR),
                        app_id,
                        page,
                    ));
                }
            }
            Ok(Ok(Err(rpc))) => return Err(with_page(self.navigation_error(app_id, page, &rpc), app_id, page)),
            Ok(Err(_)) => return Err(disconnected()),
            Err(_) => {
                conn.forget(&req_id);
                return Err(with_page(
                    ToolError::navigation_failed(
                        format!("App 在 {} 秒内没有完成到页面「{page}」的导航。", self.config.wake_timeout.as_secs()),
                        navigation_reason::TIMEOUT,
                    ),
                    app_id,
                    page,
                ));
            }
        }
        // 导航完成后等待目标工具注册（SDK 可能先发 tools/changed 再回复，也可能之后才注册）。
        loop {
            {
                let reg = self.registry();
                if reg.instance_has_tool(app_id, &instance_id, tool) {
                    return Ok(instance_id);
                }
                if reg.instance(app_id, &instance_id).is_none_or(|i| i.conn.id != conn.id) {
                    return Err(disconnected());
                }
            }
            tokio::select! {
                r = tools_rev.changed() => {
                    if r.is_err() {
                        return Err(disconnected());
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(with_page(
                        ToolError::navigation_failed(
                            format!(
                                "已导航到页面「{page}」，但 {} 秒内工具「{app_id}.{tool}」没有出现（页面可能没有提供该工具或界面未就绪）。",
                                self.config.wake_timeout.as_secs()
                            ),
                            navigation_reason::TOOL_NOT_REGISTERED,
                        ),
                        app_id,
                        page,
                    ));
                }
                _ = cancel.as_mut() => return Err(cancelled()),
            }
        }
    }

    /// App 对 `app/navigate` 的错误回复：结果大小上限同工具结果；`-32601`（旧 SDK）按不支持；
    /// `NAVIGATION_*`、`USER_ACTION_REQUIRED`（如 App 在后台、需用户切到前台）原样；其他类别归为 `NAVIGATION_FAILED`（`error`）。
    fn navigation_error(&self, app_id: &str, page: &str, rpc: &RpcError) -> ToolError {
        if rpc.code == RpcError::METHOD_NOT_FOUND {
            return ToolError::navigation_failed(
                format!("App 不支持由 Agent 导航（页面「{page}」）。请让用户自行打开该页面后重试。"),
                navigation_reason::UNSUPPORTED,
            );
        }
        let e = self.accept_error(app_id, method::NAVIGATE, rpc);
        match e.kind {
            ErrorKind::NavigationFailed
            | ErrorKind::NavigationDenied
            | ErrorKind::UserActionRequired
            | ErrorKind::PayloadTooLarge => e,
            _ => ToolError::navigation_failed(e.message, navigation_reason::ERROR),
        }
    }
}
