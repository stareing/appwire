//! 已安装 MCP Agent 的配置写入：策略表 [`AGENTS`]，每种 Agent 一个模块。
//!
//! 写入方式按核实程度分三类（`docs/plans/13-out-of-box.md` U1）：
//! - [`Method::Cli`]：用 Agent 自己的命令写入 / 删除（Claude Code、Codex CLI、Gemini CLI），参数以本机安装版本的 `--help`
//!   与隔离配置目录下的实测为准；
//! - [`Method::File`]：按官方文档的文件格式写 JSON（Cursor、VS Code），文件不是标准 JSON（含注释等）时不写；
//! - [`Method::Manual`]：位置或格式无法从官方文档确认（Windsurf、Claude Desktop），只打印手动配置说明。
//!
//! @invariant 只按条目名 [`MCP_SERVER_NAME`] 读写，不碰其他条目。

pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod gemini;
pub mod json_servers;
pub mod manual;
pub mod vscode;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use json_servers::JsonServers;

use super::ops::{CmdOutput, CommandRunner};

/// 写入各 Agent 的 MCP 服务器条目名（与仓库 `.mcp.json`、`service install` 的提示一致）。
pub const MCP_SERVER_NAME: &str = "app-mcp";

/// Agent 标识（`--agents` 取值、`setup.json` 中的 `agent`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AgentId {
    ClaudeCode,
    Codex,
    Gemini,
    Cursor,
    Vscode,
    Windsurf,
    ClaudeDesktop,
}

impl AgentId {
    pub fn as_str(self) -> &'static str {
        spec(self).key
    }
}

/// Agent 配置所在的用户目录（真实环境由 [`AgentEnv::from_system`] 取得；测试指向临时目录）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEnv {
    /// 用户主目录。
    pub user_home: PathBuf,
    /// 平台配置目录（Linux `~/.config`，macOS `~/Library/Application Support`，Windows `%APPDATA%`）。
    pub config_dir: PathBuf,
    /// `CLAUDE_CONFIG_DIR`（Claude Code 的配置目录，未设置时为主目录）。
    pub claude_config_dir: Option<PathBuf>,
    /// `CODEX_HOME`（Codex 的配置目录，未设置时为 `~/.codex`）。
    pub codex_home: Option<PathBuf>,
    /// 运行 Agent 命令的工作目录：用不含 `.mcp.json` 的目录（配置目录），避免命令读到某个项目作用域的条目。
    pub work_dir: PathBuf,
}

impl AgentEnv {
    pub fn from_system(work_dir: &Path) -> anyhow::Result<Self> {
        let env_dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        Ok(Self {
            user_home: dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法确定用户主目录"))?,
            config_dir: dirs::config_dir().ok_or_else(|| anyhow::anyhow!("无法确定用户配置目录"))?,
            claude_config_dir: env_dir("CLAUDE_CONFIG_DIR"),
            codex_home: env_dir("CODEX_HOME"),
            work_dir: work_dir.to_path_buf(),
        })
    }
}

/// Agent 配置中已有的同名条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Existing {
    /// HTTP 条目的 URL；其他类型（stdio、SSE……）为 `None`。
    pub url: Option<String>,
    /// 给用户看的说明。
    pub summary: String,
    /// 能否由本程序替换（Claude Code 中位于用户作用域之外的条目不能）。
    pub replaceable: bool,
}

impl Existing {
    pub fn http(url: &str) -> Self {
        Self { url: Some(url.to_owned()), summary: format!("HTTP {url}"), replaceable: true }
    }
}

/// 检测 Agent 是否已安装。
#[derive(Clone, Copy)]
pub enum Detect {
    /// PATH 中有该命令。
    Program(&'static str),
    /// 该目录存在（Agent 安装后创建）。
    Dir(fn(&AgentEnv) -> PathBuf),
    /// 无法可靠检测：只在 `--agents` 显式列出时处理。
    Explicit,
}

/// 经 Agent 命令写入时，读取现有条目的方式。
#[derive(Clone, Copy)]
pub enum CliRead {
    /// 运行 `program <args(name)>`，由 `parse` 解析输出。
    Command { args: fn(&str) -> Vec<String>, parse: fn(&CmdOutput) -> Result<Option<Existing>, String> },
    /// 直接读取其配置文件（[`AgentSpec::config_file`]）。
    File(JsonServers),
}

#[derive(Clone, Copy)]
pub struct CliSpec {
    pub program: &'static str,
    pub add: fn(name: &str, url: &str) -> Vec<String>,
    pub remove: fn(name: &str) -> Vec<String>,
    /// `add` 遇到同名条目时报错（需要先 `remove`）；否则 `add` 直接覆盖。
    pub replace_needs_remove: bool,
    pub read: CliRead,
}

#[derive(Clone, Copy)]
pub enum Method {
    Cli(CliSpec),
    File(JsonServers),
    Manual,
}

/// 策略表的一行。
#[derive(Clone, Copy)]
pub struct AgentSpec {
    pub id: AgentId,
    /// `--agents` 中的名字。
    pub key: &'static str,
    pub display: &'static str,
    /// 写入方式的依据（版本 / 文档地址），写进报告。
    pub evidence: &'static str,
    pub detect: Detect,
    /// 被修改的配置文件（备份与卸载时整文件恢复用）。
    pub config_file: fn(&AgentEnv) -> Option<PathBuf>,
    pub method: Method,
    /// 手动配置说明（冲突、失败、仅手动时打印）。
    pub manual: fn(name: &str, url: &str) -> String,
}

/// 策略表（顺序即处理与输出顺序）。
pub const AGENTS: &[AgentSpec] = &[
    claude_code::SPEC,
    codex::SPEC,
    gemini::SPEC,
    cursor::SPEC,
    vscode::SPEC,
    manual::WINDSURF,
    manual::CLAUDE_DESKTOP,
];

pub fn spec(id: AgentId) -> &'static AgentSpec {
    match id {
        AgentId::ClaudeCode => &claude_code::SPEC,
        AgentId::Codex => &codex::SPEC,
        AgentId::Gemini => &gemini::SPEC,
        AgentId::Cursor => &cursor::SPEC,
        AgentId::Vscode => &vscode::SPEC,
        AgentId::Windsurf => &manual::WINDSURF,
        AgentId::ClaudeDesktop => &manual::CLAUDE_DESKTOP,
    }
}

/// `--agents` 的取值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentSelection {
    /// 检测到的全部（默认）。
    All,
    None,
    List(Vec<AgentId>),
}

impl AgentSelection {
    /// 本次要处理的 Agent 及是否为显式指定。
    pub fn specs(&self) -> Vec<(&'static AgentSpec, bool)> {
        match self {
            Self::None => Vec::new(),
            Self::All => AGENTS.iter().filter(|s| !matches!(s.detect, Detect::Explicit)).map(|s| (s, false)).collect(),
            Self::List(ids) => AGENTS.iter().filter(|s| ids.contains(&s.id)).map(|s| (s, true)).collect(),
        }
    }
}

/// 解析 `--agents`：`all` / `none` / 逗号分隔的列表。
pub fn parse_selection(s: &str) -> Result<AgentSelection, String> {
    let s = s.trim();
    match s {
        "all" => return Ok(AgentSelection::All),
        "none" => return Ok(AgentSelection::None),
        _ => {}
    }
    let known = || AGENTS.iter().map(|a| a.key).collect::<Vec<_>>().join("、");
    let ids = s
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            AGENTS
                .iter()
                .find(|a| a.key == p)
                .map(|a| a.id)
                .ok_or_else(|| format!("未知的 Agent「{p}」；可选：all、none、{}", known()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err(format!("应为 all、none 或逗号分隔的列表（{}）", known()));
    }
    Ok(AgentSelection::List(ids))
}

/// 检测结果：`Some(程序路径)` 表示已安装（非命令行方式时为空路径）。
pub fn detect(spec: &AgentSpec, env: &AgentEnv, runner: &impl CommandRunner) -> Option<PathBuf> {
    match spec.detect {
        Detect::Program(p) => runner.which(p),
        Detect::Dir(f) => f(env).is_dir().then(PathBuf::new),
        Detect::Explicit => None,
    }
}

fn program_of(spec: &AgentSpec, runner: &impl CommandRunner) -> anyhow::Result<Option<(CliSpec, PathBuf)>> {
    let Method::Cli(cli) = spec.method else {
        return Ok(None);
    };
    let path = runner
        .which(cli.program)
        .ok_or_else(|| anyhow::anyhow!("PATH 中没有 {}", cli.program))?;
    Ok(Some((cli, path)))
}

fn config_path(spec: &AgentSpec, env: &AgentEnv) -> anyhow::Result<PathBuf> {
    (spec.config_file)(env).ok_or_else(|| anyhow::anyhow!("{} 没有可写的配置文件", spec.display))
}

async fn run_checked(runner: &impl CommandRunner, program: &Path, args: &[String], env: &AgentEnv) -> anyhow::Result<CmdOutput> {
    let out = runner.run(program, args, &env.work_dir).await?;
    if !out.success() {
        anyhow::bail!("{} {} 失败（退出码 {:?}）：{}", program.display(), args.join(" "), out.code, out.text().trim());
    }
    Ok(out)
}

/// 读取同名条目。
pub async fn read_existing(spec: &AgentSpec, env: &AgentEnv, runner: &impl CommandRunner, name: &str) -> anyhow::Result<Option<Existing>> {
    match spec.method {
        Method::Manual => anyhow::bail!("{} 只支持手动配置", spec.display),
        Method::File(js) => js.read(&config_path(spec, env)?, name),
        Method::Cli(cli) => match cli.read {
            CliRead::File(js) => js.read(&config_path(spec, env)?, name),
            CliRead::Command { args, parse } => {
                let (_, program) = program_of(spec, runner)?.ok_or_else(|| anyhow::anyhow!("不是命令行方式"))?;
                let out = runner.run(&program, &args(name), &env.work_dir).await?;
                parse(&out).map_err(|e| anyhow::anyhow!("读取 {} 的 MCP 配置失败：{e}", spec.display))
            }
        },
    }
}

/// 写入条目；`replacing` = 已有不同的同名条目（`--force` 或本程序以前写入的）。
pub async fn write(spec: &AgentSpec, env: &AgentEnv, runner: &impl CommandRunner, name: &str, url: &str, replacing: bool) -> anyhow::Result<()> {
    match spec.method {
        Method::Manual => anyhow::bail!("{} 只支持手动配置", spec.display),
        Method::File(js) => js.insert(&config_path(spec, env)?, name, url),
        Method::Cli(_) => {
            let Some((cli, program)) = program_of(spec, runner)? else {
                anyhow::bail!("不是命令行方式");
            };
            if replacing && cli.replace_needs_remove {
                run_checked(runner, &program, &(cli.remove)(name), env).await?;
            }
            run_checked(runner, &program, &(cli.add)(name, url), env).await.map(|_| ())
        }
    }
}

/// 删除条目（不存在时视为成功）。
pub async fn remove(spec: &AgentSpec, env: &AgentEnv, runner: &impl CommandRunner, name: &str) -> anyhow::Result<()> {
    match spec.method {
        Method::Manual => anyhow::bail!("{} 只支持手动配置", spec.display),
        Method::File(js) => js.remove(&config_path(spec, env)?, name).map(|_| ()),
        Method::Cli(_) => {
            let Some((cli, program)) = program_of(spec, runner)? else {
                anyhow::bail!("不是命令行方式");
            };
            run_checked(runner, &program, &(cli.remove)(name), env).await.map(|_| ())
        }
    }
}

/// 按行取 `Key: value` 形式的值（Claude Code 的 `mcp get` 输出）。
pub(crate) fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix(key)?.strip_prefix(':'))
        .map(str::trim)
        .next()
}

/// 输出中含 `needle`（不区分大小写）。
pub(crate) fn mentions(out: &CmdOutput, needle: &str) -> bool {
    out.text().to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_all_ids() {
        use clap::ValueEnum;
        for id in AgentId::value_variants() {
            let s = spec(*id);
            assert_eq!(s.id, *id);
            assert!(AGENTS.iter().any(|a| a.id == *id), "策略表缺少 {id:?}");
            // --agents 的名字与 clap / serde 的 kebab-case 一致
            assert_eq!(id.to_possible_value().unwrap().get_name(), s.key);
            assert_eq!(serde_json::to_value(id).unwrap(), serde_json::Value::String(s.key.into()));
            assert!(!s.evidence.is_empty());
            assert!((s.manual)(MCP_SERVER_NAME, "http://127.0.0.1:7717/mcp").contains("http://127.0.0.1:7717/mcp"));
        }
    }

    #[test]
    fn selection_parsing() {
        assert_eq!(parse_selection("all"), Ok(AgentSelection::All));
        assert_eq!(parse_selection("none"), Ok(AgentSelection::None));
        assert_eq!(
            parse_selection("claude-code, cursor"),
            Ok(AgentSelection::List(vec![AgentId::ClaudeCode, AgentId::Cursor]))
        );
        assert!(parse_selection("claude").is_err());
        assert!(parse_selection(",").is_err());
        // all 不含无法检测的手动项；显式列出时包含
        assert!(!AgentSelection::All.specs().iter().any(|(s, _)| s.id == AgentId::Windsurf));
        let l = parse_selection("windsurf").unwrap().specs();
        assert_eq!(l.len(), 1);
        assert!(l[0].1);
    }

    #[test]
    fn field_lines() {
        let t = "app-mcp:\n  Scope: User config (available in all your projects)\n  Type: http\n  URL: http://127.0.0.1:7717/mcp\n";
        assert_eq!(field(t, "URL"), Some("http://127.0.0.1:7717/mcp"));
        assert_eq!(field(t, "Type"), Some("http"));
        assert_eq!(field(t, "Command"), None);
    }
}
