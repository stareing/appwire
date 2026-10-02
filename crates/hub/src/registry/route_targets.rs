//! 路由：按路由规则选出工具调用与资源读取的目标实例。

use app_mcp_protocol::{ErrorKind, ToolError, Visibility};
use serde_json::json;

use super::Registry;
use super::records::{ResourceTarget, ToolTarget};

impl Registry {
    /// 按路由规则选出调用 `tool_name` 的目标实例。
    pub fn route_tool(
        &self,
        app_id: &str,
        tool_name: &str,
        selected: Option<&str>,
    ) -> Result<ToolTarget, ToolError> {
        let Some(entry) = self.apps.get(app_id) else {
            return Err(ToolError::new(
                ErrorKind::ToolNotFound,
                format!("没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"),
            ));
        };
        let static_tool = entry.manifest.as_ref().and_then(|m| m.tool(tool_name));
        let label = self.app_label(app_id);
        if entry.instances.is_empty() {
            return Err(if static_tool.is_some() {
                self.disconnected_error(app_id)
            } else {
                ToolError::new(
                    ErrorKind::ToolNotFound,
                    format!("App{label}没有名为「{tool_name}」的工具。"),
                )
            });
        }
        let Some(inst) = entry
            .ordered(selected, |i| i.tools.contains_key(tool_name))
            .into_iter()
            .next()
        else {
            let message = if static_tool.is_some() {
                format!(
                    "工具「{tool_name}」当前不可用：App{label}已连接，但没有实例注册该工具，\
                     通常需要先在 App 中打开对应界面后再调用。"
                )
            } else {
                format!(
                    "App{label}没有名为「{tool_name}」的工具。可调用 apps.list 查看各实例注册的工具。"
                )
            };
            return Err(ToolError::new(ErrorKind::ToolNotFound, message));
        };
        if inst.visibility == Some(Visibility::Frozen) {
            let title = inst.title.as_deref().unwrap_or(&inst.instance_id);
            return Err(ToolError::new(
                ErrorKind::InstanceFrozen,
                format!(
                    "目标实例「{title}」的页面已被浏览器冻结（后台标签页）。请让用户切换到该页面后重试，\
                     或用 apps.select 选择其他实例。"
                ),
            )
            .with_details(json!({ "appId": app_id, "instanceId": inst.instance_id })));
        }
        let tool = inst.tools.get(tool_name).cloned().ok_or_else(|| {
            ToolError::new(
                ErrorKind::ToolNotFound,
                format!("工具「{tool_name}」不存在"),
            )
        })?;
        Ok(ToolTarget {
            instance_id: inst.instance_id.clone(),
            conn: inst.conn.clone(),
            tool,
        })
    }

    /// 按路由规则选出读取资源 `name` 的目标实例。
    pub fn route_resource(
        &self,
        app_id: &str,
        name: &str,
        selected: Option<&str>,
    ) -> Result<ResourceTarget, ToolError> {
        let Some(entry) = self.apps.get(app_id) else {
            return Err(ToolError::new(
                ErrorKind::ResourceNotFound,
                format!("没有 appId 为「{app_id}」的 App。"),
            ));
        };
        if entry.instances.is_empty() {
            return Err(self.disconnected_error(app_id));
        }
        let Some(inst) = entry
            .ordered(selected, |i| i.resources.contains_key(name))
            .into_iter()
            .next()
        else {
            return Err(ToolError::new(
                ErrorKind::ResourceNotFound,
                format!(
                    "App{}当前没有实例提供资源「{name}」。",
                    self.app_label(app_id)
                ),
            ));
        };
        let resource = inst.resources.get(name).cloned().ok_or_else(|| {
            ToolError::new(ErrorKind::ResourceNotFound, format!("资源「{name}」不存在"))
        })?;
        Ok(ResourceTarget {
            instance_id: inst.instance_id.clone(),
            conn: inst.conn.clone(),
            resource,
        })
    }

}
