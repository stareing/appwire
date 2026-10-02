//! Agent 登记文件 `<home>/agents.json`（第 16 项 N5；spec/hub-api.md 3.6「Agent 身份」）。
//!
//! - 每个 Agent 一个名字与令牌；MCP 客户端以 `Authorization: Bearer <令牌>` 连接 `/mcp` 时，Hub 按 Agent 区分主体
//!   （任务、任务句柄、`apps.select`、租约分开）。未登记任何 Agent 时行为与之前一致（所有请求为本机主体）。
//! - 启动（`serve` / `stdio`）时加载；文件不存在 = 无登记。文件不合法时拒绝启动（与 `policy.json` 相同）。
//! - `agent add / remove`：编辑文件（0600，临时文件 + rename），Host 在运行时随即替换登记（`POST /agents`）；
//!   手工编辑后用 `agent reload`。
//! - Agent 令牌只能访问 `/mcp`，不能读 `/status`、改策略或登记。
//!
//! @security 文件含令牌：只以 0600 写入；令牌只在 `add` / `token` 时打印到 stdout（便于 `$(…)` 取用），其他输出不含令牌。

use std::io;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use app_mcp_hub::{AgentCredential, AgentsConfig};

use crate::config::AppHome;

/// 读取并校验登记文件；不存在时为空登记。
///
/// @error 中文说明（读取失败、JSON 不合法、登记不合法），含文件路径，不含令牌。
pub fn validate_file(path: &Path) -> Result<AgentsConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => AgentsConfig::from_json(&text).map_err(|e| format!("{}：{e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(AgentsConfig::default()),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
    }
}

/// 启动时加载登记（`serve` / `stdio`）。
pub fn load(home: &AppHome) -> anyhow::Result<AgentsConfig> {
    validate_file(&home.agents_file())
        .map_err(anyhow::Error::msg)
        .context("Agent 登记文件无效，Host 不启动（修正或删除该文件后重试；app-mcp-host agent list 查看）")
}

fn write(path: &Path, config: &AgentsConfig) -> anyhow::Result<()> {
    crate::token::write_private(path, &(serde_json::to_string_pretty(config)? + "\n"))
}

/// 登记一个 Agent 并生成令牌；`rotate` 时为已登记的 Agent 换新令牌（旧令牌随即失效）。返回新令牌。
///
/// @error 名字不合法；已登记且未要求 `rotate`；要求 `rotate` 但未登记；文件不合法或写入失败。
pub fn add(path: &Path, name: &str, rotate: bool) -> anyhow::Result<String> {
    if !app_mcp_hub::agents::is_valid_agent_name(name) {
        anyhow::bail!(
            "Agent 名「{name}」不合法：需 1–{} 个字母、数字、-、_、.，以字母或数字开头",
            app_mcp_hub::agents::MAX_AGENT_NAME_LEN
        );
    }
    let mut config = validate_file(path).map_err(anyhow::Error::msg)?;
    let token = crate::token::generate();
    match (config.agents.iter_mut().find(|a| a.name == name), rotate) {
        (Some(a), true) => a.token.clone_from(&token),
        (Some(_), false) => anyhow::bail!("Agent「{name}」已登记；换新令牌用 --rotate，查看令牌用 app-mcp-host agent token {name}"),
        (None, true) => anyhow::bail!("Agent「{name}」未登记；登记用 app-mcp-host agent add {name}"),
        (None, false) => config.agents.push(AgentCredential { name: name.to_owned(), token: token.clone() }),
    }
    config.validate().map_err(anyhow::Error::msg)?;
    write(path, &config)?;
    Ok(token)
}

/// 删除一个 Agent；返回是否找到。
pub fn remove(path: &Path, name: &str) -> anyhow::Result<bool> {
    let mut config = validate_file(path).map_err(anyhow::Error::msg)?;
    let before = config.agents.len();
    config.agents.retain(|a| a.name != name);
    if config.agents.len() == before {
        return Ok(false);
    }
    write(path, &config)?;
    Ok(true)
}

/// 把登记文件交给运行中的 Host。`Ok(None)` = Host 未运行。
async fn push(home: &AppHome) -> anyhow::Result<Option<usize>> {
    let Some((reg, token)) = crate::running_host(home).await else {
        return Ok(None);
    };
    let config = validate_file(&home.agents_file()).map_err(anyhow::Error::msg)?;
    let listen = reg.listen.as_deref().map(crate::probe_addr);
    let reply = crate::probe::post_agents(reg.ipc_endpoint.as_deref(), listen.as_deref(), token.as_deref(), &serde_json::to_string(&config)?)
        .await
        .map_err(anyhow::Error::msg)?;
    match (reply.ok, reply.error) {
        (true, _) => Ok(Some(reply.agents.unwrap_or_default())),
        (false, e) => anyhow::bail!("Host 拒绝了新登记，继续使用之前的登记：{}", e.unwrap_or_else(|| "未给出原因".to_owned())),
    }
}

/// 编辑文件后：Host 在运行就替换登记，否则说明下次启动生效（提示写 stderr，stdout 只留令牌）。
async fn push_after_edit(home: &AppHome) -> anyhow::Result<()> {
    match push(home).await? {
        Some(n) => eprintln!("运行中的 Host 已更新 Agent 登记（{n} 个）"),
        None => eprintln!("Host 未运行，登记在下次启动时生效"),
    }
    Ok(())
}

/// `app-mcp-host agent …`。
pub async fn cmd(command: crate::cli::AgentCommand) -> anyhow::Result<ExitCode> {
    use crate::cli::AgentCommand as C;
    match command {
        C::Add { home, name, rotate } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let token = add(&home.agents_file(), &name, rotate)?;
            println!("{token}");
            let done = if rotate { format!("已为 Agent「{name}」换新令牌（旧令牌随即失效）") } else { format!("已登记 Agent「{name}」") };
            eprintln!("{done}：MCP 客户端连接 /mcp 时携带请求头 Authorization: Bearer <上面的令牌>");
            push_after_edit(&home).await?;
            Ok(ExitCode::SUCCESS)
        }
        C::Remove { home, name } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            if !remove(&home.agents_file(), &name)? {
                eprintln!("没有名为「{name}」的 Agent");
                return Ok(ExitCode::FAILURE);
            }
            eprintln!("已删除 Agent「{name}」（其令牌随即失效）");
            push_after_edit(&home).await?;
            Ok(ExitCode::SUCCESS)
        }
        C::Reload(home) => {
            let home = AppHome::resolve(home.home.as_deref())?;
            match push(&home).await? {
                Some(n) => {
                    println!("已重新加载 {}：{n} 个 Agent", home.agents_file().display());
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    println!("Host 未运行；登记在下次启动时加载");
                    Ok(ExitCode::from(crate::EXIT_NOT_RUNNING))
                }
            }
        }
        C::Token { home, name } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let config = validate_file(&home.agents_file()).map_err(anyhow::Error::msg)?;
            match config.agents.iter().find(|a| a.name == name) {
                Some(a) => {
                    println!("{}", a.token);
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    eprintln!("没有名为「{name}」的 Agent；登记用 app-mcp-host agent add {name}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        C::List { home, json } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let file = validate_file(&home.agents_file());
            let names: Option<Vec<&str>> = file.as_ref().ok().map(|c| c.agents.iter().map(|a| a.name.as_str()).collect());
            let running = match crate::running_host(&home).await {
                Some((reg, token)) => {
                    let listen = reg.listen.as_deref().map(crate::probe_addr);
                    Some(
                        crate::probe::fetch_status(reg.ipc_endpoint.as_deref(), listen.as_deref(), token.as_deref())
                            .await
                            .map(|st| st.agents),
                    )
                }
                None => None,
            };
            if json {
                let v = serde_json::json!({
                    "file": home.agents_file(),
                    "fileAgents": names,
                    "fileError": file.as_ref().err(),
                    "running": running.as_ref().map(|r| r.as_ref().ok().cloned().flatten()),
                    "runningError": running.as_ref().and_then(|r| r.as_ref().err()),
                });
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(ExitCode::SUCCESS);
            }
            match &names {
                Some(n) if n.is_empty() => println!("{}：未登记 Agent（所有请求为本机主体）", home.agents_file().display()),
                Some(n) => println!("{}：{}", home.agents_file().display(), n.join("、")),
                None => println!("登记文件无效：{}", file.as_ref().err().map_or("", String::as_str)),
            }
            match running {
                None => println!("Host 未运行"),
                Some(Err(e)) => println!("Host 运行中，但状态不可读：{e}"),
                Some(Ok(None)) => println!("运行中的 Host 版本不支持 Agent 登记"),
                Some(Ok(Some(r))) if r.is_empty() => println!("运行中的 Host：未登记 Agent"),
                Some(Ok(Some(r))) => println!("运行中的 Host：{}", r.join("、")),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> std::path::PathBuf {
        let n: u64 = rand::random();
        std::env::temp_dir().join(format!("app-mcp-agents-{tag}-{}-{n:x}", std::process::id())).join("agents.json")
    }

    #[test]
    fn add_rotate_remove() {
        let path = temp_file("crud");
        assert_eq!(validate_file(&path), Ok(AgentsConfig::default()), "不存在 = 空登记");

        let t1 = add(&path, "claude", false).unwrap();
        assert_eq!(t1.len(), 64);
        let t2 = add(&path, "cursor", false).unwrap();
        assert_ne!(t1, t2);
        let c = validate_file(&path).unwrap();
        assert_eq!(c.agents.iter().map(|a| (a.name.as_str(), a.token.as_str())).collect::<Vec<_>>(), [("claude", t1.as_str()), ("cursor", t2.as_str())]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        assert!(add(&path, "claude", false).unwrap_err().to_string().contains("已登记"));
        assert!(add(&path, "nobody", true).unwrap_err().to_string().contains("未登记"));
        assert!(add(&path, "bad name", false).unwrap_err().to_string().contains("不合法"));
        let t3 = add(&path, "claude", true).unwrap();
        assert_ne!(t3, t1);
        assert_eq!(validate_file(&path).unwrap().agents[0].token, t3);

        assert!(remove(&path, "claude").unwrap());
        assert!(!remove(&path, "claude").unwrap());
        assert_eq!(validate_file(&path).unwrap().agents.len(), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_file_is_reported_without_token() {
        let path = temp_file("bad");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let secret = "s".repeat(40);
        std::fs::write(&path, format!(r#"{{"agents":[{{"name":"a","token":"{secret}"}},{{"name":"a","token":"{secret}x"}}]}}"#)).unwrap();
        let e = validate_file(&path).unwrap_err();
        assert!(e.contains("重复") && !e.contains(&secret), "{e}");
        assert!(add(&path, "b", false).is_err(), "文件不合法时不改写");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
