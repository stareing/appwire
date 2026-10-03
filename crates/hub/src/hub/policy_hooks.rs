//! 策略回调（审批 / 配对）、策略挂点（spec/hub-api.md 3.13）与 Agent 登记。

use std::sync::Arc;

use app_mcp_protocol::{ErrorKind, ToolAnnotations, ToolError};

use crate::call;
use crate::policy::{PolicyConfig, PolicyHook};
use crate::types::{ApprovalHandler, PairingHandler};

use super::{HubShared, lock, unix_millis};

impl HubShared {
    // ------------------------------------------------------------------
    // 策略回调
    // ------------------------------------------------------------------

    pub(crate) fn approval_handler(&self) -> Option<Arc<dyn ApprovalHandler>> {
        lock(&self.approval_handler).clone()
    }

    pub(crate) fn pairing_handler(&self) -> Option<Arc<dyn PairingHandler>> {
        lock(&self.pairing_handler).clone()
    }

    // ------------------------------------------------------------------
    // 策略挂点（spec/hub-api.md 3.13）
    // ------------------------------------------------------------------

    /// 当前生效的规则集（快照；替换规则集不影响已取得的快照）。
    pub(crate) fn policy(&self) -> Arc<PolicyConfig> {
        lock(&self.policy).config.clone()
    }

    fn policy_hit(&self, config: &Arc<PolicyConfig>, index: usize) {
        lock(&self.policy).hit(config, index);
    }

    /// 替换规则集；不合法时保留之前的规则并记下错误（`/status` 的 `policy.lastError`）。
    /// 成功后按工具 / 资源列表变化通知所有会话（`hide` 可能改变了列表）。
    pub(crate) fn set_policy(&self, config: PolicyConfig) -> Result<(), String> {
        let n = config.rules.len();
        let r = lock(&self.policy).replace(config, unix_millis());
        match &r {
            Ok(()) => {
                tracing::info!(rules = n, "策略规则已更新");
                self.mark_tools_changed();
                self.mark_resources_changed();
            }
            Err(e) => tracing::warn!(error = %e, "策略规则不合法，继续使用之前的规则"),
        }
        r
    }

    /// 替换已登记的 Agent（[`Hub::set_agents`]、`POST /agents`）；不合法时之前的登记继续生效。
    pub(crate) fn set_agents(&self, config: &crate::agents::AgentsConfig) -> Result<(), String> {
        config.validate()?;
        *lock(&self.agents) = crate::agents::AgentRegistry::new(config);
        tracing::info!(agents = config.agents.len(), "Agent 登记已更新");
        Ok(())
    }

    /// 出示的令牌对应的已登记 Agent。
    pub(crate) fn identify_agent(&self, token: &str) -> Option<crate::agents::AgentName> {
        lock(&self.agents).identify(token)
    }

    /// 记下一次加载失败（规则文本不是合法 JSON 等），之前的规则继续生效。
    pub(crate) fn record_policy_error(&self, message: &str) {
        lock(&self.policy).record_error(message, unix_millis());
    }

    /// 调用 / 资源读取时的整体隐藏检查：隐藏时计入命中规则的次数。
    pub(crate) fn app_hidden_hit(&self, app_id: &str) -> bool {
        let policy = self.policy();
        let Some(i) = policy.app_hidden(app_id) else {
            return false;
        };
        self.policy_hit(&policy, i);
        true
    }

    /// 某个 App / 上游工具当前的注解（Agent 实际看到的；按注解匹配规则时使用）。
    pub(crate) fn tool_annotations(&self, app_id: &str, tool: &str) -> Option<ToolAnnotations> {
        if let Some(st) = lock(&self.upstreams).get(app_id) {
            return st
                .tools
                .iter()
                .find(|t| t.name == tool)
                .map(|t| call::upstream_hub_tool(app_id, t).annotations);
        }
        let reg = self.registry();
        reg.tools_of(|a| a == app_id)
            .into_iter()
            .find(|t| t.info.name == tool)
            .map(|t| t.info.effective_annotations())
            // 页面目录中的工具（不在当前页面）：规则同样按其声明的注解匹配（spec/hub-api.md 3.14）。
            .or_else(|| {
                let catalog = reg.pages(app_id);
                crate::pages::find_tool(&catalog, tool).map(|(_, t)| t.effective_annotations())
            })
    }

    /// Agent 可见的页面目录（spec/hub-api.md 3.14）：去掉被 `hide` 规则隐藏的工具，工具全部被隐藏的页面不列出；
    /// App 整体隐藏或未知时为空。
    pub(crate) fn page_catalog(&self, app_id: &str) -> Vec<crate::pages::PageEntry> {
        let policy = self.policy();
        if policy.app_hidden(app_id).is_some() {
            return Vec::new();
        }
        let mut pages = self.registry().pages(app_id);
        if policy.has_hide() {
            for p in &mut pages {
                let had_tools = !p.tools.is_empty();
                p.tools.retain(|name, t| policy.tool_hidden(app_id, name, Some(&t.effective_annotations())).is_none());
                if had_tools && p.tools.is_empty() {
                    p.name.clear();
                }
            }
            pages.retain(|p| !p.name.is_empty());
        }
        pages
    }

    /// 是否有 Agent 可见的页面目录（决定是否列出内置工具 `apps.page`）。
    pub(crate) fn has_pages(&self) -> bool {
        let apps: Vec<String> = self.registry().app_ids();
        apps.iter().any(|a| !self.page_catalog(a).is_empty())
    }

    /// App 是否被 `hide` 规则整体隐藏（列表与名称解析都按不存在处理）。
    pub(crate) fn app_hidden(&self, app_id: &str) -> bool {
        self.policy().app_hidden(app_id).is_some()
    }

    /// 调用执行点：`hide` → `TOOL_NOT_FOUND`（与不存在的工具相同，隐藏的东西不暴露）；`deny`（call）→ `POLICY_DENIED`。
    /// 无规则时直接放行。App 整体隐藏由调用方按 appId 未知处理（[`Self::app_hidden`]）。`agent`：发起方 Agent（按 Agent 的规则）。
    pub(crate) fn check_call_policy(&self, app_id: &str, tool: &str, agent: Option<&str>) -> Result<(), ToolError> {
        let policy = self.policy();
        if policy.is_empty() {
            return Ok(());
        }
        let annotations = if policy.needs_annotations() { self.tool_annotations(app_id, tool) } else { None };
        let annotations = annotations.as_ref();
        if let Some(i) = policy.tool_hidden(app_id, tool, annotations) {
            self.policy_hit(&policy, i);
            return Err(ToolError::new(
                ErrorKind::ToolNotFound,
                format!("工具「{app_id}.{tool}」不存在。可调用 apps.tools 查看该 App 的工具。"),
            ));
        }
        match policy.denied(PolicyHook::Call, app_id, Some((tool, annotations)), agent) {
            None => Ok(()),
            Some(i) => {
                self.policy_hit(&policy, i);
                Err(crate::policy::denied_error(&policy.rules[i].id, PolicyHook::Call, app_id, Some(tool)))
            }
        }
    }

    /// 调用执行点的 App 级检查（不针对具体工具的操作，如 `apps.navigate`）：只有不带 `tool` / `annotations` 的 `deny`（call）
    /// 规则匹配 → `POLICY_DENIED`。App 整体隐藏由调用方按 appId 未知处理。
    pub(crate) fn check_app_call_policy(&self, app_id: &str, agent: Option<&str>) -> Result<(), ToolError> {
        let policy = self.policy();
        if policy.is_empty() {
            return Ok(());
        }
        match policy.denied(PolicyHook::Call, app_id, None, agent) {
            None => Ok(()),
            Some(i) => {
                self.policy_hit(&policy, i);
                Err(crate::policy::denied_error(&policy.rules[i].id, PolicyHook::Call, app_id, None))
            }
        }
    }

    /// 唤醒执行点：`deny`（wake）→ `POLICY_DENIED`。`tool` 为 `None`（资源读取触发的唤醒）时只有 App 级规则匹配。
    pub(crate) fn check_wake_policy(&self, app_id: &str, tool: Option<&str>, agent: Option<&str>) -> Result<(), ToolError> {
        let policy = self.policy();
        if policy.is_empty() {
            return Ok(());
        }
        let annotations = match tool {
            Some(t) if policy.needs_annotations() => self.tool_annotations(app_id, t),
            _ => None,
        };
        let target = tool.map(|t| (t, annotations.as_ref()));
        match policy.denied(PolicyHook::Wake, app_id, target, agent) {
            None => Ok(()),
            Some(i) => {
                self.policy_hit(&policy, i);
                Err(crate::policy::denied_error(&policy.rules[i].id, PolicyHook::Wake, app_id, tool))
            }
        }
    }

    /// 之前已配对过（token 匹配，或同一 appId + Origin 经 PairingHandler 同意过）。
    pub(crate) fn is_paired(&self, app_id: &str, origin: Option<&str>, token: Option<&str>) -> bool {
        if let Some(t) = token
            && lock(&self.paired_tokens).get(t).is_some_and(|a| a == app_id)
        {
            return true;
        }
        lock(&self.paired_origins).contains(&(app_id.to_owned(), origin.map(str::to_owned)))
    }

    pub(crate) fn remember_pairing(&self, app_id: &str, origin: Option<&str>, token: &str, approved: bool) {
        lock(&self.paired_tokens).insert(token.to_owned(), app_id.to_owned());
        if approved {
            lock(&self.paired_origins).insert((app_id.to_owned(), origin.map(str::to_owned)));
        }
    }

}
