//! `app-mcp-host doctor`：逐项检查本机的 app-mcp 环境，每项给出结论与修复建议（中文）；`--json` 输出机器可读结果。
//!
//! 检查项：Host 运行 / 版本 / 身份、单实例锁、运行时目录权限、本地 IPC 端点（路径、权限、所有者、可连通）、
//! 监听端口与备选端口（空闲 / 本 Host / 其他 app-mcp / 其他进程及其 pid 与名称）、Windows 排除端口段、防火墙说明、
//! 令牌与鉴权模式、各 App 实例状态与最近错误、各工具的声明（risk 与 MCP 注解）、资源保护（限流 / 大小上限与拒绝次数）、
//! 网页 SDK 的拦截上报、Android `adb reverse`。
//!
//! 只读：不加锁（锁状态从 `/proc/locks` 或锁文件中的进程号推断）、不修改任何文件。

use std::path::Path;
use std::time::Duration;

use app_mcp_hub::{AppState, AwakeReason, HubStatus, InstancePower, LeaseStatus, ToolAnnotations};
use app_mcp_protocol::registry::{EndpointRegistry, LOCK_FILE};
use app_mcp_protocol::{ConnectionErrorCode, ErrorKind, LISTEN_CANDIDATE_PORTS};
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{AppHome, AuthMode, Settings};
use crate::ports::{self, PortOwner};
use crate::probe::{self, Probe};

/// 检查结果的级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Error,
    Skip,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Ok => "正常",
            Level::Info => "信息",
            Level::Warn => "注意",
            Level::Error => "错误",
            Level::Skip => "跳过",
        }
    }
}

/// 一项检查。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub status: Level,
    /// 结论。
    pub summary: String,
    /// 修复建议。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// 相关的错误码（spec/protocol.md 10.1）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    pub details: Value,
}

impl Check {
    fn new(id: &'static str, title: &'static str, status: Level, summary: impl Into<String>) -> Self {
        Self { id, title, status, summary: summary.into(), hint: None, code: None, details: Value::Null }
    }
    fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
    fn code(mut self, c: ConnectionErrorCode) -> Self {
        self.code = Some(c.as_str());
        if self.hint.is_none() {
            self.hint = Some(c.hint().to_owned());
        }
        self
    }
    fn details(mut self, d: Value) -> Self {
        self.details = d;
        self
    }
}

/// 诊断报告。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// 本程序版本。
    pub version: &'static str,
    pub home: String,
    pub checks: Vec<Check>,
}

impl Report {
    /// 有 `error` 级别的检查。
    pub fn has_errors(&self) -> bool {
        self.checks.iter().any(|c| c.status == Level::Error)
    }

    /// 人类可读的输出。
    pub fn render(&self) -> String {
        let mut out = format!("app-mcp-host doctor {}（配置目录 {}）\n", self.version, self.home);
        for c in &self.checks {
            out.push_str(&format!("\n[{}] {}\n  结论：{}\n", c.status.label(), c.title, c.summary));
            if let Some(h) = &c.hint {
                out.push_str(&format!("  建议：{h}\n"));
            }
            if let Some(code) = c.code {
                out.push_str(&format!("  错误码：{code}\n"));
            }
        }
        let count = |l: Level| self.checks.iter().filter(|c| c.status == l).count();
        out.push_str(&format!(
            "\n共 {} 项：错误 {}，注意 {}，正常 {}\n",
            self.checks.len(),
            count(Level::Error),
            count(Level::Warn),
            count(Level::Ok)
        ));
        out
    }
}

/// 一个端口上的情况。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PortState {
    Free,
    /// app-mcp Host（`own` = 本配置目录登记的实例）。
    AppMcp { pid: u32, user: Option<String>, own: bool },
    /// 其他程序。
    Other { description: String, owner: Option<PortOwner> },
}

/// 探测 `addr`（`host:port`）上的情况；`own_pid` 为本配置目录运行中实例的进程号。
pub async fn port_state(addr: &str, own_pid: Option<u32>) -> PortState {
    match probe::probe(addr).await {
        Probe::Free => PortState::Free,
        Probe::AppMcp(h) => PortState::AppMcp {
            pid: h.identity.pid,
            user: h.identity.user.clone(),
            own: own_pid == Some(h.identity.pid),
        },
        Probe::Other(description) => PortState::Other { description, owner: port_owner(addr) },
    }
}

/// 监听 `addr` 端口的进程（查询失败时为 `None`）。
pub fn port_owner(addr: &str) -> Option<PortOwner> {
    let port = addr.rsplit_once(':')?.1.parse().ok()?;
    ports::listening_owner(port).ok().flatten()
}

/// 端口占用者的一句话说明。
pub fn describe_port_state(addr: &str, st: &PortState) -> String {
    match st {
        PortState::Free => format!("{addr} 空闲"),
        PortState::AppMcp { pid, own: true, .. } => format!("{addr} 由本配置目录的 Host 监听（pid {pid}）"),
        PortState::AppMcp { pid, user, own: false } => format!(
            "{addr} 上是另一个 app-mcp Host（pid {pid}，用户 {}）：它使用不同的配置目录（--home / APP_MCP_HOME），或属于其他用户",
            user.as_deref().unwrap_or("未知")
        ),
        PortState::Other { description, owner } => match owner {
            Some(o) => format!("{addr} 被其他程序占用：{o}（{description}）"),
            None => format!("{addr} 被其他程序占用（{description}；无法查到进程）"),
        },
    }
}

fn host_of(addr: &str) -> &str {
    addr.rsplit_once(':').map(|(h, _)| h).unwrap_or("127.0.0.1")
}

/// 要检查的监听地址：显式配置时只有它；否则默认端口与备选端口。
pub fn candidate_addrs(s: &Settings) -> Vec<String> {
    if s.listen_explicit {
        // 端口 0（随机分配，测试用）没有可探测的固定地址。
        let addr = crate::probe_addr(&s.listen);
        return if addr.ends_with(":0") { Vec::new() } else { vec![addr] };
    }
    let host = host_of(&s.listen).to_owned();
    LISTEN_CANDIDATE_PORTS.iter().map(|p| format!("{host}:{p}")).collect()
}

/// `service install` 之前的端口检查结果。
#[derive(Debug, PartialEq, Eq)]
pub struct PortPreflight {
    /// Host 将会使用的地址（第一个空闲的候选）；全部被占用时为 `None`。
    pub chosen: Option<String>,
    /// 被占用的候选及说明。
    pub busy: Vec<(String, String)>,
}

/// 检查监听地址：依次探测候选地址，直到找到空闲的。
pub async fn port_preflight(s: &Settings) -> PortPreflight {
    let candidates = candidate_addrs(s);
    if candidates.is_empty() {
        return PortPreflight { chosen: Some(s.listen.clone()), busy: Vec::new() };
    }
    let mut busy = Vec::new();
    for addr in candidates {
        let st = port_state(&addr, None).await;
        if st == PortState::Free {
            return PortPreflight { chosen: Some(addr), busy };
        }
        let d = describe_port_state(&addr, &st);
        busy.push((addr, d));
    }
    PortPreflight { chosen: None, busy }
}

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

    Report { version: env!("CARGO_PKG_VERSION"), home: home.dir.display().to_string(), checks }
}

fn run_dir_check(dir: &Path) -> Check {
    let meta = match std::fs::metadata(dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Check::new("run_dir", "运行时目录", Level::Info, format!("{} 不存在（首次启动时创建）", dir.display()));
        }
        Err(e) => return Check::new("run_dir", "运行时目录", Level::Error, format!("{} 无法读取：{e}", dir.display())),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let me = app_mcp_protocol::endpoint::current_uid();
        let mode = meta.mode() & 0o777;
        let details = json!({ "path": dir, "mode": format!("{mode:o}"), "uid": meta.uid() });
        if meta.uid() != me {
            return Check::new("run_dir", "运行时目录", Level::Error, format!("{} 属于 uid {}，不是当前用户 {me}", dir.display(), meta.uid()))
                .hint("以当前用户重新创建该目录，或用 --home 指定自己的配置目录")
                .details(details);
        }
        if mode & 0o022 != 0 {
            return Check::new("run_dir", "运行时目录", Level::Error, format!("{} 对组或其他用户可写（权限 {mode:o}），Host 会拒绝启动", dir.display()))
                .hint(format!("chmod 700 {}", dir.display()))
                .details(details);
        }
        Check::new("run_dir", "运行时目录", Level::Ok, format!("{}（权限 {mode:o}，属于当前用户）", dir.display())).details(details)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Check::new("run_dir", "运行时目录", Level::Ok, format!("{}（位于用户目录，继承用户专属权限）", dir.display()))
    }
}

fn ipc_check(endpoint: Option<&str>, running: bool, status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "本地 IPC 端点";
    let Some(ep) = endpoint else {
        return Check::new("ipc", T, Level::Info, "已关闭（ipcEndpoint = none）：原生 App 从登记文件读到 ws://<listen>/app，经 TCP 连接");
    };
    let details = json!({ "endpoint": ep });
    if let Some(name) = ep.strip_prefix("pipe:")
        && let Err(issue) = app_mcp_protocol::endpoint::check_pipe_name(name)
    {
        let max = app_mcp_protocol::endpoint::MAX_PIPE_NAME_CHARS;
        let len = name.encode_utf16().count();
        return Check::new("ipc", T, Level::Error, format!("{name}：{len} 字符，超过命名管道名上限 {max} 字符"))
            .code(issue.code)
            .details(details);
    }
    // 套接字文件与目录的权限检查只在 Unix 上；Windows 的管道所有者在读取 /status 时核对。
    #[cfg(unix)]
    let mut details = details;
    #[cfg(not(unix))]
    let _ = running;
    #[cfg(unix)]
    if let Some(path) = ep.strip_prefix("unix:") {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let path = Path::new(path);
        if let Err(issue) = app_mcp_protocol::endpoint::check_unix_socket_path(path) {
            let max = app_mcp_protocol::endpoint::MAX_UNIX_SOCKET_PATH_BYTES;
            let len = path.as_os_str().len();
            return Check::new("ipc", T, Level::Error, format!("{}：{len} 字节，超过本平台上限 {max} 字节", path.display()))
                .code(issue.code)
                .details(details);
        }
        let me = app_mcp_protocol::endpoint::current_uid();
        match std::fs::symlink_metadata(path) {
            Ok(m) => {
                details["socket"] = json!({ "mode": format!("{:o}", m.mode() & 0o777), "uid": m.uid(), "isSocket": m.file_type().is_socket() });
                if !m.file_type().is_socket() {
                    return Check::new("ipc", T, Level::Error, format!("{} 不是套接字", path.display()))
                        .hint("删除该文件后重新启动 Host")
                        .details(details);
                }
                if m.uid() != me {
                    return Check::new("ipc", T, Level::Error, format!("{} 属于 uid {}，不是当前用户 {me}", path.display(), m.uid()))
                        .code(ConnectionErrorCode::IpcPermissionDenied)
                        .details(details);
                }
            }
            Err(_) if !running => {
                return Check::new("ipc", T, Level::Info, format!("{ep}（Host 未运行，套接字不存在）")).details(details);
            }
            Err(e) => {
                return Check::new("ipc", T, Level::Error, format!("{} 无法访问：{e}", path.display()))
                    .code(ConnectionErrorCode::HostNotRunning)
                    .details(details);
            }
        }
        if let Some(dir) = path.parent()
            && let Ok(m) = std::fs::metadata(dir)
        {
            details["dir"] = json!({ "path": dir, "mode": format!("{:o}", m.mode() & 0o777), "uid": m.uid() });
            if m.uid() != me || m.mode() & 0o022 != 0 {
                return Check::new("ipc", T, Level::Error, format!("套接字目录 {} 不属于当前用户或对他人可写", dir.display()))
                    .code(ConnectionErrorCode::IpcPermissionDenied)
                    .hint(format!("chmod 700 {}，并确认目录属于当前用户", dir.display()))
                    .details(details);
            }
        }
    }
    match status {
        Some(Ok(_)) => Check::new("ipc", T, Level::Ok, format!("{ep}：可连通，监听方是当前用户")).details(details),
        Some(Err(e)) => {
            let code = if e.contains("其他用户") || e.contains("不是当前用户") {
                ConnectionErrorCode::IpcPermissionDenied
            } else {
                ConnectionErrorCode::ConnectFailed
            };
            Check::new("ipc", T, Level::Error, format!("{ep}：{e}")).code(code).details(details)
        }
        None => Check::new("ipc", T, Level::Info, format!("{ep}（Host 未运行）")).details(details),
    }
}

fn excluded_check(addrs: &[String], explicit: bool) -> Check {
    const T: &str = "Windows 排除端口段";
    if !cfg!(windows) {
        return Check::new("excluded_ports", T, Level::Skip, "只适用于 Windows");
    }
    let ranges = match ports::excluded_port_ranges() {
        Ok(r) => r,
        Err(e) => {
            return Check::new("excluded_ports", T, Level::Info, format!("无法读取（netsh int ipv4 show excludedportrange protocol=tcp）：{e}"));
        }
    };
    let ports: Vec<u16> = addrs.iter().filter_map(|a| a.rsplit_once(':')?.1.parse().ok()).collect();
    let hit: Vec<String> = ports
        .iter()
        .filter_map(|p| ranges.iter().find(|r| r.contains(*p)).map(|r| format!("{p}（排除段 {}-{}）", r.start, r.end)))
        .collect();
    let details = json!({ "ranges": ranges, "checked": ports });
    if hit.is_empty() {
        return Check::new("excluded_ports", T, Level::Ok, format!("端口 {ports:?} 都不在排除段内（共 {} 段）", ranges.len())).details(details);
    }
    let all = hit.len() == ports.len();
    Check::new(
        "excluded_ports",
        T,
        if all || explicit { Level::Error } else { Level::Warn },
        format!("以下端口落在系统保留的排除段内，无法绑定：{}", hit.join("、")),
    )
    .code(ConnectionErrorCode::PortBusy)
    .hint("排除段通常由 Hyper-V / WSL / WinNAT 动态保留：管理员运行 net stop winnat && net start winnat 释放后，用 netsh int ipv4 add excludedportrange protocol=tcp startport=7717 numberofports=1 为 app-mcp 永久保留；或用 --listen 换端口")
    .details(details)
}

fn auth_check(home: &AppHome, s: &Settings) -> Check {
    const T: &str = "令牌与鉴权";
    let path = home.token_file();
    let mode = match s.auth {
        AuthMode::Browser => "browser（浏览器来源必须带令牌，本地客户端可不带）",
        AuthMode::All => "all（所有 TCP 请求都必须带令牌）",
        AuthMode::Off => "off（不校验令牌）",
    };
    let details = json!({ "auth": s.auth, "tokenFile": path });
    if s.auth == AuthMode::Off {
        return Check::new("auth", T, Level::Warn, format!("令牌策略 {mode}：本机任何 localhost 页面都能调用 /mcp"))
            .hint("改回默认：app-mcp-host service install --auth browser（多用户机器用 --auth all）")
            .details(details);
    }
    match std::fs::metadata(&path) {
        Err(_) => Check::new("auth", T, Level::Info, format!("令牌策略 {mode}；令牌文件尚未生成（首次启动 serve 时生成）")).details(details),
        Ok(_m) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perm = _m.permissions().mode() & 0o777;
                if perm & 0o077 != 0 {
                    return Check::new("auth", T, Level::Error, format!("令牌文件 {} 对其他用户可读（权限 {perm:o}）", path.display()))
                        .hint(format!("chmod 600 {}，并用 app-mcp-host token --regenerate 轮换令牌", path.display()))
                        .details(details);
                }
            }
            let extra = if s.auth == AuthMode::All {
                "；客户端需配置 Authorization: Bearer $(app-mcp-host token)"
            } else {
                ""
            };
            Check::new("auth", T, Level::Ok, format!("令牌策略 {mode}；令牌文件 {}{extra}", path.display())).details(details)
        }
    }
}

fn apps_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "App 实例";
    let st = match status {
        None => return Check::new("apps", T, Level::Skip, "Host 未运行"),
        Some(Err(e)) => {
            return Check::new("apps", T, Level::Warn, format!("无法读取运行中 Host 的 /status：{e}"))
                .hint("经本地 IPC 或令牌访问 /status；Host 版本过旧时请升级");
        }
        Some(Ok(st)) => st,
    };
    let details = serde_json::to_value(&st.apps).unwrap_or(Value::Null);
    if st.apps.is_empty() {
        return Check::new("apps", T, Level::Info, format!("没有已知 App（MCP 会话 {} 个）", st.mcp_sessions))
            .hint("启动接入了 SDK 的 App，或用 --manifest 加载静态清单")
            .details(details);
    }
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for a in &st.apps {
        let state = match a.state {
            AppState::Connected => "在线",
            AppState::Waking => "唤醒中",
            AppState::Dormant => "休眠",
            AppState::Disconnected => "未连接",
        };
        let cids: Vec<&str> = a.instances.iter().filter_map(|i| i.info.connection_id.as_deref()).collect();
        let cid = if cids.is_empty() { String::new() } else { format!("，连接 {}", cids.join("/")) };
        let wakes = if a.wakes > 0 { format!("，唤醒 {} 次", a.wakes) } else { String::new() };
        lines.push(format!("{}：{state}（实例 {}{cid}{wakes}）", a.app_id, a.instances.len()));
        for i in &a.instances {
            if let Some(p) = &i.power {
                lines.push(format!("{}/{} {}", a.app_id, i.info.instance_id, power_text(p)));
            }
        }
        if let Some(e) = &a.last_error {
            errors.push(format!("{}：[{}] {}", a.app_id, e.code.as_deref().unwrap_or("-"), e.message));
        }
    }
    let summary = format!("{}；MCP 会话 {} 个", lines.join("；"), st.mcp_sessions);
    if errors.is_empty() {
        Check::new("apps", T, Level::Ok, summary).details(details)
    } else {
        Check::new("apps", T, Level::Warn, format!("{summary}。最近错误：{}", errors.join("；")))
            .hint("按错误码处理（spec/protocol.md 10.1）；唤醒失败时检查清单的 wake 配置与 App 是否已安装")
            .details(details)
    }
}

/// 视为"唤醒未完成"的最近错误类别：激活命令失败（含单实例转交超时后第二实例非 0 退出）或唤醒后未回连。
const WAKE_FAILURE_KINDS: [ErrorKind; 2] = [ErrorKind::LaunchFailed, ErrorKind::AppNotResponding];

const WAKE_FAILURE_HINT: &str = "App 未运行时检查清单的 wake 配置与 App 是否已安装；App 进程在运行却唤醒失败时，\
多半是它没能及时处理激活：检查该进程的优先级是否被设为「低」（IDLE）或处于 Windows 效率模式（任务管理器 → 详细信息 / 进程右键），\
以及系统 CPU 是否满载；负载下降后通常自行恢复。本库不调整进程调度，需要时由用户或 App 调整";

/// 唤醒失败排查（TASKS 4f G12）：最近错误为 `LAUNCH_FAILED` / `APP_NOT_RESPONDING` 的 App 给出修复提示。
/// 唤醒速率超限（`WAKE_RATE_LIMITED`）不在此列，见 App 实例检查。
fn wake_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "唤醒";
    let Some(Ok(st)) = status else {
        return Check::new("wake", T, Level::Skip, "无法读取运行中 Host 的状态");
    };
    let failed: Vec<String> = st
        .apps
        .iter()
        .filter_map(|a| {
            let e = a.last_error.as_ref()?;
            let code = e.code.as_deref()?;
            WAKE_FAILURE_KINDS
                .iter()
                .any(|k| k.as_str() == code)
                .then(|| format!("{}：[{code}] {}", a.app_id, e.message))
        })
        .collect();
    if failed.is_empty() {
        return Check::new("wake", T, Level::Ok, "最近没有唤醒失败");
    }
    Check::new("wake", T, Level::Warn, format!("最近唤醒失败：{}", failed.join("；")))
        .hint(WAKE_FAILURE_HINT)
        .details(json!({ "apps": failed }))
}

/// 休眠记录持久化（spec/hub-api.md 3.5「持久化」）：离线检查 `<home>/state/dormant/` 中的文件（Host 未运行也可用），
/// 并附上运行中 Host 的写入状态。被跳过的文件（损坏、版本未知、超出上限）给出警告。
fn dormant_store_check(state_dir: &Path, status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "休眠记录";
    let ttl = app_mcp_hub::HubConfig::default().dormant_ttl;
    let running = match status {
        Some(Ok(st)) => st.dormant_store.as_ref(),
        _ => None,
    };
    let dir = state_dir.join(app_mcp_hub::dormant_store::DORMANT_DIR);
    let files = match app_mcp_hub::dormant_store::inspect(state_dir, std::time::SystemTime::now(), ttl) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Check::new("dormant_store", T, Level::Info, format!("{} 不存在（还没有 App 休眠过）", dir.display()));
        }
        Err(e) => {
            return Check::new("dormant_store", T, Level::Warn, format!("{} 无法读取：{e}", dir.display()))
                .hint("检查目录权限；Host 读不到时重启后不能列出重启前休眠的 App（App 回连后恢复）");
        }
    };
    let details = json!({ "dir": dir.display().to_string(), "files": files, "running": running });
    let instances: u64 = files.iter().map(|f| f.instances).sum();
    let bad: Vec<String> =
        files.iter().filter_map(|f| f.problem.as_ref().map(|p| format!("{}：{p}", f.file))).collect();
    let mut summary = format!("{}：{} 个 App、{instances} 个休眠实例", dir.display(), files.len() - bad.len());
    if let Some(e) = running.and_then(|r| r.last_error.as_deref()) {
        summary.push_str(&format!("；最近写入失败：{e}"));
    }
    if bad.is_empty() && running.and_then(|r| r.last_error.as_ref()).is_none() {
        return Check::new("dormant_store", T, Level::Ok, summary).details(details);
    }
    if !bad.is_empty() {
        summary.push_str(&format!("；跳过的文件：{}", bad.join("；")));
    }
    Check::new("dormant_store", T, Level::Warn, summary)
        .hint("被跳过的文件不会读回（不影响启动，对应 App 回连后恢复）；版本未知的文件可能由更新的 Host 写入，确认无用后可删除")
        .details(details)
}

/// 租约策略与统计（spec/lifecycle.md 第 13 节 B2）。
fn lease_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "租约";
    let Some(Ok(st)) = status else {
        return Check::new("lease", T, Level::Skip, "无法读取 Host 状态");
    };
    let Some(l) = &st.lease else {
        return Check::new("lease", T, Level::Skip, "运行中的 Host 版本不提供租约统计");
    };
    let details = serde_json::to_value(l).unwrap_or(Value::Null);
    Check::new("lease", T, Level::Info, lease_text(l)).details(details)
}

/// 各工具的声明（docs/plans/14-safety.md S5）：`risk` 与 Agent 实际看到的 MCP 注解，便于用户核对 Agent 的放行规则。
fn tools_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "工具声明（risk 与 MCP 注解）";
    let Some(Ok(st)) = status else {
        return Check::new("tools", T, Level::Skip, "无法读取 Host 状态");
    };
    let details: serde_json::Map<String, Value> = st
        .apps
        .iter()
        .filter(|a| !a.tools.is_empty())
        .map(|a| (a.app_id.clone(), serde_json::to_value(&a.tools).unwrap_or(Value::Null)))
        .collect();
    let lines: Vec<String> = st
        .apps
        .iter()
        .flat_map(|a| {
            a.tools.iter().map(move |d| {
                let source = if d.annotations.is_some() { "已声明注解" } else { "注解按 risk 推导" };
                let output = if d.output_schema { "，有 outputSchema" } else { "" };
                let risk = serde_json::to_value(d.risk).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                format!("{}.{}：risk {risk}，{}（{source}{output}）", a.app_id, d.name, annotation_text(&d.effective))
            })
        })
        .collect();
    if lines.is_empty() {
        return Check::new("tools", T, Level::Info, "没有已知工具（或运行中的 Host 版本不提供工具声明）");
    }
    Check::new("tools", T, Level::Info, format!("{} 个工具：\n    {}", lines.len(), lines.join("\n    ")))
        .hint("本库只如实传递 App 的声明，要不要确认由 Agent 决定；可据此配置 Agent 的放行规则（按工具全名）")
        .details(Value::Object(details))
}

/// MCP 工具注解的一行文本（未声明的提示为 `-`）。
fn annotation_text(a: &ToolAnnotations) -> String {
    let b = |v: Option<bool>| v.map_or("-".to_owned(), |v| v.to_string());
    let mut text = format!(
        "readOnlyHint={} destructiveHint={} idempotentHint={} openWorldHint={}",
        b(a.read_only_hint),
        b(a.destructive_hint),
        b(a.idempotent_hint),
        b(a.open_world_hint)
    );
    if let Some(t) = &a.title {
        text.push_str(&format!(" title=「{t}」"));
    }
    text
}

/// 资源保护（spec/hub-api.md 3.11）：限流与大小上限的策略，以及各 App 启动以来被拒绝的次数。
fn limits_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "资源保护（限流与大小上限）";
    let Some(Ok(st)) = status else {
        return Check::new("limits", T, Level::Skip, "无法读取 Host 状态");
    };
    let Some(l) = &st.limits else {
        return Check::new("limits", T, Level::Skip, "运行中的 Host 版本不提供资源保护信息");
    };
    let n = |v: Option<u64>| match v {
        Some(0) => "不限".to_owned(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    let rate = |per_minute: Option<u32>, burst: Option<u32>| match per_minute {
        Some(0) => "不限".to_owned(),
        _ => format!("每分钟 {} 次、突发 {} 次", n(per_minute.map(u64::from)), n(burst.map(u64::from))),
    };
    let policy = format!(
        "每工具 {}；每 App {}；参数 {} 字节、结果 {} 字节、资源 {} 字节；outputSchema 不符时 {}",
        rate(l.tool_rate_per_minute, l.tool_rate_burst),
        rate(l.app_rate_per_minute, l.app_rate_burst),
        n(l.max_arguments_bytes),
        n(l.max_result_bytes),
        n(l.max_resource_bytes),
        st.output_validation
            .and_then(|v| serde_json::to_value(v).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "-".to_owned()),
    );
    let rejected: Vec<String> = st
        .apps
        .iter()
        .filter(|a| a.rate_limited > 0 || a.too_large > 0)
        .map(|a| format!("{}：限流 {} 次、超大 {} 次", a.app_id, a.rate_limited, a.too_large))
        .collect();
    let details = json!({
        "limits": l,
        "outputValidation": st.output_validation,
        "rejected": st.apps.iter().map(|a| json!({"appId": a.app_id, "rateLimited": a.rate_limited, "tooLarge": a.too_large})).collect::<Vec<_>>(),
    });
    if rejected.is_empty() {
        return Check::new("limits", T, Level::Ok, format!("{policy}；没有被拒绝的调用")).details(details);
    }
    Check::new("limits", T, Level::Warn, format!("{policy}。启动以来被拒绝：{}", rejected.join("；")))
        .hint("RATE_LIMITED / PAYLOAD_TOO_LARGE 见 spec/protocol.md 第 4 节；确属正常用量时调大 --tool-rate-limit / --app-rate-limit / --max-*-bytes")
        .details(details)
}

/// 规则文件的校验结果（`policy_check` 的输入，便于测试）。
fn validate_policy_file(home: &AppHome) -> (String, Result<app_mcp_hub::PolicyConfig, String>) {
    let path = home.policy_file();
    (path.display().to_string(), crate::policy::validate_file(&path))
}

/// 策略规则（spec/hub-api.md 3.13）：生效规则与命中次数；规则文件不合法、最近一次重载失败为错误；
/// 文件与生效规则不一致（改了文件没有 reload）为注意。
fn policy_check(file: (String, Result<app_mcp_hub::PolicyConfig, String>), status: Option<&Result<HubStatus, String>>) -> Check {
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

fn lease_text(l: &LeaseStatus) -> String {
    let policy = match l.mode.as_str() {
        "off" => return "租约已关闭（--lease-ms 0）".to_owned(),
        "fixed" => format!("固定 {} ms（自适应已关闭）", l.default_ms),
        _ => {
            let idle = if l.idle_revoke_ms == 0 {
                "不因空闲收回".to_owned()
            } else {
                format!("会话空闲 {} ms 收回默认租约", l.idle_revoke_ms)
            };
            format!(
                "自适应：最近 {} 个间隔 p90 + {} ms，范围 [{}, {}] ms，无历史 {} ms，{idle}",
                l.window, l.margin_ms, l.min_ms, l.max_ms, l.default_ms
            )
        }
    };
    let mut text = format!(
        "{policy}；已发出 自适应 {} / 默认 {} 次，收回 会话结束 {} / 空闲 {} 次",
        l.adaptive_grants, l.default_grants, l.revoked_session_end, l.revoked_idle
    );
    let pairs: Vec<String> = l
        .pairs
        .iter()
        .map(|p| {
            let src = if p.adaptive { "统计" } else { "默认" };
            format!("{}→{} {} ms（{src}，{} 个样本）", p.session, p.app_id, p.next_ttl_ms, p.samples)
        })
        .collect();
    if !pairs.is_empty() {
        text.push_str(&format!("；当前：{}", pairs.join("、")));
    }
    text
}

/// 每实例功耗观测的一行摘要（spec/lifecycle.md 第 12 节）。
fn power_text(p: &InstancePower) -> String {
    let heartbeat = match p.heartbeat_ms {
        None => "双向（旧 SDK）".to_owned(),
        Some(0) => "无（靠连接断开）".to_owned(),
        Some(ms) => format!("SDK 每 {ms} ms"),
    };
    let mode = p.lifecycle_mode.map_or("未声明", |m| match m {
        app_mcp_protocol::LifecycleMode::Persistent => "persistent",
        app_mcp_protocol::LifecycleMode::Idle => "idle",
        app_mcp_protocol::LifecycleMode::OnDemand => "on-demand",
    });
    let mut text = format!(
        "回连 {} 次，唤醒 {} 次，在线 {} 秒，心跳 {} 次（{heartbeat}），模式 {mode}",
        p.reconnects, p.wakes, p.online_secs, p.heartbeats
    );
    if !p.awake_reasons.is_empty() {
        let reasons: Vec<&str> = p
            .awake_reasons
            .iter()
            .map(|r| match r {
                AwakeReason::Persistent => "persistent 模式",
                AwakeReason::Call => "调用进行中",
                AwakeReason::Lease => "租约",
                AwakeReason::Subscription => "资源订阅",
                AwakeReason::WakePending => "待派发的唤醒",
            })
            .collect();
        text.push_str(&format!("，未休眠原因：{}", reasons.join("、")));
    }
    text
}

fn reports_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "SDK 上报（浏览器拦截等）";
    const LIMIT: &str = "被拦截期间页面无法连接 Host，只有连接恢复后才会上报；从未连上的页面请看页面自身的 state.code";
    let Some(Ok(st)) = status else {
        return Check::new("reports", T, Level::Skip, "无法读取 Host 状态");
    };
    let details = serde_json::to_value(&st.reports).unwrap_or(Value::Null);
    if st.reports.is_empty() {
        return Check::new("reports", T, Level::Ok, format!("没有收到上报（{LIMIT}）")).details(details);
    }
    let mut hints = Vec::new();
    let lines: Vec<String> = st
        .reports
        .iter()
        .map(|r| {
            if let Some(c) = ConnectionErrorCode::parse(&r.code)
                && !hints.contains(&c.hint())
            {
                hints.push(c.hint());
            }
            format!("{}（{}，连接 {}）：[{}] {} ×{}", r.app_id, r.instance_id, r.connection_id, r.code, r.message, r.count)
        })
        .collect();
    Check::new("reports", T, Level::Warn, format!("最近 {} 条：{}（{LIMIT}）", lines.len(), lines.join("；")))
        .hint(hints.join("；"))
        .details(details)
}

/// PATH 中的可执行文件（`setup` 检测 Agent 时同样使用）。
///
/// @compat Windows 上按 `.exe`、`.cmd`、`.bat` 依次查找（npm 全局安装的命令是 `.cmd` 包装脚本）。
pub(crate) fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        [".exe", ".cmd", ".bat"].iter().map(|ext| format!("{name}{ext}")).collect()
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

async fn adb_check(host_port: u16) -> Check {
    const T: &str = "Android adb reverse";
    let Some(adb) = find_in_path("adb") else {
        return Check::new("adb", T, Level::Skip, "PATH 中没有 adb");
    };
    let device_port = LISTEN_CANDIDATE_PORTS[0];
    let mut cmd = tokio::process::Command::new(&adb);
    cmd.args(["reverse", "--list"]).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = match tokio::time::timeout(Duration::from_secs(5), cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Check::new("adb", T, Level::Info, format!("运行 {} 失败：{e}", adb.display())),
        Err(_) => return Check::new("adb", T, Level::Info, "adb reverse --list 5 秒内没有返回（adb 服务未就绪？）"),
    };
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let details = json!({ "adb": adb, "output": text, "stderr": String::from_utf8_lossy(&out.stderr) });
    let fix = format!("adb reverse tcp:{device_port} tcp:{host_port}");
    if !out.status.success() {
        return Check::new("adb", T, Level::Info, format!("adb reverse --list 失败（没有连接的设备？）：{}", String::from_utf8_lossy(&out.stderr).trim()))
            .hint(format!("连接设备后运行 {fix}"))
            .details(details);
    }
    if ports::adb_reverse_has(&text, device_port, host_port) {
        Check::new("adb", T, Level::Ok, format!("已转发：设备 tcp:{device_port} → 本机 tcp:{host_port}")).details(details)
    } else {
        Check::new("adb", T, Level::Info, format!("没有设备 tcp:{device_port} → 本机 tcp:{host_port} 的转发；只有 Android App 需要"))
            .hint(fix)
            .details(details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_locks() {
        let text = "1: POSIX  ADVISORY  WRITE 900 00:19:555 0 EOF\n2: FLOCK  ADVISORY  WRITE 4242 08:02:123456 0 EOF\n2: -> FLOCK  ADVISORY  WRITE 4343 08:02:123456 0 EOF\n";
        assert_eq!(parse_proc_locks(text, 123456), Some(4242));
        assert_eq!(parse_proc_locks(text, 555), None, "POSIX 锁不算");
        assert_eq!(parse_proc_locks(text, 23456), None, "inode 需完整匹配");
    }

    #[test]
    fn lease_summary() {
        let mut l = LeaseStatus {
            mode: "adaptive".into(),
            default_ms: 60_000,
            min_ms: 5_000,
            max_ms: 60_000,
            margin_ms: 5_000,
            window: 20,
            idle_revoke_ms: 30_000,
            adaptive_grants: 4,
            default_grants: 3,
            pairs: vec![app_mcp_hub::LeasePairStatus {
                session: "mcp:3".into(),
                app_id: "shop".into(),
                samples: 4,
                next_ttl_ms: 9_000,
                adaptive: true,
            }],
            ..Default::default()
        };
        let t = lease_text(&l);
        assert!(t.contains("p90 + 5000 ms") && t.contains("空闲 30000 ms") && t.contains("mcp:3→shop 9000 ms（统计"), "{t}");
        l.mode = "fixed".into();
        assert!(lease_text(&l).starts_with("固定 60000 ms"));
        l.mode = "off".into();
        assert!(lease_text(&l).contains("已关闭"));
        let c = lease_check(None);
        assert!(matches!(c.status, Level::Skip));
    }

    fn status_with_tools(rate_limited: u64) -> HubStatus {
        serde_json::from_value(json!({
            "service": "app-mcp", "version": "0", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
            "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 0, "reports": [],
            "apps": [{
                "appId": "shop", "name": "商城", "kind": "app", "state": "connected", "instances": [],
                "rateLimited": rate_limited, "tooLarge": 1,
                "tools": [
                    {"name": "cart.checkout", "risk": "payment", "effective": {"readOnlyHint": false, "destructiveHint": true}},
                    {"name": "order.cancel", "risk": "write", "annotations": {"idempotentHint": true, "title": "取消"},
                     "effective": {"readOnlyHint": false, "idempotentHint": true, "title": "取消"}, "outputSchema": true}
                ]
            }],
            "limits": {"toolRatePerMinute": 120, "toolRateBurst": 30, "appRatePerMinute": 0, "appRateBurst": 60,
                       "maxArgumentsBytes": 1048576, "maxResultBytes": 0, "maxResourceBytes": 4194304},
            "outputValidation": "log"
        }))
        .unwrap()
    }

    #[test]
    fn tools_check_lists_declarations() {
        let st = status_with_tools(0);
        let c = tools_check(Some(&Ok(st)));
        assert!(matches!(c.status, Level::Info));
        assert!(c.summary.contains("2 个工具"), "{}", c.summary);
        assert!(
            c.summary.contains("shop.cart.checkout：risk payment，readOnlyHint=false destructiveHint=true idempotentHint=- openWorldHint=-（注解按 risk 推导）"),
            "{}",
            c.summary
        );
        assert!(c.summary.contains("shop.order.cancel：risk write") && c.summary.contains("title=「取消」（已声明注解，有 outputSchema）"), "{}", c.summary);
        assert_eq!(c.details["shop"][1]["annotations"]["idempotentHint"], true);
        assert!(matches!(tools_check(None).status, Level::Skip));
        let mut empty = status_with_tools(0);
        empty.apps[0].tools.clear();
        assert!(tools_check(Some(&Ok(empty))).summary.contains("没有已知工具"));
    }

    #[test]
    fn policy_check_levels() {
        use app_mcp_hub::PolicyConfig;
        let rules = PolicyConfig::from_json(r#"{"rules": [{"id": "h", "action": "hide", "app": "notes"}]}"#).unwrap();
        let file = |r: Result<PolicyConfig, String>| ("/x/policy.json".to_owned(), r);
        let mut st = status_with_tools(0);
        st.policy = Some(serde_json::from_value(json!({
            "rules": [{"id": "h", "action": "hide", "app": "notes", "hits": 2}], "loadedAtMs": 1
        })).unwrap());
        let c = policy_check(file(Ok(rules.clone())), Some(&Ok(st.clone())));
        assert!(matches!(c.status, Level::Info) && c.summary.contains("h：hide app=notes，命中 2 次"), "{}", c.summary);
        let c = policy_check(file(Ok(PolicyConfig::default())), Some(&Ok(st.clone())));
        assert!(matches!(c.status, Level::Warn) && c.summary.contains("尚未重载"), "{}", c.summary);
        let c = policy_check(file(Err("坏了".into())), Some(&Ok(st.clone())));
        assert!(matches!(c.status, Level::Error) && c.summary.contains("继续使用之前的规则"), "{}", c.summary);
        assert!(policy_check(file(Err("坏了".into())), None).summary.contains("启动时会因此失败"));
        let mut failed = st.clone();
        failed.policy.as_mut().unwrap().last_error = Some(app_mcp_hub::PolicyLoadError { message: "id 重复".into(), at_ms: 2 });
        let c = policy_check(file(Ok(rules)), Some(&Ok(failed)));
        assert!(matches!(c.status, Level::Error) && c.summary.contains("id 重复"), "{}", c.summary);
        let mut empty = st;
        empty.policy = Some(Default::default());
        let c = policy_check(file(Ok(PolicyConfig::default())), Some(&Ok(empty)));
        assert!(matches!(c.status, Level::Ok) && c.summary.contains("默认放行"), "{}", c.summary);
        assert!(matches!(policy_check(file(Ok(PolicyConfig::default())), None).status, Level::Skip));
    }

    #[test]
    fn limits_check_reports_policy_and_rejections() {
        let c = limits_check(Some(&Ok(status_with_tools(0))));
        assert!(matches!(c.status, Level::Warn), "超大 1 次也算");
        assert!(c.summary.contains("每工具 每分钟 120 次、突发 30 次；每 App 不限") && c.summary.contains("结果 不限 字节"), "{}", c.summary);
        assert!(c.summary.contains("shop：限流 0 次、超大 1 次") && c.summary.contains("不符时 log"), "{}", c.summary);
        let mut ok = status_with_tools(0);
        ok.apps[0].too_large = 0;
        let c = limits_check(Some(&Ok(ok)));
        assert!(matches!(c.status, Level::Ok) && c.summary.contains("没有被拒绝的调用"), "{}", c.summary);
        let mut old = status_with_tools(3);
        old.limits = None;
        assert!(matches!(limits_check(Some(&Ok(old))).status, Level::Skip));
    }

    fn status_with_error(code: Option<&str>) -> HubStatus {
        let mut st = status_with_tools(0);
        st.apps[0].last_error = code.map(|c| app_mcp_hub::LastError {
            code: Some(c.into()),
            message: format!("唤醒 App「shop」失败：{c}"),
            at_ms: 1,
        });
        st
    }

    #[test]
    fn wake_check_hints_priority_on_wake_failures() {
        for code in ["LAUNCH_FAILED", "APP_NOT_RESPONDING"] {
            let c = wake_check(Some(&Ok(status_with_error(Some(code)))));
            assert!(matches!(c.status, Level::Warn), "{code}");
            assert!(c.summary.contains(&format!("shop：[{code}]")), "{}", c.summary);
            let hint = c.hint.as_deref().unwrap_or_default();
            assert!(hint.contains("优先级") && hint.contains("效率模式") && hint.contains("满载"), "{hint}");
        }
        for code in [None, Some("WAKE_RATE_LIMITED"), Some("PAIRING_REJECTED")] {
            let c = wake_check(Some(&Ok(status_with_error(code))));
            assert!(matches!(c.status, Level::Ok) && c.hint.is_none(), "{code:?}");
        }
        assert!(matches!(wake_check(None).status, Level::Skip));
        assert!(matches!(wake_check(Some(&Err("x".into()))).status, Level::Skip));
    }

    #[test]
    fn report_render_and_errors() {
        let r = Report {
            version: "0",
            home: "/h".into(),
            checks: vec![
                Check::new("a", "甲", Level::Ok, "好"),
                Check::new("b", "乙", Level::Error, "坏").code(ConnectionErrorCode::PortBusy),
            ],
        };
        assert!(r.has_errors());
        let text = r.render();
        assert!(text.contains("[错误] 乙") && text.contains("PORT_BUSY") && text.contains("建议："), "{text}");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["checks"][1]["status"], "error");
        assert_eq!(v["checks"][1]["code"], "PORT_BUSY");
    }

    #[cfg(unix)]
    #[test]
    fn ipc_check_reports_too_long_path() {
        let long = format!("unix:/{}", "p".repeat(app_mcp_protocol::endpoint::MAX_UNIX_SOCKET_PATH_BYTES));
        let c = ipc_check(Some(&long), false, None);
        assert!(matches!(c.status, Level::Error), "{c:?}");
        assert_eq!(c.code, Some("IPC_PATH_TOO_LONG"));
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("--ipc-endpoint")), "{:?}", c.hint);
    }

    #[test]
    fn ipc_check_reports_too_long_pipe_name() {
        let long = format!(r"pipe:\\.\pipe\{}", "p".repeat(app_mcp_protocol::endpoint::MAX_PIPE_NAME_CHARS));
        let c = ipc_check(Some(&long), false, None);
        assert!(matches!(c.status, Level::Error), "{c:?}");
        assert_eq!(c.code, Some("IPC_PATH_TOO_LONG"));
        assert!(c.hint.as_deref().is_some_and(|h| h.contains("--ipc-endpoint")), "{:?}", c.hint);
    }

    #[test]
    fn power_line() {
        let p = InstancePower {
            reconnects: 2,
            wakes: 1,
            online_secs: 30,
            heartbeats: 0,
            heartbeat_ms: Some(0),
            lifecycle_mode: Some(app_mcp_protocol::LifecycleMode::Idle),
            awake_reasons: vec![AwakeReason::Lease, AwakeReason::Subscription],
        };
        let t = power_text(&p);
        assert!(t.contains("回连 2 次") && t.contains("无（靠连接断开）") && t.contains("模式 idle"), "{t}");
        assert!(t.contains("未休眠原因：租约、资源订阅"), "{t}");
        assert!(power_text(&InstancePower::default()).contains("双向（旧 SDK）"));
    }

    #[test]
    fn port_state_text() {
        let other = PortState::Other {
            description: "HTTP 服务（HTTP/1.1 404）".into(),
            owner: Some(PortOwner { pid: Some(7), name: Some("nginx".into()), uid: None }),
        };
        assert!(describe_port_state("127.0.0.1:7717", &other).contains("nginx（pid 7）"));
        assert_eq!(
            serde_json::to_value(PortState::AppMcp { pid: 1, user: None, own: true }).unwrap()["kind"],
            "appMcp"
        );
    }

    /// 显式地址被占用：预检报告占用者（本进程），没有可用地址。
    #[tokio::test]
    async fn preflight_reports_owner() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        // 有连接进来就回一个非 app-mcp 的 HTTP 响应
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            while let Ok((mut s, _)) = l.accept().await {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
            }
        });
        let home = AppHome { dir: std::env::temp_dir().join("app-mcp-doctor-unused") };
        let file = crate::config::FileConfig { listen: Some(addr.clone()), ..Default::default() };
        let s = Settings::resolve(&file, &crate::config::Overrides::default(), &home).unwrap();
        let p = port_preflight(&s).await;
        assert_eq!(p.chosen, None);
        assert_eq!(p.busy.len(), 1);
        assert!(p.busy[0].1.contains("其他程序"), "{:?}", p.busy);
        #[cfg(any(target_os = "linux", windows))]
        assert!(p.busy[0].1.contains(&format!("pid {}", std::process::id())), "{:?}", p.busy);
    }

    #[test]
    fn dormant_store_check_reports_skipped_files() {
        let n: u64 = rand::random();
        let state = std::env::temp_dir().join(format!("app-mcp-doctor-state-{}-{n:x}", std::process::id()));
        assert!(matches!(dormant_store_check(&state, None).status, Level::Info), "目录不存在");
        let dir = state.join("dormant");
        std::fs::create_dir_all(&dir).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let good = json!({"version": 1, "appId": "calc", "savedAtMs": now, "tools": [], "instances": [{
            "instanceId": "i1", "appName": "计算器", "clientKind": "native", "tools": [], "resumeToken": "r",
            "toolsHash": "h", "sleptAtMs": now, "connectedAtMs": now}]});
        std::fs::write(dir.join("calc.json"), good.to_string()).unwrap();
        let c = dormant_store_check(&state, None);
        assert!(matches!(c.status, Level::Ok), "{}", c.summary);
        assert!(c.summary.contains("1 个 App、1 个休眠实例"), "{}", c.summary);
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let c = dormant_store_check(&state, None);
        assert!(matches!(c.status, Level::Warn));
        assert!(c.summary.contains("broken.json"), "{}", c.summary);
        assert!(c.hint.is_some());
        std::fs::remove_dir_all(&state).unwrap();
    }
}
