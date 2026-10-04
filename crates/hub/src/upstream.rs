//! 上游 MCP 服务器：Host 以子进程启动已有的 MCP 服务器（stdio），作为 MCP 客户端连接，
//! 把其工具以 `<name>.<tool>`、资源以 `app-mcp://<name>/<编码后的上游 URI>`（MCP Apps 界面资源为 `ui://<name>/…`，见 [`ui`]）聚合进来。
//!
//! 上游退出后标记为未连接，并按指数退避重启（500ms 起、×2、最大 30s；
//! 连续运行超过 30s 后重置退避计数）。

use std::collections::BTreeMap;
use std::path::Path;

use rmcp::model::{Resource, Tool};
use rmcp::{Peer, RoleClient};
use serde::{Deserialize, Serialize};

/// 子进程客户端与重启循环（feature `upstream`）。
#[cfg(feature = "upstream")]
mod client;
#[cfg(feature = "upstream")]
pub(crate) use client::run;
/// 上游资源的 Hub 侧 URI 与 MCP Apps 界面透传。
pub mod ui;

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

/// 上游连接成功时取得的信息（`initialize` 结果与首次列表）。
#[cfg(feature = "upstream")]
pub(crate) struct UpstreamHello {
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub instructions: Option<String>,
    pub server_name: Option<String>,
    pub mcp_apps: bool,
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
    /// 上游在 `initialize` 结果中声明了 MCP Apps 扩展（[`ui::MCP_APPS_EXTENSION`]）。
    pub mcp_apps: bool,
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

/// 本构建未包含上游聚合：`Hub::start` 已拒绝非空的 `upstreams`（`features::check_config`），
/// 这里只在万一被调用时把该上游记为失败，不静默忽略。
#[cfg(not(feature = "upstream"))]
pub(crate) async fn run(shared: std::sync::Arc<crate::hub::HubShared>, name: String, _config: UpstreamConfig) {
    shared.upstream_disconnected(
        &name,
        Some("本构建未包含上游聚合（app-mcp-hub 的 cargo feature `upstream` 未开启）".into()),
    );
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
