//! 集成测试：`setup` / `uninstall` 的编排（公开 API + 假的服务操作与 Agent 命令）以及命令行入口（dry-run）。
//!
//! 不调用真实的 `claude` / `codex` / `gemini`、不安装真实服务、不碰用户的 Agent 配置：Agent 的配置目录全部指向临时目录，
//! 假 `claude` / `gemini` 按本机实测的行为（见 `src/setup/agents/*.rs` 的依据说明）读写临时目录中的配置文件。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use app_mcp_host::config::{AppHome, FileConfig, Overrides, Settings};
use app_mcp_host::doctor::Report;
use app_mcp_host::setup::agents::{AgentEnv, AgentId, AgentSelection};
use app_mcp_host::setup::ops::{CmdOutput, CommandRunner, HostOps, ServiceOutcome};
use app_mcp_host::setup::record::SetupRecord;
use app_mcp_host::setup::{self, AgentOutcome, SetupOptions, StepStatus, UninstallOptions};
use app_mcp_protocol::registry::EndpointRegistry;
use serde_json::{Value, json};

const URL: &str = "http://127.0.0.1:7717/mcp";

// ---------------------------------------------------------------------------
// 测试环境
// ---------------------------------------------------------------------------

struct Sandbox {
    root: PathBuf,
    home: AppHome,
    env: AgentEnv,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let n: u64 = rand::random();
        let root = std::env::temp_dir().join(format!("appwire-setup-{tag}-{}-{n:x}", std::process::id()));
        let home = AppHome { dir: root.join("app-home") };
        std::fs::create_dir_all(&home.dir).unwrap();
        let env = AgentEnv {
            user_home: root.join("user"),
            config_dir: root.join("user").join(".config"),
            claude_config_dir: Some(root.join("claude")),
            codex_home: Some(root.join("codex")),
            work_dir: home.dir.clone(),
        };
        std::fs::create_dir_all(&env.config_dir).unwrap();
        Self { root, home, env }
    }

    fn claude_file(&self) -> PathBuf {
        self.env.claude_config_dir.as_ref().unwrap().join(".claude.json")
    }
    fn cursor_file(&self) -> PathBuf {
        self.env.user_home.join(".cursor").join("mcp.json")
    }
    fn vscode_file(&self) -> PathBuf {
        self.env.config_dir.join("Code").join("User").join("mcp.json")
    }
    fn gemini_file(&self) -> PathBuf {
        self.env.user_home.join(".gemini").join("settings.json")
    }

    fn write(&self, path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn opts(&self, agents: AgentSelection, force: bool, dry_run: bool) -> SetupOptions {
        SetupOptions { home: self.home.clone(), agents, force, dry_run }
    }

    fn record(&self) -> Option<SetupRecord> {
        SetupRecord::load(&self.home.dir).unwrap()
    }

    fn backups(&self) -> usize {
        std::fs::read_dir(self.home.dir.join(setup::BACKUP_DIR)).map(|d| d.count()).unwrap_or(0)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn registry(listen: &str) -> EndpointRegistry {
    serde_json::from_value(json!({ "service": "app-mcp", "version": "test", "pid": 4242, "listen": listen, "startedAtMs": 0 }))
        .unwrap()
}

/// 假的服务操作：记录调用，不安装真实服务。
#[derive(Default)]
struct FakeHost {
    installs: AtomicUsize,
    uninstalls: AtomicUsize,
    /// 安装后未就绪（且没有运行中的实例）。
    not_ready: AtomicBool,
    installed_exe: Mutex<Option<PathBuf>>,
}

impl HostOps for FakeHost {
    fn settings(&self, home: &AppHome) -> anyhow::Result<Settings> {
        let file = FileConfig::load(&home.config_file(), false)?;
        Settings::resolve(&file, &Overrides::default(), home)
    }
    fn service_location(&self) -> String {
        "fake-service".into()
    }
    async fn install_service(&self, _home: &AppHome, exe: &Path, _restart: bool) -> anyhow::Result<ServiceOutcome> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        *self.installed_exe.lock().unwrap() = Some(exe.to_path_buf());
        let registry = if self.not_ready.load(Ordering::SeqCst) { Err("没有监听".into()) } else { Ok(registry("127.0.0.1:7717")) };
        Ok(ServiceOutcome { location: "fake-service".into(), messages: vec![], registry })
    }
    async fn uninstall_service(&self, _home: &AppHome) -> anyhow::Result<bool> {
        self.uninstalls.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
    async fn running(&self, _home: &AppHome) -> Option<EndpointRegistry> {
        (!self.not_ready.load(Ordering::SeqCst) && self.installs.load(Ordering::SeqCst) > 0).then(|| registry("127.0.0.1:7717"))
    }
    async fn doctor(&self, home: &AppHome) -> anyhow::Result<Report> {
        Ok(Report { version: "test", home: home.dir.display().to_string(), checks: vec![] })
    }
}

/// 假的 Agent 命令：`claude`（`mcp add|get|remove`，读写 `<claude_config_dir>/.claude.json`）与 `gemini`
/// （`mcp add|remove`，读写 `~/.gemini/settings.json`），行为按本机实测版本模拟。
struct FakeRunner {
    env: AgentEnv,
    programs: Vec<&'static str>,
    calls: Mutex<Vec<String>>,
    /// `claude mcp add` 成功返回但写入错误的 URL（模拟写入后校验失败）。
    claude_writes_wrong_url: AtomicBool,
}

impl FakeRunner {
    fn new(sb: &Sandbox, programs: &[&'static str]) -> Self {
        Self { env: sb.env.clone(), programs: programs.to_vec(), calls: Mutex::new(vec![]), claude_writes_wrong_url: AtomicBool::new(false) }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn claude(&self, args: &[String]) -> CmdOutput {
        let path = self.env.claude_config_dir.as_ref().unwrap().join(".claude.json");
        let mut doc: Value = std::fs::read_to_string(&path).ok().map(|t| serde_json::from_str(&t).unwrap()).unwrap_or(json!({}));
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let ok = |s: String| CmdOutput { code: Some(0), stdout: s, stderr: String::new() };
        let fail = |s: String| CmdOutput { code: Some(1), stdout: String::new(), stderr: s };
        match a.as_slice() {
            ["mcp", "get", name] => match doc["mcpServers"].get(*name) {
                None => fail(format!("No MCP server named \"{name}\". Configured servers: context7")),
                Some(e) if e["type"] == "http" => ok(format!(
                    "{name}:\n  Scope: User config (available in all your projects)\n  Status: ✔ Connected\n  Type: http\n  URL: {}\n",
                    e["url"].as_str().unwrap()
                )),
                Some(e) => ok(format!(
                    "{name}:\n  Scope: User config (available in all your projects)\n  Type: stdio\n  Command: {}\n",
                    e["command"].as_str().unwrap_or("")
                )),
            },
            ["mcp", "add", "--scope", "user", "--transport", "http", name, url] => {
                if doc["mcpServers"].get(*name).is_some() {
                    return fail(format!("MCP server {name} already exists in user config"));
                }
                let url = if self.claude_writes_wrong_url.load(Ordering::SeqCst) { "http://wrong/mcp" } else { url };
                if doc.get("mcpServers").is_none() {
                    doc["mcpServers"] = json!({});
                }
                doc["mcpServers"][*name] = json!({ "type": "http", "url": url });
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
                ok(format!("Added HTTP MCP server {name} with URL: {url} to user config"))
            }
            ["mcp", "remove", "--scope", "user", name] => {
                let Some(m) = doc.get_mut("mcpServers").and_then(Value::as_object_mut).filter(|m| m.contains_key(*name)) else {
                    return fail(format!("No MCP server named \"{name}\" in user scope"));
                };
                m.remove(*name);
                std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
                ok(format!("Removed MCP server {name} from user config"))
            }
            other => fail(format!("fake claude：未模拟的参数 {other:?}")),
        }
    }

    fn gemini(&self, args: &[String]) -> CmdOutput {
        let path = self.env.user_home.join(".gemini").join("settings.json");
        let mut doc: Value = std::fs::read_to_string(&path).ok().map(|t| serde_json::from_str(&t).unwrap()).unwrap_or(json!({}));
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        match a.as_slice() {
            ["mcp", "add", "--scope", "user", "--transport", "http", name, url] => {
                if doc.get("mcpServers").is_none() {
                    doc["mcpServers"] = json!({});
                }
                doc["mcpServers"][*name] = json!({ "url": url, "type": "http" });
            }
            ["mcp", "remove", "--scope", "user", name] => {
                if let Some(m) = doc.get_mut("mcpServers").and_then(Value::as_object_mut) {
                    m.remove(*name);
                }
            }
            other => return CmdOutput { code: Some(1), stdout: String::new(), stderr: format!("未模拟 {other:?}") },
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        CmdOutput { code: Some(0), stdout: String::new(), stderr: String::new() }
    }
}

impl CommandRunner for FakeRunner {
    fn which(&self, program: &str) -> Option<PathBuf> {
        self.programs.contains(&program).then(|| PathBuf::from("/fake/bin").join(program))
    }

    async fn run(&self, program: &Path, args: &[String], cwd: &Path) -> anyhow::Result<CmdOutput> {
        assert_eq!(cwd, self.env.work_dir, "Agent 命令应在配置目录中运行");
        let name = program.file_name().unwrap().to_string_lossy().into_owned();
        self.calls.lock().unwrap().push(format!("{name} {}", args.join(" ")));
        Ok(match name.as_str() {
            "claude" => self.claude(args),
            "gemini" => self.gemini(args),
            other => panic!("不应运行 {other}"),
        })
    }
}

fn exe() -> PathBuf {
    PathBuf::from("/opt/appwire/app-mcp-host")
}

fn outcome(r: &setup::SetupReport, id: AgentId) -> AgentOutcome {
    r.agents.iter().find(|a| a.agent == id).map(|a| a.outcome).unwrap_or_else(|| panic!("报告中没有 {id:?}"))
}

const CLAUDE_ORIGINAL: &str = "{\n  \"numStartups\": 3,\n  \"mcpServers\": {\n    \"context7\": { \"type\": \"stdio\", \"command\": \"npx\" }\n  }\n}\n";
const CURSOR_ORIGINAL: &str = "{\"theme\": \"dark\", \"mcpServers\": {\"other\": {\"url\": \"http://other/mcp\"}}}";

// ---------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn setup_writes_detected_agents_and_is_idempotent() {
    let sb = Sandbox::new("idem");
    sb.write(&sb.claude_file(), CLAUDE_ORIGINAL);
    sb.write(&sb.cursor_file(), CURSOR_ORIGINAL);
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &["claude"]);

    let r = setup::setup(&sb.opts(AgentSelection::All, false, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert!(r.ok(), "{}", r.render());
    assert_eq!(r.mcp_url.as_deref(), Some(URL));
    assert_eq!(r.binary.status, StepStatus::Unchanged, "不在包管理器目录：原地使用");
    assert_eq!(r.service.status, StepStatus::Done);
    assert_eq!(outcome(&r, AgentId::ClaudeCode), AgentOutcome::Added);
    assert_eq!(outcome(&r, AgentId::Cursor), AgentOutcome::Added);
    for id in [AgentId::Codex, AgentId::Gemini, AgentId::Vscode] {
        assert_eq!(outcome(&r, id), AgentOutcome::NotDetected);
    }
    assert!(!r.agents.iter().any(|a| a.agent == AgentId::Windsurf), "all 不含仅手动的 Agent");
    assert!(r.doctor.is_some());
    assert_eq!(runner.calls(), vec![
        "claude mcp get app-mcp".to_owned(),
        format!("claude mcp add --scope user --transport http app-mcp {URL}"),
        "claude mcp get app-mcp".to_owned(),
    ]);
    // 写入结果：只多了 app-mcp，其他条目保留
    let cursor: Value = serde_json::from_str(&std::fs::read_to_string(sb.cursor_file()).unwrap()).unwrap();
    assert_eq!(cursor, json!({"theme": "dark", "mcpServers": {"other": {"url": "http://other/mcp"}, "app-mcp": {"url": URL}}}));
    let claude: Value = serde_json::from_str(&std::fs::read_to_string(sb.claude_file()).unwrap()).unwrap();
    assert_eq!(claude["mcpServers"]["app-mcp"], json!({"type": "http", "url": URL}));
    assert_eq!(claude["mcpServers"]["context7"]["command"], "npx");
    // 清单与备份
    let rec = sb.record().expect("setup.json");
    assert_eq!(rec.agents.iter().map(|a| a.agent).collect::<Vec<_>>(), vec![AgentId::ClaudeCode, AgentId::Cursor]);
    assert!(rec.service.is_some());
    assert_eq!(sb.backups(), 2);
    assert_eq!(*host.installed_exe.lock().unwrap(), Some(app_mcp_host::service::background_exe_for(&exe())));

    // 再次执行：无写入、无新备份
    let r = setup::setup(&sb.opts(AgentSelection::All, false, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert!(r.ok());
    assert_eq!(outcome(&r, AgentId::ClaudeCode), AgentOutcome::Unchanged);
    assert_eq!(outcome(&r, AgentId::Cursor), AgentOutcome::Unchanged);
    assert_eq!(runner.calls().iter().filter(|c| c.contains(" add ")).count(), 1);
    assert_eq!(sb.backups(), 2);
    assert_eq!(sb.record().unwrap().agents.len(), 2);
}

#[tokio::test]
async fn conflicting_entry_needs_force_and_uninstall_restores_it() {
    let sb = Sandbox::new("force");
    let original = "{\"mcpServers\": {\"app-mcp\": {\"type\": \"http\", \"url\": \"http://127.0.0.1:9999/mcp\"}}}";
    sb.write(&sb.claude_file(), original);
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &["claude"]);
    let only_claude = AgentSelection::List(vec![AgentId::ClaudeCode]);

    let r = setup::setup(&sb.opts(only_claude.clone(), false, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert!(!r.ok());
    let a = &r.agents[0];
    assert_eq!(a.outcome, AgentOutcome::Conflict);
    assert!(a.detail.contains("--force"), "{}", a.detail);
    assert!(a.manual.as_deref().unwrap().contains(URL));
    assert_eq!(std::fs::read_to_string(sb.claude_file()).unwrap(), original, "冲突时不修改");
    assert!(!runner.calls().iter().any(|c| c.contains(" add ") || c.contains(" remove ")));

    let r = setup::setup(&sb.opts(only_claude, true, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert!(r.ok(), "{}", r.render());
    assert_eq!(r.agents[0].outcome, AgentOutcome::Replaced);
    let calls = runner.calls();
    let remove = calls.iter().position(|c| c == "claude mcp remove --scope user app-mcp").expect("先 remove");
    let add = calls.iter().position(|c| c.starts_with("claude mcp add")).expect("再 add");
    assert!(remove < add);
    assert!(sb.record().unwrap().agents[0].replaced.as_deref().unwrap().contains("9999"));

    // 文件自写入后未变化：卸载时整文件恢复（原条目回来）
    let u = setup::uninstall(&UninstallOptions { home: sb.home.clone(), purge: false, dry_run: false }, &host, &runner, &sb.env)
        .await
        .unwrap();
    assert!(u.ok(), "{}", u.render());
    assert_eq!(u.agents[0].outcome, AgentOutcome::Restored);
    assert_eq!(std::fs::read_to_string(sb.claude_file()).unwrap(), original);
}

#[tokio::test]
async fn uninstall_restores_bytes_or_removes_only_our_entry() {
    let sb = Sandbox::new("uninst");
    sb.write(&sb.claude_file(), CLAUDE_ORIGINAL);
    sb.write(&sb.cursor_file(), CURSOR_ORIGINAL);
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &["claude", "gemini"]);
    let r = setup::setup(&sb.opts(AgentSelection::All, false, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert!(r.ok(), "{}", r.render());
    assert_eq!(outcome(&r, AgentId::Gemini), AgentOutcome::Added);

    // 用户之后改了 Claude 的配置（加了别的键）：卸载时只删 app-mcp
    let mut claude: Value = serde_json::from_str(&std::fs::read_to_string(sb.claude_file()).unwrap()).unwrap();
    claude["numStartups"] = json!(4);
    std::fs::write(sb.claude_file(), serde_json::to_string_pretty(&claude).unwrap()).unwrap();

    let dry = setup::uninstall(&UninstallOptions { home: sb.home.clone(), purge: false, dry_run: true }, &host, &runner, &sb.env)
        .await
        .unwrap();
    assert!(dry.agents.iter().all(|a| a.outcome == AgentOutcome::Planned));
    assert_eq!(dry.service.status, StepStatus::Planned);
    assert_eq!(host.uninstalls.load(Ordering::SeqCst), 0);
    assert!(sb.record().is_some());

    let u = setup::uninstall(&UninstallOptions { home: sb.home.clone(), purge: false, dry_run: false }, &host, &runner, &sb.env)
        .await
        .unwrap();
    assert!(u.ok(), "{}", u.render());
    let by = |id| u.agents.iter().find(|a| a.agent == id).unwrap().outcome;
    assert_eq!(by(AgentId::Cursor), AgentOutcome::Restored);
    assert_eq!(by(AgentId::Gemini), AgentOutcome::Restored);
    assert_eq!(by(AgentId::ClaudeCode), AgentOutcome::Removed);
    assert_eq!(std::fs::read_to_string(sb.cursor_file()).unwrap(), CURSOR_ORIGINAL, "逐字节恢复");
    assert!(!sb.gemini_file().exists(), "setup 创建的文件恢复为不存在");
    let claude: Value = serde_json::from_str(&std::fs::read_to_string(sb.claude_file()).unwrap()).unwrap();
    assert_eq!(claude["numStartups"], 4, "用户的改动保留");
    assert!(claude["mcpServers"].get("app-mcp").is_none());
    assert_eq!(claude["mcpServers"]["context7"]["command"], "npx");
    assert_eq!(host.uninstalls.load(Ordering::SeqCst), 1);
    assert!(sb.record().is_none(), "全部撤销后删除 setup.json");
    assert!(!sb.home.dir.join(setup::BACKUP_DIR).exists(), "备份随之清理");

    // 再次卸载：没有清单，什么都不做
    let u = setup::uninstall(&UninstallOptions { home: sb.home.clone(), purge: false, dry_run: false }, &host, &runner, &sb.env)
        .await
        .unwrap();
    assert!(!u.record_found && u.agents.is_empty() && u.ok());
    assert_eq!(host.uninstalls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn verification_failure_rolls_back() {
    let sb = Sandbox::new("rollback");
    sb.write(&sb.claude_file(), CLAUDE_ORIGINAL);
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &["claude"]);
    runner.claude_writes_wrong_url.store(true, Ordering::SeqCst);
    let r = setup::setup(&sb.opts(AgentSelection::List(vec![AgentId::ClaudeCode]), false, false), &host, &runner, &sb.env, &exe())
        .await
        .unwrap();
    assert!(!r.ok());
    let a = &r.agents[0];
    assert_eq!(a.outcome, AgentOutcome::Failed);
    assert!(a.detail.contains("校验不符") && a.detail.contains("已恢复备份"), "{}", a.detail);
    assert_eq!(std::fs::read_to_string(sb.claude_file()).unwrap(), CLAUDE_ORIGINAL);
    assert!(sb.record().unwrap().agents.is_empty(), "失败的写入不记录");
}

#[tokio::test]
async fn non_standard_json_is_left_untouched() {
    let sb = Sandbox::new("jsonc");
    let text = "{\n  // 我的服务器\n  \"servers\": {}\n}\n";
    sb.write(&sb.vscode_file(), text);
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &[]);
    let r = setup::setup(&sb.opts(AgentSelection::All, false, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert_eq!(outcome(&r, AgentId::Vscode), AgentOutcome::Failed);
    let a = r.agents.iter().find(|a| a.agent == AgentId::Vscode).unwrap();
    assert!(a.manual.as_deref().unwrap().contains("\"type\": \"http\""));
    assert_eq!(std::fs::read_to_string(sb.vscode_file()).unwrap(), text);
}

#[tokio::test]
async fn dry_run_changes_nothing() {
    let sb = Sandbox::new("dry");
    sb.write(&sb.cursor_file(), CURSOR_ORIGINAL);
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &["claude"]);
    let sel = AgentSelection::List(vec![AgentId::ClaudeCode, AgentId::Cursor, AgentId::Windsurf, AgentId::ClaudeDesktop]);
    let r = setup::setup(&sb.opts(sel, false, true), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert_eq!(r.service.status, StepStatus::Planned);
    assert_eq!(outcome(&r, AgentId::ClaudeCode), AgentOutcome::Planned);
    assert_eq!(outcome(&r, AgentId::Cursor), AgentOutcome::Planned);
    assert_eq!(outcome(&r, AgentId::Windsurf), AgentOutcome::Manual);
    assert_eq!(outcome(&r, AgentId::ClaudeDesktop), AgentOutcome::Manual);
    assert!(r.doctor.is_none());
    assert!(r.mcp_url.as_deref().unwrap().starts_with(URL), "Host 未运行时按配置推测：{:?}", r.mcp_url);
    assert_eq!(host.installs.load(Ordering::SeqCst), 0);
    assert_eq!(runner.calls(), vec!["claude mcp get app-mcp".to_owned()], "只读");
    assert!(sb.record().is_none());
    assert_eq!(sb.backups(), 0);
    assert_eq!(std::fs::read_to_string(sb.cursor_file()).unwrap(), CURSOR_ORIGINAL);
    assert!(r.render().contains("--dry-run"));
}

#[tokio::test]
async fn host_not_ready_skips_agents() {
    let sb = Sandbox::new("notready");
    let host = FakeHost::default();
    host.not_ready.store(true, Ordering::SeqCst);
    let runner = FakeRunner::new(&sb, &["claude"]);
    let r = setup::setup(&sb.opts(AgentSelection::All, false, false), &host, &runner, &sb.env, &exe()).await.unwrap();
    assert!(!r.ok());
    assert_eq!(r.service.status, StepStatus::Failed);
    assert_eq!(r.mcp_url, None);
    assert!(r.agents.iter().all(|a| a.outcome == AgentOutcome::Skipped));
    assert!(runner.calls().is_empty(), "没有实际地址时不写任何 Agent");
}

#[tokio::test]
async fn auth_all_does_not_write_tokens_into_agents() {
    let sb = Sandbox::new("auth");
    std::fs::write(sb.home.config_file(), r#"{"http":{"auth":"all"}}"#).unwrap();
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &["claude"]);
    let r = setup::setup(&sb.opts(AgentSelection::List(vec![AgentId::ClaudeCode]), false, false), &host, &runner, &sb.env, &exe())
        .await
        .unwrap();
    let a = &r.agents[0];
    assert_eq!(a.outcome, AgentOutcome::Skipped);
    assert!(a.manual.as_deref().unwrap().contains("Authorization: Bearer"));
    assert!(runner.calls().is_empty());
}

#[tokio::test]
async fn package_managed_binary_is_copied_and_purged() {
    let sb = Sandbox::new("bin");
    let src_dir = sb.root.join("npx").join("node_modules").join("appwire-cli-x").join("bin");
    std::fs::create_dir_all(&src_dir).unwrap();
    let src = src_dir.join("app-mcp-host");
    std::fs::write(&src, b"binary").unwrap();
    if cfg!(windows) {
        std::fs::write(app_mcp_host::service::background_exe_for(&src), b"bg").unwrap();
    }
    let host = FakeHost::default();
    let runner = FakeRunner::new(&sb, &[]);
    let r = setup::setup(&sb.opts(AgentSelection::None, false, false), &host, &runner, &sb.env, &src).await.unwrap();
    assert!(r.ok(), "{}", r.render());
    assert_eq!(r.binary.status, StepStatus::Done);
    let stable = sb.home.dir.join("bin").join("app-mcp-host");
    assert_eq!(std::fs::read(&stable).unwrap(), b"binary");
    assert_eq!(*host.installed_exe.lock().unwrap(), Some(app_mcp_host::service::background_exe_for(&stable)), "服务指向稳定位置");
    let r = setup::setup(&sb.opts(AgentSelection::None, false, false), &host, &runner, &sb.env, &src).await.unwrap();
    assert_eq!(r.binary.status, StepStatus::Unchanged);

    let u = setup::uninstall(&UninstallOptions { home: sb.home.clone(), purge: true, dry_run: false }, &host, &runner, &sb.env)
        .await
        .unwrap();
    assert!(u.ok(), "{}", u.render());
    assert_eq!(u.binary.status, StepStatus::Done);
    assert!(!sb.home.dir.join("bin").exists());
    assert!(sb.record().is_none());
}

// ---------------------------------------------------------------------------
// 命令行（真实进程；只用 dry-run / 无清单的 uninstall，不安装服务、不运行任何 Agent 命令）
// ---------------------------------------------------------------------------

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");

fn cli(sb: &Sandbox, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .arg("--home")
        .arg(&sb.home.dir)
        .env_remove("APP_MCP_HOME")
        // 主目录 / 配置目录指向临时目录：即便误入非 dry-run 路径也碰不到用户的真实配置
        .env("HOME", &sb.env.user_home)
        .env("XDG_CONFIG_HOME", &sb.env.config_dir)
        .env("PATH", sb.root.join("empty-path"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn cli_dry_run_and_uninstall_without_record() {
    let sb = Sandbox::new("cli");
    std::fs::write(sb.home.config_file(), r#"{"listen":"127.0.0.1:0","ipcEndpoint":"none"}"#).unwrap();
    let out = cli(&sb, &["setup", "--dry-run", "--json", "--agents", "none"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).expect("JSON 输出");
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["service"]["status"], "planned");
    assert_eq!(v["agents"], json!([]));
    assert!(!sb.home.dir.join("setup.json").exists());
    assert!(!sb.home.dir.join("bin").exists());

    let out = cli(&sb, &["setup", "--dry-run", "--agents", "windsurf"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("serverUrl"), "{text}");

    let out = cli(&sb, &["uninstall", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["recordFound"], false);
    assert_eq!(v["service"]["status"], "skipped", "没有清单时不卸载服务");

    let out = cli(&sb, &["setup", "--agents", "nope"]);
    assert_eq!(out.status.code(), Some(2));
}
