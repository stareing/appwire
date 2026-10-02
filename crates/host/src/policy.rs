//! 策略规则文件 `<home>/policy.json`（spec/hub-api.md 3.13；docs/plans/16-agent-os.md P2、18-user-loop.md L5）。
//!
//! - 启动（`serve` / `stdio`）时加载；文件不存在 = 无规则。文件不合法时拒绝启动（不在无规则的情况下静默放行）。
//! - `app-mcp-host policy reload`：把文件原文交给运行中的 Host（`POST /policy`），Host 校验不合法时保留之前的规则，
//!   错误记入 `/status`（`doctor` 报出）。
//! - `hide` / `deny` / `remove`：编辑文件（临时文件 + rename），Host 在运行时随即重载。
//! - Host 不调用外部程序做判断；需要回调时由厂商嵌入 Hub 实现（`ApprovalHandler`）。

use std::io;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use app_mcp_hub::{PolicyAction, PolicyConfig, PolicyHook, PolicyRule, PolicyStatus};

use crate::config::AppHome;

/// 读取规则文件原文；不存在时为 `None`。
fn read_text(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// 解析并校验规则文件；不存在时为空规则集。
///
/// @error 中文说明（读取失败、JSON 不合法、规则不合法），含文件路径。
pub fn validate_file(path: &Path) -> Result<PolicyConfig, String> {
    match read_text(path) {
        Ok(None) => Ok(PolicyConfig::default()),
        Ok(Some(text)) => PolicyConfig::from_json(&text).map_err(|e| format!("{}：{e}", path.display())),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
    }
}

/// 启动时加载规则（`serve` / `stdio`）。
pub fn load(home: &AppHome) -> anyhow::Result<PolicyConfig> {
    validate_file(&home.policy_file())
        .map_err(anyhow::Error::msg)
        .context("策略规则文件无效，Host 不启动（运行 app-mcp-host policy validate 查看，修正或删除该文件后重试）")
}

/// 写入规则文件（临时文件 + rename，不留下半写的文件）。
fn write(path: &Path, config: &PolicyConfig) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(config)? + "\n";
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {} 失败", path.display()))
}

/// 由动作与目标生成规则 id（`hide-shop`、`deny-shop-cart.add`、按 Agent 时 `deny-shop-cart.add-for-cursor`）：
/// 只保留 id 允许的字符，`*` 写作 `any`。
pub fn default_id(action: PolicyAction, app: &str, tool: Option<&str>, agent: Option<&str>) -> String {
    let verb = match action {
        PolicyAction::Hide => "hide",
        PolicyAction::Deny => "deny",
    };
    let part = |s: &str| -> String {
        s.replace('*', "any")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            .collect()
    };
    let mut id = match tool {
        Some(t) => format!("{verb}-{}-{}", part(app), part(t)),
        None => format!("{verb}-{}", part(app)),
    };
    if let Some(a) = agent {
        id.push_str(&format!("-for-{}", part(a)));
    }
    id.chars().take(64).collect()
}

/// 在规则文件末尾加一条规则（先校验整个规则集）。
pub fn add_rule(path: &Path, rule: PolicyRule) -> anyhow::Result<PolicyConfig> {
    let mut config = validate_file(path).map_err(anyhow::Error::msg)?;
    if config.rules.iter().any(|r| r.id == rule.id) {
        anyhow::bail!("已有 id 为「{}」的规则；用 --id 指定其他标识，或先 app-mcp-host policy remove {}", rule.id, rule.id);
    }
    config.rules.push(rule);
    config.validate().map_err(anyhow::Error::msg)?;
    write(path, &config)?;
    Ok(config)
}

/// 删除规则；返回是否找到。
pub fn remove_rule(path: &Path, id: &str) -> anyhow::Result<bool> {
    let mut config = validate_file(path).map_err(anyhow::Error::msg)?;
    let before = config.rules.len();
    config.rules.retain(|r| r.id != id);
    if config.rules.len() == before {
        return Ok(false);
    }
    write(path, &config)?;
    Ok(true)
}

/// 一条规则的一行描述（`policy show`、`doctor`）。
pub fn describe_rule(rule: &PolicyRule) -> String {
    let action = match rule.action {
        PolicyAction::Hide => "hide",
        PolicyAction::Deny => "deny",
    };
    let mut target = format!("app={}", rule.app);
    if let Some(t) = &rule.tool {
        target.push_str(&format!(" tool={t}"));
    }
    if let Some(a) = &rule.annotations {
        target.push_str(&format!(" annotations={}", serde_json::to_string(a).unwrap_or_default()));
    }
    if let Some(a) = &rule.agent {
        target.push_str(&format!(" agent={a}"));
    }
    let hooks = match (&rule.hooks, rule.action) {
        (Some(h), _) => format!("（{}）", h.iter().map(|h| h.as_str()).collect::<Vec<_>>().join(", ")),
        (None, PolicyAction::Deny) => "（call）".to_owned(),
        (None, PolicyAction::Hide) => String::new(),
    };
    format!("{}：{action}{hooks} {target}", rule.id)
}

/// 生效规则的多行描述。
pub fn describe_status(st: &PolicyStatus) -> String {
    if st.rules.is_empty() {
        return "无规则（默认放行）".to_owned();
    }
    st.rules
        .iter()
        .map(|r| format!("{}，命中 {} 次", describe_rule(&r.rule), r.hits))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 把规则文件交给运行中的 Host。`Ok(None)` = Host 未运行。
async fn push(home: &AppHome) -> anyhow::Result<Option<usize>> {
    let Some((reg, token)) = crate::running_host(home).await else {
        return Ok(None);
    };
    let text = read_text(&home.policy_file())
        .with_context(|| format!("读取 {} 失败", home.policy_file().display()))?
        .unwrap_or_else(|| "{}".to_owned());
    let listen = reg.listen.as_deref().map(crate::probe_addr);
    let reply = crate::probe::post_policy(reg.ipc_endpoint.as_deref(), listen.as_deref(), token.as_deref(), &text)
        .await
        .map_err(anyhow::Error::msg)?;
    match (reply.ok, reply.error) {
        (true, _) => Ok(Some(reply.rules.unwrap_or_default())),
        (false, e) => anyhow::bail!(
            "Host 拒绝了新规则，继续使用之前的规则：{}",
            e.unwrap_or_else(|| "未给出原因".to_owned())
        ),
    }
}

/// 编辑文件后：Host 在运行就重载，否则说明下次启动生效。
async fn push_after_edit(home: &AppHome) -> anyhow::Result<ExitCode> {
    match push(home).await? {
        Some(n) => println!("运行中的 Host 已重载规则（{n} 条）"),
        None => println!("Host 未运行，规则在下次启动时生效"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `app-mcp-host policy …`。
pub async fn cmd(command: crate::cli::PolicyCommand) -> anyhow::Result<ExitCode> {
    use crate::cli::PolicyCommand as C;
    match command {
        C::Validate { home, file } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let path = file.unwrap_or_else(|| home.policy_file());
            match validate_file(&path) {
                Ok(c) => {
                    println!("{} 合法：{} 条规则", path.display(), c.rules.len());
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("{e}");
                    Ok(ExitCode::FAILURE)
                }
            }
        }
        C::Reload(home) => {
            let home = AppHome::resolve(home.home.as_deref())?;
            match push(&home).await? {
                Some(n) => {
                    println!("已重载 {}：{n} 条规则生效", home.policy_file().display());
                    Ok(ExitCode::SUCCESS)
                }
                None => {
                    println!("Host 未运行；规则在下次启动时加载");
                    Ok(ExitCode::from(crate::EXIT_NOT_RUNNING))
                }
            }
        }
        C::Show { home, json } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let file = validate_file(&home.policy_file());
            let running = match crate::running_host(&home).await {
                Some((reg, token)) => {
                    let listen = reg.listen.as_deref().map(crate::probe_addr);
                    Some(
                        crate::probe::fetch_status(reg.ipc_endpoint.as_deref(), listen.as_deref(), token.as_deref())
                            .await
                            .map(|st| st.policy),
                    )
                }
                None => None,
            };
            if json {
                let v = serde_json::json!({
                    "file": home.policy_file(),
                    "fileRules": file.as_ref().ok(),
                    "fileError": file.as_ref().err(),
                    "running": running.as_ref().map(|r| r.as_ref().ok().cloned().flatten()),
                    "runningError": running.as_ref().and_then(|r| r.as_ref().err()),
                });
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(ExitCode::SUCCESS);
            }
            match &file {
                Ok(c) => println!("{}：{} 条规则", home.policy_file().display(), c.rules.len()),
                Err(e) => println!("规则文件无效：{e}"),
            }
            match running {
                None => {
                    if let Ok(c) = &file {
                        for r in &c.rules {
                            println!("  {}", describe_rule(r));
                        }
                    }
                    println!("Host 未运行");
                }
                Some(Err(e)) => println!("Host 运行中，但状态不可读：{e}"),
                Some(Ok(None)) => println!("运行中的 Host 版本不支持策略规则"),
                Some(Ok(Some(st))) => {
                    println!("运行中的 Host 生效的规则：");
                    for line in describe_status(&st).lines() {
                        println!("  {line}");
                    }
                    if let Some(e) = &st.last_error {
                        println!("最近一次重载失败（之前的规则继续生效）：{}", e.message);
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        C::Hide(args) => {
            let home = AppHome::resolve(args.home.home.as_deref())?;
            let id = args.id.unwrap_or_else(|| default_id(PolicyAction::Hide, &args.app, args.tool.as_deref(), None));
            let rule = PolicyRule { id, action: PolicyAction::Hide, app: args.app, tool: args.tool, annotations: None, agent: None, hooks: None };
            let line = describe_rule(&rule);
            add_rule(&home.policy_file(), rule)?;
            println!("已添加 {line}");
            push_after_edit(&home).await
        }
        C::Deny { rule: args, wake, agent } => {
            let home = AppHome::resolve(args.home.home.as_deref())?;
            let id = args.id.unwrap_or_else(|| default_id(PolicyAction::Deny, &args.app, args.tool.as_deref(), agent.as_deref()));
            let hooks = wake.then(|| vec![PolicyHook::Call, PolicyHook::Wake]);
            let rule = PolicyRule { id, action: PolicyAction::Deny, app: args.app, tool: args.tool, annotations: None, agent, hooks };
            let line = describe_rule(&rule);
            add_rule(&home.policy_file(), rule)?;
            println!("已添加 {line}");
            push_after_edit(&home).await
        }
        C::Remove { home, id } => {
            let home = AppHome::resolve(home.home.as_deref())?;
            if !remove_rule(&home.policy_file(), &id)? {
                eprintln!("没有 id 为「{id}」的规则（app-mcp-host policy show 查看）");
                return Ok(ExitCode::FAILURE);
            }
            println!("已删除规则 {id}");
            push_after_edit(&home).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> AppHome {
        let dir = std::env::temp_dir().join(format!("app-mcp-policy-{tag}-{}-{:08x}", std::process::id(), rand_tag()));
        std::fs::create_dir_all(&dir).unwrap();
        AppHome { dir }
    }

    fn rand_tag() -> u32 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos())
    }

    #[test]
    fn missing_file_is_empty_policy() {
        let home = temp_home("missing");
        assert!(load(&home).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&home.dir);
    }

    #[test]
    fn invalid_file_fails_load() {
        let home = temp_home("invalid");
        std::fs::write(home.policy_file(), r#"{"rules": [{"id": "x", "action": "hide", "app": "a*b"}]}"#).unwrap();
        let e = format!("{:#}", load(&home).unwrap_err());
        assert!(e.contains("不启动") && e.contains("末尾"), "{e}");
        std::fs::write(home.policy_file(), "not json").unwrap();
        assert!(validate_file(&home.policy_file()).unwrap_err().contains("JSON"));
        let _ = std::fs::remove_dir_all(&home.dir);
    }

    #[test]
    fn edit_rules() {
        let home = temp_home("edit");
        let path = home.policy_file();
        let id = default_id(PolicyAction::Hide, "shop", Some("admin.*"), None);
        assert_eq!(id, "hide-shop-admin.any");
        assert_eq!(default_id(PolicyAction::Deny, "shop", Some("pay"), Some("bot-*")), "deny-shop-pay-for-bot-any");
        let by_agent = PolicyRule {
            id: "d".into(), action: PolicyAction::Deny, app: "shop".into(), tool: Some("pay".into()), annotations: None,
            agent: Some("cursor".into()), hooks: None,
        };
        assert!(describe_rule(&by_agent).contains("deny（call） app=shop tool=pay agent=cursor"), "{}", describe_rule(&by_agent));
        let rule = |id: &str| PolicyRule {
            id: id.into(), action: PolicyAction::Hide, app: "shop".into(), tool: Some("admin.*".into()), annotations: None, agent: None, hooks: None,
        };
        add_rule(&path, rule(&id)).unwrap();
        assert!(add_rule(&path, rule(&id)).unwrap_err().to_string().contains("已有"));
        let bad = PolicyRule { id: "bad".into(), action: PolicyAction::Hide, app: "a b".into(), tool: None, annotations: None, agent: None, hooks: None };
        assert!(add_rule(&path, bad).is_err());
        let c = load(&home).unwrap();
        assert_eq!(c.rules.len(), 1, "不合法的规则不写入");
        assert!(describe_rule(&c.rules[0]).contains("hide app=shop tool=admin.*"));
        assert!(remove_rule(&path, &id).unwrap());
        assert!(!remove_rule(&path, &id).unwrap());
        assert!(load(&home).unwrap().is_empty());
        assert_eq!(default_id(PolicyAction::Deny, "*", None, None), "deny-any");
        let _ = std::fs::remove_dir_all(&home.dir);
    }
}
