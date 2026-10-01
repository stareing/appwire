//! Codex CLI：`codex mcp add <name> --url <url>`（streamable HTTP），全局配置 `~/.codex/config.toml`（或 `$CODEX_HOME`）。
//!
//! @why 依据（2026-10-02，本机 codex-cli 0.156.1）：`codex mcp add --help` 列出 `--url <URL>`（streamable HTTP），
//! `codex mcp get --help` 列出 `--json`；在隔离的 `CODEX_HOME` 下实测：`add` 对同名条目直接覆盖（退出 0），
//! `get --json` 输出 `transport.type = "streamable_http"` 与 `transport.url`，不存在时 "No MCP server named" 退出 1，
//! `remove` 不存在的条目也退出 0；写入 `$CODEX_HOME/config.toml` 的 `[mcp_servers.<name>]`。

use std::path::PathBuf;

use serde_json::Value;

use super::{AgentEnv, AgentId, AgentSpec, CliRead, CliSpec, Detect, Existing, Method, mentions};
use crate::setup::ops::CmdOutput;

pub const SPEC: AgentSpec = AgentSpec {
    id: AgentId::Codex,
    key: "codex",
    display: "Codex CLI",
    evidence: "本机 codex-cli 0.156.1：`codex mcp add --help`（--url）、`codex mcp get --json` 与隔离 CODEX_HOME 实测",
    detect: Detect::Program("codex"),
    config_file,
    method: Method::Cli(CliSpec {
        program: "codex",
        add,
        remove,
        replace_needs_remove: false,
        read: CliRead::Command { args: get, parse: parse_get },
    }),
    manual,
};

fn config_file(env: &AgentEnv) -> Option<PathBuf> {
    Some(env.codex_home.clone().unwrap_or_else(|| env.user_home.join(".codex")).join("config.toml"))
}

fn add(name: &str, url: &str) -> Vec<String> {
    ["mcp", "add", name, "--url", url].map(str::to_owned).to_vec()
}

fn remove(name: &str) -> Vec<String> {
    ["mcp", "remove", name].map(str::to_owned).to_vec()
}

fn get(name: &str) -> Vec<String> {
    ["mcp", "get", name, "--json"].map(str::to_owned).to_vec()
}

/// 解析 `codex mcp get <name> --json`。
pub fn parse_get(out: &CmdOutput) -> Result<Option<Existing>, String> {
    if !out.success() {
        return if mentions(out, "No MCP server named") { Ok(None) } else { Err(out.text().trim().to_owned()) };
    }
    let v: Value = serde_json::from_str(out.stdout.trim()).map_err(|e| format!("输出不是 JSON：{e}"))?;
    let t = &v["transport"];
    let kind = t["type"].as_str().unwrap_or("未知类型");
    Ok(Some(match (kind, t["url"].as_str()) {
        ("streamable_http", Some(url)) => Existing::http(url),
        _ => Existing {
            url: None,
            summary: format!("{kind} {}", t["command"].as_str().or(t["url"].as_str()).unwrap_or("")),
            replaceable: true,
        },
    }))
}

fn manual(name: &str, url: &str) -> String {
    format!("codex mcp add {name} --url {url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32, stdout: &str) -> CmdOutput {
        CmdOutput { code: Some(code), stdout: stdout.into(), stderr: String::new() }
    }

    #[test]
    fn parses_get_json() {
        // 实测输出（隔离 CODEX_HOME，codex-cli 0.156.1）
        let http = r#"{"name":"app-mcp","enabled":true,"transport":{"type":"streamable_http","url":"http://127.0.0.1:7717/mcp","bearer_token_env_var":null}}"#;
        assert_eq!(parse_get(&out(0, http)).unwrap(), Some(Existing::http("http://127.0.0.1:7717/mcp")));
        let stdio = r#"{"name":"x","transport":{"type":"stdio","command":"npx","args":[]}}"#;
        let e = parse_get(&out(0, stdio)).unwrap().unwrap();
        assert_eq!(e.url, None);
        assert_eq!(e.summary, "stdio npx");
        let missing = CmdOutput { code: Some(1), stdout: String::new(), stderr: "Error: No MCP server named 'app-mcp' found.".into() };
        assert_eq!(parse_get(&missing).unwrap(), None);
        assert!(parse_get(&out(0, "not json")).is_err());
        assert!(parse_get(&out(2, "other failure")).is_err());
    }
}
