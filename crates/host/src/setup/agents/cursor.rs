//! Cursor：全局配置 `~/.cursor/mcp.json`，`{"mcpServers": {"<name>": {"url": "..."}}}`。
//!
//! @why 依据：Cursor 官方文档 <https://cursor.com/docs/context/mcp>（2026-10-02 查阅）：全局配置为主目录下的
//! `~/.cursor/mcp.json`；远程服务器以 `url` 字段表示（无 `type`）。本机未安装 Cursor，未实测。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{AgentEnv, AgentId, AgentSpec, Detect, JsonServers, Method};

pub const SPEC: AgentSpec = AgentSpec {
    id: AgentId::Cursor,
    key: "cursor",
    display: "Cursor",
    evidence: "Cursor 官方文档 https://cursor.com/docs/context/mcp（~/.cursor/mcp.json，mcpServers.<name>.url）",
    detect: Detect::Dir(dir),
    config_file,
    method: Method::File(SERVERS),
    manual,
};

pub const SERVERS: JsonServers = JsonServers { root: "mcpServers", entry, url_of };

fn entry(url: &str) -> Value {
    json!({ "url": url })
}

fn url_of(v: &Value) -> Option<String> {
    v.get("url").and_then(Value::as_str).map(str::to_owned)
}

fn dir(env: &AgentEnv) -> PathBuf {
    env.user_home.join(".cursor")
}

fn config_file(env: &AgentEnv) -> Option<PathBuf> {
    Some(dir(env).join("mcp.json"))
}

fn manual(name: &str, url: &str) -> String {
    format!("在 ~/.cursor/mcp.json 的 \"mcpServers\" 中加入 \"{name}\": {{ \"url\": \"{url}\" }}")
}
