//! Gemini CLI：`gemini mcp add --scope user --transport http`，用户设置 `~/.gemini/settings.json` 的 `mcpServers`。
//!
//! @why 依据（2026-10-02，本机 gemini 0.46.0）：`gemini mcp add --help` 列出 `-s, --scope <user|project>`（默认 project）
//! 与 `-t, --transport <stdio|sse|http>`；`gemini mcp` 没有 `get` 子命令，`list` 为人类可读文本，因此直接读设置文件。
//! 在隔离的 `HOME` 下实测：`add` 写入 `~/.gemini/settings.json` 的 `mcpServers.<name> = {"url": ..., "type": "http"}`，
//! 同名条目直接更新（退出 0）；`remove --scope user` 不存在时也退出 0。旧格式 `httpUrl` 同样识别为 HTTP 条目。

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{AgentEnv, AgentId, AgentSpec, CliRead, CliSpec, Detect, JsonServers, Method};

pub const SPEC: AgentSpec = AgentSpec {
    id: AgentId::Gemini,
    key: "gemini",
    display: "Gemini CLI",
    evidence: "本机 gemini 0.46.0：`gemini mcp add --help`（--scope user、--transport http）与隔离 HOME 实测（~/.gemini/settings.json）",
    detect: Detect::Program("gemini"),
    config_file,
    method: Method::Cli(CliSpec {
        program: "gemini",
        add,
        remove,
        replace_needs_remove: false,
        read: CliRead::File(SETTINGS),
    }),
    manual,
};

/// `settings.json` 中的服务器表（只读；写入经 `gemini mcp add`）。
pub const SETTINGS: JsonServers = JsonServers { root: "mcpServers", entry, url_of };

fn entry(url: &str) -> Value {
    json!({ "url": url, "type": "http" })
}

/// `httpUrl`（旧格式），或 `type = "http"` 的 `url`（不带 type 的 `url` 是 SSE）。
fn url_of(v: &Value) -> Option<String> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    s("httpUrl").or_else(|| s("url").filter(|_| s("type") == Some("http"))).map(str::to_owned)
}

fn config_file(env: &AgentEnv) -> Option<PathBuf> {
    Some(env.user_home.join(".gemini").join("settings.json"))
}

fn add(name: &str, url: &str) -> Vec<String> {
    ["mcp", "add", "--scope", "user", "--transport", "http", name, url].map(str::to_owned).to_vec()
}

fn remove(name: &str) -> Vec<String> {
    ["mcp", "remove", "--scope", "user", name].map(str::to_owned).to_vec()
}

fn manual(name: &str, url: &str) -> String {
    format!("gemini mcp add --scope user --transport http {name} {url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_entry_shapes() {
        assert_eq!(url_of(&entry("http://h/mcp")).as_deref(), Some("http://h/mcp"));
        assert_eq!(url_of(&json!({"httpUrl": "http://h/mcp"})).as_deref(), Some("http://h/mcp"));
        assert_eq!(url_of(&json!({"url": "http://h/sse"})), None);
        assert_eq!(url_of(&json!({"command": "x"})), None);
    }
}
