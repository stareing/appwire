//! 上游 MCP 服务器：Host 以子进程启动已有的 MCP 服务器（stdio），作为 MCP 客户端连接，
//! 把其工具以 `<name>.<tool>`、资源以 `app-mcp://<name>/<编码后的上游 URI>` 聚合进来。
//!
//! 上游退出后标记为未连接，并按指数退避重启（500ms 起、×2、最大 30s；
//! 连续运行超过 30s 后重置退避计数）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, Resource, Tool};
use rmcp::service::NotificationContext;
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, Peer, RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};

use crate::hub::HubShared;

const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const STABLE_AFTER: Duration = Duration::from_secs(30);
const INIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 一个上游 MCP 服务器的启动方式。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// `--config` 文件格式。
#[derive(Clone, Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    upstreams: BTreeMap<String, UpstreamConfig>,
}

/// 读取 `--config` 文件中的 `upstreams`。
pub fn load_config_file(path: &Path) -> Result<BTreeMap<String, UpstreamConfig>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取配置 {} 失败：{e}", path.display()))?;
    let file: ConfigFile = serde_json::from_str(&text)
        .map_err(|e| format!("解析配置 {} 失败：{e}", path.display()))?;
    Ok(file.upstreams)
}

/// 解析 `--upstream <name>=<命令行>`。
pub fn parse_cli_spec(spec: &str) -> Result<(String, UpstreamConfig), String> {
    let (name, cmdline) = spec
        .split_once('=')
        .ok_or_else(|| format!("--upstream 格式应为 <name>=<命令行>：{spec}"))?;
    let mut words = split_command_line(cmdline)?.into_iter();
    let command = words
        .next()
        .ok_or_else(|| format!("--upstream {name} 缺少命令"))?;
    Ok((
        name.trim().to_owned(),
        UpstreamConfig {
            command,
            args: words.collect(),
            env: BTreeMap::new(),
        },
    ))
}

/// 按空白切分命令行，支持单引号、双引号与反斜杠转义（双引号内）。
pub fn split_command_line(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("命令行中的单引号没有闭合".into()),
                    }
                }
            }
            '"' => {
                has_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\')) => cur.push(ch),
                            Some(ch) => {
                                cur.push('\\');
                                cur.push(ch);
                            }
                            None => return Err("命令行中的双引号没有闭合".into()),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("命令行中的双引号没有闭合".into()),
                    }
                }
            }
            c if c.is_whitespace() => {
                if has_word {
                    out.push(std::mem::take(&mut cur));
                    has_word = false;
                }
            }
            c => {
                has_word = true;
                cur.push(c);
            }
        }
    }
    if has_word {
        out.push(cur);
    }
    Ok(out)
}

/// 上游的运行状态。
#[derive(Debug, Default)]
pub struct UpstreamState {
    pub config: UpstreamConfig,
    /// 已连接时为 `Some`。
    pub peer: Option<Peer<RoleClient>>,
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    /// 上游 `initialize` 结果中的 `instructions`。
    pub instructions: Option<String>,
    /// 上游自报的名称（`serverInfo.title` 或 `serverInfo.name`）。
    pub server_name: Option<String>,
    pub restarts: u32,
    pub last_error: Option<String>,
}

impl UpstreamState {
    pub fn new(config: UpstreamConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    pub fn connected(&self) -> bool {
        self.peer.is_some()
    }
}

/// 资源 URI 路径部分的百分号编码（保留 RFC 3986 unreserved 字符）。
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [`encode_uri_component`] 的逆操作；非法编码返回 `None`。
pub fn decode_uri_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 作为上游客户端的回调：列表变化时刷新缓存并转发 list_changed。
struct UpstreamClient {
    name: String,
    shared: Weak<HubShared>,
}

impl ClientHandler for UpstreamClient {
    async fn on_tool_list_changed(&self, context: NotificationContext<RoleClient>) {
        if let Some(shared) = self.shared.upgrade() {
            match context.peer.list_all_tools().await {
                Ok(tools) => shared.set_upstream_tools(&self.name, tools),
                Err(e) => tracing::warn!(upstream = %self.name, "刷新上游工具列表失败：{e}"),
            }
        }
    }

    async fn on_resource_list_changed(&self, context: NotificationContext<RoleClient>) {
        if let Some(shared) = self.shared.upgrade() {
            match context.peer.list_all_resources().await {
                Ok(resources) => shared.set_upstream_resources(&self.name, resources),
                Err(e) => tracing::warn!(upstream = %self.name, "刷新上游资源列表失败：{e}"),
            }
        }
    }

    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("app-mcp-host", env!("CARGO_PKG_VERSION")),
        )
    }
}

/// 启动一个上游并在退出后按退避重启，直到任务被中止。
pub(crate) async fn run(shared: Arc<HubShared>, name: String, config: UpstreamConfig) {
    let mut failures: u32 = 0;
    loop {
        let started = tokio::time::Instant::now();
        match connect(&shared, &name, &config).await {
            Ok(service) => {
                let peer = service.peer().clone();
                let info = service.peer_info();
                let caps = info
                    .as_ref()
                    .map(|i| i.capabilities.clone())
                    .unwrap_or_default();
                let tools = if caps.tools.is_some() {
                    peer.list_all_tools().await.unwrap_or_else(|e| {
                        tracing::warn!(upstream = %name, "获取上游工具列表失败：{e}");
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                let resources = if caps.resources.is_some() {
                    peer.list_all_resources().await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                let instructions = info.as_ref().and_then(|i| i.instructions.clone());
                let server_name = info
                    .as_ref()
                    .and_then(|i| i.server_info.as_ref())
                    .map(|s| s.title.clone().unwrap_or_else(|| s.name.clone()));
                tracing::info!(upstream = %name, tools = tools.len(), resources = resources.len(), "上游 MCP 服务器已连接");
                shared.upstream_connected(&name, peer, tools, resources, instructions, server_name);
                match service.waiting().await {
                    Ok(reason) => {
                        tracing::warn!(upstream = %name, ?reason, "上游 MCP 服务器已断开")
                    }
                    Err(e) => tracing::warn!(upstream = %name, "上游 MCP 服务器任务异常：{e}"),
                }
                shared.upstream_disconnected(&name, None);
            }
            Err(e) => {
                tracing::error!(upstream = %name, "启动上游 MCP 服务器失败：{e}");
                shared.upstream_disconnected(&name, Some(e));
            }
        }
        if started.elapsed() >= STABLE_AFTER {
            failures = 0;
        }
        let delay = INITIAL_BACKOFF
            .saturating_mul(1u32 << failures.min(10))
            .min(MAX_BACKOFF);
        failures = failures.saturating_add(1);
        tokio::time::sleep(delay).await;
    }
}

async fn connect(
    shared: &Arc<HubShared>,
    name: &str,
    config: &UpstreamConfig,
) -> Result<rmcp::service::RunningService<RoleClient, UpstreamClient>, String> {
    let mut cmd = tokio::process::Command::new(&config.command);
    cmd.args(&config.args).envs(&config.env).kill_on_drop(true);
    // 常驻 Host 没有控制台：上游（多为 npx / python 等控制台程序）不弹出窗口。
    #[cfg(windows)]
    cmd.creation_flags(crate::CREATE_NO_WINDOW);
    // 子进程的 stderr 继承 Host 的 stderr；stdout 专用于与 Host 的 MCP 通信。
    let transport =
        TokioChildProcess::new(cmd).map_err(|e| format!("无法启动 {}：{e}", config.command))?;
    let client = UpstreamClient {
        name: name.to_owned(),
        shared: Arc::downgrade(shared),
    };
    match tokio::time::timeout(INIT_TIMEOUT, client.serve(transport)).await {
        Ok(Ok(service)) => Ok(service),
        Ok(Err(e)) => Err(format!("MCP 初始化失败：{e}")),
        Err(_) => Err("MCP 初始化超时".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_splitting() {
        assert_eq!(split_command_line("a b  c").unwrap(), ["a", "b", "c"]);
        assert_eq!(
            split_command_line(r#"node "my server.js" --x='1 2'"#).unwrap(),
            ["node", "my server.js", "--x=1 2"]
        );
        assert_eq!(split_command_line(r#""a\"b" ''"#).unwrap(), ["a\"b", ""]);
        assert!(split_command_line("'open").is_err());
        assert!(split_command_line("").unwrap().is_empty());
    }

    #[test]
    fn cli_spec() {
        let (name, cfg) = parse_cli_spec("files=npx -y @scope/server /tmp").unwrap();
        assert_eq!(name, "files");
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, ["-y", "@scope/server", "/tmp"]);
        assert!(parse_cli_spec("noequals").is_err());
        assert!(parse_cli_spec("x=  ").is_err());
    }

    #[test]
    fn config_file() {
        let path =
            std::env::temp_dir().join(format!("app-mcp-upstream-cfg-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"upstreams": {"files": {"command": "srv", "args": ["-v"], "env": {"A": "1"}}, "min": {"command": "x"}}}"#,
        )
        .unwrap();
        let ups = load_config_file(&path).unwrap();
        assert_eq!(ups["files"].args, ["-v"]);
        assert_eq!(ups["files"].env["A"], "1");
        assert!(ups["min"].args.is_empty());
        std::fs::write(&path, "{").unwrap();
        assert!(load_config_file(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn uri_component_roundtrip() {
        for s in ["file:///tmp/a b.txt", "demo://greeting", "中文?x=1&y=%"] {
            let e = encode_uri_component(s);
            assert!(!e.contains('/'), "{e}");
            assert_eq!(decode_uri_component(&e).as_deref(), Some(s));
        }
        assert_eq!(decode_uri_component("%zz"), None);
        assert_eq!(decode_uri_component("%4"), None);
    }
}
