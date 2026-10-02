//! Agent 身份（docs/plans/16-agent-os.md N5；spec/hub-api.md 3.6「Agent 身份」）：按 Agent 发的访问令牌。
//!
//! 令牌 → Agent 名的登记表。`/mcp` 核对 `Authorization: Bearer` 时先比本机令牌、再查本表：命中即该请求的主体为
//! `agent:<名>`（调用方键 `principal:agent:<名>`），任务、任务句柄、`apps.select`、租约、listen 流上限与审批的 `principal`
//! 随之按 Agent 分开。身份只用于区分与归属（句柄绑定、记账、日志），授权由 Agent 自身配置负责，本库不做。
//!
//! @security 名字来自登记（用户 / 厂商），不取自 `clientInfo`（自报、不可信，S-F6）；令牌比较为常量时间，
//! `Debug` 与 `/status` 不输出令牌。

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 一次登记的 Agent 数上限（B-07）。
pub const MAX_AGENTS: usize = 256;
/// Agent 名的最大长度（字符）。
pub const MAX_AGENT_NAME_LEN: usize = 64;
/// Agent 令牌的最小长度（字节）。
///
/// @why 32：至少 128 位熵的十六进制表示；`app-mcp-host agent add` 生成 64 位十六进制（256 位），与本机令牌相同。
pub const MIN_AGENT_TOKEN_LEN: usize = 32;
/// Agent 令牌的最大长度（字节）。
pub const MAX_AGENT_TOKEN_LEN: usize = 512;

/// 一个 Agent 的凭据。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCredential {
    /// Agent 名：1–64 个 ASCII 字母、数字、`-`、`_`、`.`，以字母或数字开头（[`is_valid_agent_name`]）。
    pub name: String,
    /// 访问令牌（`Authorization: Bearer <令牌>`）：32–512 个可见 ASCII 字符，不含空白。
    pub token: String,
}

impl fmt::Debug for AgentCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentCredential").field("name", &self.name).field("token", &"<redacted>").finish()
    }
}

/// Agent 登记（[`crate::HubConfig::agents`]、[`crate::Hub::set_agents`]、`POST /agents`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(default)]
    pub agents: Vec<AgentCredential>,
}

impl AgentsConfig {
    /// 从 JSON 文本解析并校验。
    ///
    /// @error 不是合法 JSON / 结构不符 / [`AgentsConfig::validate`] 失败 → 中文错误说明（不含令牌）。
    pub fn from_json(text: &str) -> Result<Self, String> {
        let config: Self = serde_json::from_str(text).map_err(|e| format!("Agent 登记不是合法的 JSON：{e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// 校验：条数上限、名字格式、令牌格式、名字与令牌各自不重复。
    pub fn validate(&self) -> Result<(), String> {
        if self.agents.len() > MAX_AGENTS {
            return Err(format!("登记的 Agent 过多（{} 个，上限 {MAX_AGENTS}）。", self.agents.len()));
        }
        for (i, a) in self.agents.iter().enumerate() {
            if !is_valid_agent_name(&a.name) {
                return Err(format!(
                    "第 {} 个 Agent 的名字 {:?} 不合法：需 1–{MAX_AGENT_NAME_LEN} 个字母、数字、-、_、.，以字母或数字开头。",
                    i + 1,
                    a.name
                ));
            }
            if !is_valid_token(&a.token) {
                return Err(format!(
                    "Agent {:?} 的令牌不合法：需 {MIN_AGENT_TOKEN_LEN}–{MAX_AGENT_TOKEN_LEN} 个可见 ASCII 字符、不含空白。",
                    a.name
                ));
            }
            let earlier = &self.agents[..i];
            if earlier.iter().any(|b| b.name == a.name) {
                return Err(format!("Agent 名 {:?} 重复。", a.name));
            }
            if earlier.iter().any(|b| token_eq(&b.token, &a.token)) {
                return Err(format!("Agent {:?} 的令牌与其他 Agent 相同。", a.name));
            }
        }
        Ok(())
    }
}

/// Agent 名是否合法（见 [`AgentCredential::name`]）。
pub fn is_valid_agent_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else { return false };
    name.len() <= MAX_AGENT_NAME_LEN
        && first.is_ascii_alphanumeric()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn is_valid_token(token: &str) -> bool {
    (MIN_AGENT_TOKEN_LEN..=MAX_AGENT_TOKEN_LEN).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic())
}

/// 常量时间比较（避免按字节提前返回泄露令牌前缀）。
pub(crate) fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 已登记的 Agent 名（经 [`AgentsConfig::validate`]，可直接用于调用方键）。
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct AgentName(Arc<str>);

impl AgentName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_test(name: &str) -> Self {
        assert!(is_valid_agent_name(name));
        Self(name.into())
    }
}

impl fmt::Debug for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 令牌 → Agent 名（`HubShared` 持有，运行中可整体替换）。
#[derive(Default)]
pub(crate) struct AgentRegistry {
    entries: Vec<(AgentName, String)>,
}

impl AgentRegistry {
    /// @input config 已通过 [`AgentsConfig::validate`]。
    pub(crate) fn new(config: &AgentsConfig) -> Self {
        Self {
            entries: config.agents.iter().map(|a| (AgentName(a.name.as_str().into()), a.token.clone())).collect(),
        }
    }

    /// 令牌对应的 Agent；未登记为 `None`。
    ///
    /// @security 逐条常量时间比较且不提前结束，耗时不随命中位置变化。
    pub(crate) fn identify(&self, token: &str) -> Option<AgentName> {
        self.entries
            .iter()
            .fold(None, |found, (name, t)| if token_eq(t, token) { Some(name.clone()) } else { found })
    }

    /// 登记的 Agent 名，按名字排序（`/status` 的 `agents`）。
    pub(crate) fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.iter().map(|(n, _)| n.as_str().to_owned()).collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T1: &str = "0123456789abcdef0123456789abcdef";
    const T2: &str = "fedcba9876543210fedcba9876543210";

    fn cred(name: &str, token: &str) -> AgentCredential {
        AgentCredential { name: name.to_owned(), token: token.to_owned() }
    }

    #[test]
    fn agent_names() {
        for ok in ["a", "claude", "claude-code", "Agent_1.2", &"x".repeat(MAX_AGENT_NAME_LEN)] {
            assert!(is_valid_agent_name(ok), "{ok}");
        }
        for bad in ["", "-a", ".a", "_a", "a b", "a/b", "a:b", "名字", &"x".repeat(MAX_AGENT_NAME_LEN + 1)] {
            assert!(!is_valid_agent_name(bad), "{bad}");
        }
    }

    #[test]
    fn validate_rules() {
        let ok = AgentsConfig { agents: vec![cred("a", T1), cred("b", T2)] };
        assert_eq!(ok.validate(), Ok(()));
        assert_eq!(AgentsConfig::default().validate(), Ok(()));

        let cases = [
            (vec![cred("a b", T1)], "名字"),
            (vec![cred("a", "short")], "令牌不合法"),
            (vec![cred("a", &format!("{T1} x"))], "令牌不合法"),
            (vec![cred("a", &"t".repeat(MAX_AGENT_TOKEN_LEN + 1))], "令牌不合法"),
            (vec![cred("a", T1), cred("a", T2)], "重复"),
            (vec![cred("a", T1), cred("b", T1)], "相同"),
        ];
        for (agents, needle) in cases {
            let e = AgentsConfig { agents }.validate().unwrap_err();
            assert!(e.contains(needle), "{e}");
            assert!(!e.contains(T1), "错误信息不得含令牌：{e}");
        }
        let many = AgentsConfig {
            agents: (0..=MAX_AGENTS).map(|i| cred(&format!("a{i}"), &format!("{T1}{i:04}"))).collect(),
        };
        assert!(many.validate().unwrap_err().contains("过多"));
    }

    #[test]
    fn from_json_and_debug_redacts() {
        let c = AgentsConfig::from_json(&format!(r#"{{"agents":[{{"name":"claude","token":"{T1}"}}]}}"#)).unwrap();
        assert_eq!(c.agents[0].name, "claude");
        assert!(!format!("{c:?}").contains(T1));
        assert_eq!(AgentsConfig::from_json("{}").unwrap(), AgentsConfig::default());
        assert!(AgentsConfig::from_json("[").unwrap_err().contains("JSON"));
        assert!(AgentsConfig::from_json(r#"{"agents":[{"name":"","token":"x"}]}"#).is_err());
    }

    #[test]
    fn registry_identifies_by_token() {
        let reg = AgentRegistry::new(&AgentsConfig { agents: vec![cred("b", T2), cred("a", T1)] });
        assert_eq!(reg.identify(T1).map(|n| n.to_string()).as_deref(), Some("a"));
        assert_eq!(reg.identify(T2).map(|n| n.to_string()).as_deref(), Some("b"));
        assert_eq!(reg.identify("nope"), None);
        assert_eq!(reg.identify(&T1[..31]), None);
        assert_eq!(reg.names(), ["a", "b"]);
        assert_eq!(AgentRegistry::default().identify(T1), None);
    }

    #[test]
    fn token_eq_basic() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
    }
}
