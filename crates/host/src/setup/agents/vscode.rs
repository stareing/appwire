//! VS Code（Copilot 智能体模式）：用户配置目录下的 `mcp.json`，`{"servers": {"<name>": {"type": "http", "url": "..."}}}`。
//!
//! @why 依据（2026-10-02 查阅）：<https://code.visualstudio.com/docs/copilot/customization/mcp-servers>——用户级
//! `mcp.json` 位于用户配置（profile）文件夹，HTTP 服务器为 `"type": "http"` + `"url"`；
//! <https://code.visualstudio.com/docs/configure/profiles>——用户配置目录为 Windows `%APPDATA%\Code\User`、
//! macOS `~/Library/Application Support/Code/User`、Linux `~/.config/Code/User`（非默认 profile 在 `User/profiles/<id>`，
//! 本程序只写默认 profile）。`code --add-mcp` 只能添加、不能读取或删除，且 WSL 远程 CLI（本机 code 1.139.1 remote-cli）
//! 不提供该参数，因此直接写文件。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{AgentEnv, AgentId, AgentSpec, Detect, JsonServers, Method};

pub const SPEC: AgentSpec = AgentSpec {
    id: AgentId::Vscode,
    key: "vscode",
    display: "VS Code",
    evidence: "VS Code 官方文档 copilot/customization/mcp-servers 与 configure/profiles（<用户配置目录>/Code/User/mcp.json，servers.<name>.type=http）",
    detect: Detect::Dir(dir),
    config_file,
    method: Method::File(SERVERS),
    manual,
};

pub const SERVERS: JsonServers = JsonServers { root: "servers", entry, url_of };

fn entry(url: &str) -> Value {
    json!({ "type": "http", "url": url })
}

fn url_of(v: &Value) -> Option<String> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    s("url").filter(|_| s("type") == Some("http")).map(str::to_owned)
}

fn dir(env: &AgentEnv) -> PathBuf {
    env.config_dir.join("Code").join("User")
}

fn config_file(env: &AgentEnv) -> Option<PathBuf> {
    Some(dir(env).join("mcp.json"))
}

fn manual(name: &str, url: &str) -> String {
    format!(
        "VS Code 命令面板运行 “MCP: Open User Configuration”，在 \"servers\" 中加入 \"{name}\": {{ \"type\": \"http\", \"url\": \"{url}\" }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_only() {
        assert_eq!(url_of(&entry("http://h/mcp")).as_deref(), Some("http://h/mcp"));
        assert_eq!(url_of(&json!({"type": "sse", "url": "http://h/sse"})), None);
    }
}
