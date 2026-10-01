//! 只打印手动配置说明的 Agent（写入位置或格式无法从官方文档确认，P-08：不写文件）。
//!
//! - Windsurf：官方文档地址 docs.windsurf.com/windsurf/cascade/mcp 在 2026-10-02 重定向到 Devin Desktop 文档，
//!   其中的位置为 `~/.config/devin/mcp_config.json`（Windows `%APPDATA%\devin\mcp_config.json`），与第三方资料中的
//!   `~/.codeium/windsurf/mcp_config.json` 不一致；远程条目字段为 `serverUrl`。位置无法确认，只给说明。
//! - Claude Desktop：官方文档（modelcontextprotocol.io/docs/develop/connect-local-servers）中
//!   `claude_desktop_config.json` 只记载 `command` 形式（stdio）的服务器，没有 HTTP `url` 条目。

use std::path::PathBuf;

use super::{AgentEnv, AgentId, AgentSpec, Detect, Method};

pub const WINDSURF: AgentSpec = AgentSpec {
    id: AgentId::Windsurf,
    key: "windsurf",
    display: "Windsurf",
    evidence: "官方文档重定向到 Devin Desktop（~/.config/devin/mcp_config.json），与旧位置 ~/.codeium/windsurf 不一致：仅手动",
    detect: Detect::Explicit,
    config_file: no_file,
    method: Method::Manual,
    manual: windsurf_manual,
};

pub const CLAUDE_DESKTOP: AgentSpec = AgentSpec {
    id: AgentId::ClaudeDesktop,
    key: "claude-desktop",
    display: "Claude Desktop",
    evidence: "MCP 官方文档 connect-local-servers：claude_desktop_config.json 只记载 stdio（command）服务器：仅手动",
    detect: Detect::Explicit,
    config_file: no_file,
    method: Method::Manual,
    manual: claude_desktop_manual,
};

fn no_file(_: &AgentEnv) -> Option<PathBuf> {
    None
}

fn windsurf_manual(name: &str, url: &str) -> String {
    format!(
        "在 Windsurf 的 MCP 设置（Cascade → MCP，打开 mcp_config.json）的 \"mcpServers\" 中加入 \"{name}\": {{ \"serverUrl\": \"{url}\" }}"
    )
}

fn claude_desktop_manual(name: &str, url: &str) -> String {
    format!(
        "Claude Desktop 的 claude_desktop_config.json 只记载 stdio 服务器，AppWire 不自动配置；若你的版本支持添加 HTTP MCP 服务器，名称填 {name}、地址填 {url}"
    )
}
