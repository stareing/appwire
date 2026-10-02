//! `app-mcp-host doctor`：逐项检查本机的 app-mcp 环境，每项给出结论与修复建议（中文）；`--json` 输出机器可读结果。
//!
//! 检查项：Host 运行 / 版本 / 身份、单实例锁、运行时目录权限、本地 IPC 端点（路径、权限、所有者、可连通）、
//! 监听端口与备选端口（空闲 / 本 Host / 其他 app-mcp / 其他进程及其 pid 与名称）、Windows 排除端口段、防火墙说明、
//! 令牌与鉴权模式、各 App 实例状态与最近错误、各工具的声明（risk 与 MCP 注解）、资源保护（限流 / 大小上限与拒绝次数）、
//! 网页 SDK 的拦截上报、Android `adb reverse`、按名寻址的名字服务（`naming.*`，见 [`naming`]）。
//!
//! 只读：不加锁（锁状态从 `/proc/locks` 或锁文件中的进程号推断）、不修改任何文件。

use std::path::Path;

use app_mcp_protocol::registry::{EndpointRegistry, LOCK_FILE};
use app_mcp_protocol::{ConnectionErrorCode, LISTEN_CANDIDATE_PORTS};
use serde_json::{Value, json};

use crate::config::{AppHome, Settings};
use crate::ports;
use crate::probe;

mod checks_apps;
mod checks_host;
mod checks_policy;
mod checks_usage;
pub(crate) mod command;
mod naming;
mod port_scan;
mod report;

use checks_apps::{apps_check, dormant_store_check, lease_check, limits_check, reports_check, tools_check, wake_check};
use checks_host::{adb_check, auth_check, excluded_check, ipc_check, run_dir_check};
use checks_policy::{agents_check, policy_check, read_agents_file, validate_policy_file};
use checks_usage::usage_check;
pub(crate) use checks_apps::callers_text;
pub(crate) use checks_host::find_in_path;
pub use port_scan::{PortPreflight, PortState, candidate_addrs, describe_port_state, port_owner, port_preflight, port_state};
pub use report::{Check, Level, Report};

/// `/proc/locks` 中对 inode 为 `inode` 的文件持有 `FLOCK` 的进程号。
pub fn parse_proc_locks(text: &str, inode: u64) -> Option<u32> {
    let suffix = format!(":{inode}");
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().filter(|t| *t != "->").collect();
        // `1: FLOCK ADVISORY WRITE <pid> <maj>:<min>:<inode> <start> <end>`
        if f.get(1) != Some(&"FLOCK") || !f.get(5)?.ends_with(&suffix) {
            return None;
        }
        f.get(4)?.parse().ok()
    })
}

/// 锁的持有者：`(pid, 依据)`。
fn lock_holder(path: &Path) -> Option<(u32, &'static str)> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let inode = std::fs::metadata(path).ok()?.ino();
        let locks = std::fs::read_to_string("/proc/locks").ok()?;
        parse_proc_locks(&locks, inode).map(|pid| (pid, "/proc/locks"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let pid: u32 = std::fs::read_to_string(path).ok()?.trim().parse().ok()?;
        ports::process_alive(pid).then_some((pid, "锁文件中的进程号（进程存活，推断）"))
    }
}

/// 运行全部检查。
pub async fn run(home: &AppHome, s: &Settings) -> Report {
    let mut checks = Vec::new();
    let registry = EndpointRegistry::read(&home.registry_file());
    let running = crate::running_instance(home).await;
    let token = std::fs::read_to_string(home.token_file()).ok().map(|t| t.trim().to_owned());
    let status = match &running {
        Some(reg) => Some(
            probe::fetch_status(
                reg.ipc_endpoint.as_deref(),
                reg.listen.as_deref().map(crate::probe_addr).as_deref(),
                token.as_deref(),
            )
            .await,
        ),
        None => None,
    };

    // 1. Host
    checks.push(match (&running, &registry) {
        (Some(reg), _) => {
            let id = &reg.identity;
            Check::new(
                "host",
                "Host 运行状态",
                Level::Ok,
                format!(
                    "在运行：pid {}，版本 {}，用户 {}；HTTP {}，本地 IPC {}",
                    id.pid,
                    id.version,
                    id.user.as_deref().unwrap_or("未知"),
                    reg.listen.as_deref().unwrap_or("未开启"),
                    reg.ipc_endpoint.as_deref().unwrap_or("未开启"),
                ),
            )
            .details(serde_json::to_value(reg).unwrap_or(Value::Null))
        }
        (None, Ok(Some(reg))) => Check::new(
            "host",
            "Host 运行状态",
            Level::Error,
            format!(
                "登记文件 {} 记录 pid {}，但其地址 {} 上不是该进程：Host 已异常退出或卡住",
                home.registry_file().display(),
                reg.identity.pid,
                reg.listen.as_deref().unwrap_or("-")
            ),
        )
        .code(ConnectionErrorCode::HostNotRunning)
        .hint("重新启动 Host：app-mcp-host service start（或 serve）；日志见 ".to_owned() + &home.log_dir().display().to_string()),
        (None, Ok(None)) => Check::new("host", "Host 运行状态", Level::Error, "未运行（没有登记文件）")
            .code(ConnectionErrorCode::HostNotRunning),
        (None, Err(e)) => Check::new("host", "Host 运行状态", Level::Error, format!("登记文件无法读取：{e}"))
            .hint("删除损坏的登记文件后重新启动 Host"),
    });

    // 2. 单实例锁
    let lock_path = home.run_dir().join(LOCK_FILE);
    checks.push(if !lock_path.exists() {
        Check::new("lock", "单实例锁", Level::Info, format!("{} 不存在（本配置目录从未启动过 Host）", lock_path.display()))
    } else {
        match (lock_holder(&lock_path), &running) {
            (Some((pid, how)), Some(reg)) if pid == reg.identity.pid => Check::new(
                "lock",
                "单实例锁",
                Level::Ok,
                format!("由运行中的 Host 持有（pid {pid}，依据：{how}）"),
            )
            .hint("无需处理；重复执行 serve 会打印该实例的信息并以退出码 0 结束"),
            (Some((pid, how)), _) => Check::new(
                "lock",
                "单实例锁",
                Level::Warn,
                format!(
                    "被 pid {pid}（{}）持有（依据：{how}），但它没有在登记的地址上服务：可能正在启动或卡住",
                    ports::process_name(pid).unwrap_or_else(|| "未知进程".into())
                ),
            )
            .code(ConnectionErrorCode::LockHeld)
            .hint(format!("稍等片刻再运行 doctor；仍如此时结束 pid {pid} 后重新启动 Host")),
            (None, Some(_)) => Check::new(
                "lock",
                "单实例锁",
                Level::Info,
                "未能确定持有者（本平台无法读取锁信息，或锁文件中没有进程号）",
            ),
            (None, None) => Check::new("lock", "单实例锁", Level::Ok, "未被持有（没有 Host 在运行）"),
        }
    });

    // 3. 运行时目录
    checks.push(run_dir_check(&home.run_dir()));

    // 4. 本地 IPC 端点
    let ipc_endpoint = running
        .as_ref()
        .map(|r| r.ipc_endpoint.clone())
        .unwrap_or_else(|| s.ipc_endpoint.clone());
    checks.push(ipc_check(ipc_endpoint.as_deref(), running.is_some(), status.as_ref()));

    // 5. 监听端口
    let own_pid = running.as_ref().map(|r| r.identity.pid);
    let actual = running.as_ref().and_then(|r| r.listen.clone()).map(|a| crate::probe_addr(&a));
    let mut addrs = candidate_addrs(s);
    if let Some(a) = &actual
        && !addrs.contains(a)
    {
        addrs.insert(0, a.clone());
    }
    let mut port_details = Vec::new();
    let mut lines = Vec::new();
    for addr in &addrs {
        let st = port_state(addr, own_pid).await;
        lines.push(describe_port_state(addr, &st));
        port_details.push(json!({ "addr": addr, "state": st }));
    }
    checks.push(match (&running, &actual) {
        (Some(_), Some(a)) => Check::new("ports", "监听端口", Level::Ok, format!("Host 监听 {a}；{}", lines.join("；"))),
        (Some(_), None) => Check::new("ports", "监听端口", Level::Info, format!("Host 未开启 TCP 服务；{}", lines.join("；"))),
        (None, _) if port_details.is_empty() => {
            Check::new("ports", "监听端口", Level::Info, format!("配置的监听地址 {} 由系统随机分配端口", s.listen))
        }
        (None, _) => {
            let all_busy = port_details.iter().all(|d| d["state"]["kind"] != "free");
            let level = if all_busy { Level::Error } else if port_details.iter().any(|d| d["state"]["kind"] != "free") { Level::Warn } else { Level::Ok };
            let mut c = Check::new("ports", "监听端口", level, lines.join("；"));
            if level != Level::Ok {
                c = c.code(ConnectionErrorCode::PortBusy);
            }
            c
        }
    }
    .details(json!(port_details)));

    // 6. Windows 排除端口段
    checks.push(excluded_check(&addrs, s.listen_explicit));

    // 7. 防火墙
    checks.push(if s.http_allow_remote {
        Check::new(
            "firewall",
            "防火墙",
            Level::Warn,
            "已允许远程访问（--http-allow-remote）：端口对其他机器开放，需由防火墙限制来源",
        )
        .hint("如非必要，关闭 http.allowRemote；需要时只对可信网段放行，并使用 --auth all")
    } else {
        Check::new(
            "firewall",
            "防火墙",
            Level::Info,
            "只监听回环地址：回环连接不经过 Windows 防火墙 / ufw 等入站规则，无需放行端口；Android 真机经 adb reverse 转发，同样不需要",
        )
    });

    // 8. 令牌与鉴权
    checks.push(auth_check(home, s));

    // 9. App 实例
    checks.push(apps_check(status.as_ref()));
    checks.push(wake_check(status.as_ref()));
    checks.push(dormant_store_check(&home.state_dir(), status.as_ref()));
    checks.push(lease_check(status.as_ref()));
    checks.push(tools_check(status.as_ref()));
    checks.push(limits_check(status.as_ref()));
    checks.push(policy_check(validate_policy_file(home), status.as_ref()));
    checks.push(agents_check(&read_agents_file(home), status.as_ref()));
    checks.push(usage_check(status.as_ref()));

    // 10. 网页拦截上报
    checks.push(reports_check(status.as_ref()));

    // 11. Android adb reverse
    let host_port = actual
        .as_deref()
        .or(addrs.first().map(String::as_str))
        .and_then(|a| a.rsplit_once(':'))
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(LISTEN_CANDIDATE_PORTS[0]);
    checks.push(adb_check(host_port).await);

    // 12. 按名寻址（spec/naming.md 第 11 节）
    checks.extend(naming::run(&naming::NamingEnv::from_system()).await);

    Report { version: env!("CARGO_PKG_VERSION"), home: home.dir.display().to_string(), checks }
}

#[cfg(test)]
mod tests;
