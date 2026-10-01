//! Claude Code：`claude mcp add|get|remove`，用户作用域（所有项目可用）。
//!
//! @why 依据（2026-10-02，本机 Claude Code 2.1.281）：`claude mcp add --help` 列出 `-s, --scope <local|user|project>`
//! 与 `-t, --transport <stdio|sse|http>`；在隔离的 `CLAUDE_CONFIG_DIR` 下实测：`add` 遇同名条目报 "already exists" 退出 1，
//! `get` 输出 `Scope:` / `Type:` / `URL:` 行、不存在时 "No MCP server named" 退出 1，`remove -s user` 删除用户作用域条目，
//! 用户作用域写入 `$CLAUDE_CONFIG_DIR/.claude.json`（未设置时 `~/.claude.json`）的 `mcpServers`。

use std::path::PathBuf;

use super::{AgentEnv, AgentId, AgentSpec, CliRead, CliSpec, Detect, Existing, Method, field, mentions};
use crate::setup::ops::CmdOutput;

pub const SPEC: AgentSpec = AgentSpec {
    id: AgentId::ClaudeCode,
    key: "claude-code",
    display: "Claude Code",
    evidence: "本机 claude 2.1.281：`claude mcp add --help`（--scope user、--transport http）与隔离 CLAUDE_CONFIG_DIR 实测",
    detect: Detect::Program("claude"),
    config_file,
    method: Method::Cli(CliSpec {
        program: "claude",
        add,
        remove,
        replace_needs_remove: true,
        read: CliRead::Command { args: get, parse: parse_get },
    }),
    manual,
};

fn config_file(env: &AgentEnv) -> Option<PathBuf> {
    Some(env.claude_config_dir.as_ref().unwrap_or(&env.user_home).join(".claude.json"))
}

fn add(name: &str, url: &str) -> Vec<String> {
    ["mcp", "add", "--scope", "user", "--transport", "http", name, url].map(str::to_owned).to_vec()
}

fn remove(name: &str) -> Vec<String> {
    ["mcp", "remove", "--scope", "user", name].map(str::to_owned).to_vec()
}

fn get(name: &str) -> Vec<String> {
    ["mcp", "get", name].map(str::to_owned).to_vec()
}

/// 解析 `claude mcp get <name>`。
pub fn parse_get(out: &CmdOutput) -> Result<Option<Existing>, String> {
    if !out.success() {
        return if mentions(out, "No MCP server named") { Ok(None) } else { Err(out.text().trim().to_owned()) };
    }
    let text = out.text();
    let scope = field(&text, "Scope").unwrap_or("未知作用域");
    let kind = field(&text, "Type").unwrap_or("未知类型");
    let url = field(&text, "URL").filter(|_| kind == "http").map(str::to_owned);
    let target = field(&text, "URL").or_else(|| field(&text, "Command")).unwrap_or("");
    Ok(Some(Existing {
        url,
        summary: format!("{kind} {target}（{scope}）").replace("  ", " "),
        // `-s user` 只能改用户作用域；其他作用域的同名条目由用户自行处理。
        replaceable: scope.starts_with("User"),
    }))
}

fn manual(name: &str, url: &str) -> String {
    format!("claude mcp add --scope user --transport http {name} {url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32, stdout: &str) -> CmdOutput {
        CmdOutput { code: Some(code), stdout: stdout.into(), stderr: String::new() }
    }

    #[test]
    fn parses_get_output() {
        // 实测输出（隔离 CLAUDE_CONFIG_DIR，Claude Code 2.1.281）
        let http = "app-mcp:\n  Scope: User config (available in all your projects)\n  Status: ✘ Failed to connect\n  Type: http\n  URL: http://127.0.0.1:7717/mcp\n\nTo remove this server, run: claude mcp remove app-mcp -s user\n";
        let e = parse_get(&out(0, http)).unwrap().unwrap();
        assert_eq!(e.url.as_deref(), Some("http://127.0.0.1:7717/mcp"));
        assert!(e.replaceable);
        let stdio = "context7:\n  Scope: User config (available in all your projects)\n  Status: ✔ Connected\n  Type: stdio\n  Command: npx\n  Args: -y @upstash/context7-mcp\n";
        let e = parse_get(&out(0, stdio)).unwrap().unwrap();
        assert_eq!(e.url, None);
        assert!(e.summary.contains("stdio npx"));
        let local = "app-mcp:\n  Scope: Local config (private to you in this project)\n  Type: http\n  URL: http://x/mcp\n";
        assert!(!parse_get(&out(0, local)).unwrap().unwrap().replaceable);
        let missing = "No MCP server named \"app-mcp\". Configured servers: context7";
        assert_eq!(parse_get(&out(1, missing)).unwrap(), None);
        assert!(parse_get(&out(1, "boom")).is_err());
    }

    #[test]
    fn args() {
        assert_eq!(add("app-mcp", "http://h/mcp").join(" "), "mcp add --scope user --transport http app-mcp http://h/mcp");
        assert_eq!(remove("app-mcp").join(" "), "mcp remove --scope user app-mcp");
    }
}
