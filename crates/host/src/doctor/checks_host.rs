//! 本机环境检查：运行时目录、本地 IPC 端点、Windows 排除端口段、令牌与鉴权、Android `adb reverse`。

use std::path::Path;
use std::time::Duration;

use app_mcp_hub::HubStatus;
use app_mcp_protocol::{ConnectionErrorCode, LISTEN_CANDIDATE_PORTS};
use serde_json::json;

use super::{Check, Level, command};
use crate::config::{AppHome, AuthMode, Settings};
use crate::ports;

pub(super) fn run_dir_check(dir: &Path) -> Check {
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

pub(super) fn ipc_check(endpoint: Option<&str>, running: bool, status: Option<&Result<HubStatus, String>>) -> Check {
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

pub(super) fn excluded_check(addrs: &[String], explicit: bool) -> Check {
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

pub(super) fn auth_check(home: &AppHome, s: &Settings) -> Check {
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

/// 每条 adb 命令的超时（B-08）。
pub(super) const ADB_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn adb_check(host_port: u16) -> Check {
    const T: &str = "Android adb reverse";
    let Some(adb) = find_in_path("adb") else {
        return Check::new("adb", T, Level::Skip, "PATH 中没有 adb");
    };
    let device_port = LISTEN_CANDIDATE_PORTS[0];
    let out = match command::run_tool(&adb, &["reverse", "--list"], ADB_TIMEOUT).await {
        Ok(o) => o,
        Err(e @ command::ToolFailure::Spawn(_)) => {
            return Check::new("adb", T, Level::Info, format!("{}：{e}", adb.display()));
        }
        Err(e) => return Check::new("adb", T, Level::Info, format!("adb reverse --list {e}（adb 服务未就绪？）")),
    };
    let text = out.stdout;
    let details = json!({ "adb": adb, "output": text, "stderr": out.stderr });
    let fix = format!("adb reverse tcp:{device_port} tcp:{host_port}");
    if !out.success {
        return Check::new("adb", T, Level::Info, format!("adb reverse --list 失败（没有连接的设备？）：{}", out.stderr.trim()))
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
