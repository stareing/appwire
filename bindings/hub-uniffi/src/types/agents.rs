//! Agent 身份（spec/hub-api.md 3.6「Agent 身份」）：按 Agent 发的访问令牌。

use std::fmt;

use app_mcp_hub as hub;

/// 一个 Agent 的访问令牌：经 MCP HTTP 出口出示此令牌的请求，主体为 `agent:<name>`（只用于区分与归属，不做授权）。
///
/// @security `Debug` 不输出令牌。
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentCredential {
    /// 1–64 个 ASCII 字母、数字、`-`、`_`、`.`，以字母或数字开头。
    pub name: String,
    /// 32–512 个可见 ASCII 字符、不含空白（`Authorization: Bearer <token>`）。
    pub token: String,
}

impl fmt::Debug for AgentCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentCredential").field("name", &self.name).field("token", &"<redacted>").finish()
    }
}

pub(crate) fn agents_config(agents: Vec<AgentCredential>) -> hub::AgentsConfig {
    hub::AgentsConfig {
        agents: agents.into_iter().map(|a| hub::AgentCredential { name: a.name, token: a.token }).collect(),
    }
}
