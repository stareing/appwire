//! 用户 / 厂商规则文件的检查：Agent 登记（agents.json）、策略规则（policy.json）与意图默认表（intents.json）。

use app_mcp_hub::HubStatus;
use serde_json::json;

use super::{Check, Level};
use crate::config::AppHome;

/// Agent 登记文件的读取结果（`agents_check` 的输入，便于测试）。
pub(super) struct AgentsFile {
    pub(super) path: String,
    pub(super) config: Result<app_mcp_hub::AgentsConfig, String>,
    /// 文件权限位（Unix，存在时）；其他平台与不存在时为 `None`。
    pub(super) mode: Option<u32>,
}

pub(super) fn read_agents_file(home: &AppHome) -> AgentsFile {
    let path = home.agents_file();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&path).ok().map(|m| m.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let mode = None;
    AgentsFile { path: path.display().to_string(), config: crate::agents::validate_file(&path), mode }
}

/// Agent 登记（第 16 项 N5，spec/hub-api.md 3.6「Agent 身份」）：登记的 Agent 与各自的任务数（只列名字，不含令牌）。
/// 文件不合法为错误；文件可被其他用户读取、文件与运行中的登记不一致为注意。
pub(super) fn agents_check(file: &AgentsFile, status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "Agent 登记";
    let st = match status {
        Some(Ok(st)) => Some(st),
        _ => None,
    };
    let running = st.and_then(|s| s.agents.clone());
    let file_names: Option<Vec<String>> = file.config.as_ref().ok().map(|c| {
        let mut n: Vec<String> = c.agents.iter().map(|a| a.name.clone()).collect();
        n.sort();
        n
    });
    let details = json!({
        "file": file.path,
        "fileAgents": file_names,
        "fileError": file.config.as_ref().err(),
        "running": running,
    });
    if let Err(e) = &file.config {
        let effect = if running.is_some() { "运行中的 Host 继续使用之前的登记" } else { "Host 启动时会因此失败" };
        return Check::new("agents", T, Level::Error, format!("登记文件无效（{effect}）：{e}"))
            .hint("修正或删除该文件后运行 app-mcp-host agent reload")
            .details(details);
    }
    if let Some(mode) = file.mode.filter(|m| m & 0o077 != 0) {
        return Check::new("agents", T, Level::Warn, format!("登记文件含令牌，但其他用户可读（权限 {mode:03o}）"))
            .hint(format!("chmod 600 {}", file.path))
            .details(details);
    }
    let names = file_names.unwrap_or_default();
    let Some(running) = running else {
        let summary = match (names.is_empty(), st.is_some()) {
            (true, _) => "未登记 Agent（所有 MCP 请求为本机主体）".to_owned(),
            (false, true) => format!("登记文件有 {} 个 Agent；运行中的 Host 版本不支持 Agent 登记", names.len()),
            (false, false) => format!("登记文件有 {} 个 Agent；Host 未运行或状态不可读，无法确认生效情况", names.len()),
        };
        let level = if names.is_empty() { Level::Ok } else { Level::Skip };
        return Check::new("agents", T, level, summary).details(details);
    };
    if running != names {
        return Check::new("agents", T, Level::Warn, format!("登记文件与运行中的登记不一致（改动尚未生效）。生效：{}", join_or_none(&running)))
            .hint("运行 app-mcp-host agent reload 使文件中的登记生效")
            .details(details);
    }
    if running.is_empty() {
        return Check::new("agents", T, Level::Ok, "未登记 Agent（所有 MCP 请求为本机主体）").details(details);
    }
    let tasks = st.and_then(|s| s.tasks.as_ref());
    let described: Vec<String> = running
        .iter()
        .map(|name| {
            let n = tasks.map_or(0, |t| t.iter().filter(|t| t.agent.as_deref() == Some(name.as_str())).count());
            format!("{name}（{n} 个任务）")
        })
        .collect();
    Check::new("agents", T, Level::Info, described.join("、")).details(details)
}

fn join_or_none(names: &[String]) -> String {
    if names.is_empty() { "无".to_owned() } else { names.join("、") }
}

/// 规则文件的校验结果（`policy_check` 的输入，便于测试）。
pub(super) fn validate_policy_file(home: &AppHome) -> (String, Result<app_mcp_hub::PolicyConfig, String>) {
    let path = home.policy_file();
    (path.display().to_string(), crate::policy::validate_file(&path))
}

/// 策略规则（spec/hub-api.md 3.13）：生效规则与命中次数；规则文件不合法、最近一次重载失败为错误；
/// 文件与生效规则不一致（改了文件没有 reload）为注意。
pub(super) fn policy_check(file: (String, Result<app_mcp_hub::PolicyConfig, String>), status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "策略规则";
    let (path, file) = file;
    let running = match status {
        Some(Ok(st)) => st.policy.as_ref(),
        _ => None,
    };
    let details = json!({
        "file": path,
        "fileError": file.as_ref().err(),
        "running": running,
    });
    if let Err(e) = &file {
        let effect = if running.is_some() { "运行中的 Host 继续使用之前的规则" } else { "Host 启动时会因此失败" };
        return Check::new("policy", T, Level::Error, format!("规则文件无效（{effect}）：{e}"))
            .hint("修正或删除该文件后运行 app-mcp-host policy reload；app-mcp-host policy validate 校验")
            .details(details);
    }
    let file_rules = file.as_ref().map(|c| c.rules.clone()).unwrap_or_default();
    let Some(st) = running else {
        let summary = if file_rules.is_empty() {
            "无规则（默认放行）；Host 未运行或状态不可读".to_owned()
        } else {
            format!("规则文件有 {} 条规则；Host 未运行或状态不可读，无法确认生效情况", file_rules.len())
        };
        return Check::new("policy", T, Level::Skip, summary).details(details);
    };
    if let Some(e) = &st.last_error {
        return Check::new("policy", T, Level::Error, format!("最近一次重载失败，之前的规则继续生效：{}", e.message))
            .hint("修正 policy.json 后运行 app-mcp-host policy reload")
            .details(details);
    }
    let effective: Vec<_> = st.rules.iter().map(|r| r.rule.clone()).collect();
    let summary = crate::policy::describe_status(st).replace('\n', "；");
    if effective != file_rules {
        return Check::new("policy", T, Level::Warn, format!("规则文件与运行中的规则不一致（改动尚未重载）。生效：{summary}"))
            .hint("运行 app-mcp-host policy reload 使文件中的规则生效")
            .details(details);
    }
    let level = if st.rules.is_empty() { Level::Ok } else { Level::Info };
    Check::new("policy", T, level, summary).details(details)
}

/// 意图默认表文件的校验结果（`intents_check` 的输入，便于测试）。
pub(super) fn validate_intents_file(home: &AppHome) -> (String, Result<app_mcp_hub::IntentsConfig, String>) {
    let path = home.intents_file();
    (path.display().to_string(), crate::intents::validate_file(&path))
}

/// 标准意图的默认 App（spec/intents.md 第 4 节）：文件不合法、最近一次重载失败为错误；文件与生效的默认表不一致为注意。
pub(super) fn intents_check(
    file: (String, Result<app_mcp_hub::IntentsConfig, String>),
    status: Option<&Result<HubStatus, String>>,
) -> Check {
    const T: &str = "意图默认 App";
    let (path, file) = file;
    let running = match status {
        Some(Ok(st)) => st.intents.as_ref(),
        _ => None,
    };
    let details = json!({ "file": path, "fileError": file.as_ref().err(), "running": running });
    let file = match file {
        Ok(c) => c,
        Err(e) => {
            let effect = if running.is_some() { "运行中的 Host 继续使用之前的默认表" } else { "Host 启动时会因此失败" };
            return Check::new("intents", T, Level::Error, format!("默认表文件无效（{effect}）：{e}"))
                .hint("修正或删除该文件后运行 app-mcp-host intents reload；app-mcp-host intents validate 校验")
                .details(details);
        }
    };
    let summary = crate::intents::describe(&file).replace('\n', "；");
    let Some(st) = running else {
        return Check::new("intents", T, Level::Skip, format!("{summary}；Host 未运行或版本不支持，无法确认生效情况")).details(details);
    };
    if let Some(e) = &st.last_error {
        return Check::new("intents", T, Level::Error, format!("最近一次重载失败，之前的默认表继续生效：{e}"))
            .hint("修正 intents.json 后运行 app-mcp-host intents reload")
            .details(details);
    }
    if st.defaults != file.defaults {
        return Check::new("intents", T, Level::Warn, format!("默认表文件与运行中的不一致（改动尚未重载）。文件：{summary}"))
            .hint("运行 app-mcp-host intents reload 使文件中的默认表生效")
            .details(details);
    }
    let level = if file.defaults.is_empty() { Level::Ok } else { Level::Info };
    Check::new("intents", T, level, summary).details(details)
}
