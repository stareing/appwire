//! 当前用户的登录自启服务：Linux systemd `--user`、macOS launchd LaunchAgent、Windows 当前用户 `Run` 项。
//!
//! 服务文件内容由纯函数生成（[`systemd_unit`]、[`systemd_socket_unit`]、[`launchd_plist`]、[`windows_run_command`]），
//! 启动命令为 `<可执行文件绝对路径> serve --home <配置目录>`；其余设置都在 `<配置目录>/config.json`。
//! 所有操作都不需要管理员权限。
//!
//! 两种方式（[`ServiceSpec::on_demand`]）：
//! - 登录自启（缺省）：登录即运行，常驻。
//! - 按需启动（`service install --on-demand`，spec/protocol.md 1.9）：Linux 写 `app-mcp-host.socket`（HTTP 监听与本地 IPC
//!   由 systemd 持有）+ 不随登录启动的 `app-mcp-host.service`；macOS plist 带 `Sockets`、不设 `RunAtLoad` / `KeepAlive`。
//!   首个连接时系统启动 Host，Host 空闲 `lifecycle.idleExitMs` 后退出。Windows 没有对等机制，不支持。

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod launchd;
#[cfg(all(unix, not(target_os = "macos")))]
mod systemd;
mod unit_files;
#[cfg(windows)]
mod windows_run;

#[cfg(target_os = "macos")]
use launchd as platform;
#[cfg(all(unix, not(target_os = "macos")))]
use systemd as platform;
#[cfg(windows)]
use windows_run as platform;

pub use unit_files::{launchd_plist, systemd_socket_unit, systemd_unit, windows_quote, windows_run_command};

/// systemd unit 名。
pub const SYSTEMD_UNIT: &str = "app-mcp-host.service";
/// systemd 套接字单元名（按需启动；与服务同名，激活 [`SYSTEMD_UNIT`]）。
pub const SYSTEMD_SOCKET_UNIT: &str = "app-mcp-host.socket";
/// launchd Label（plist 文件名为 `<Label>.plist`）。
pub const LAUNCHD_LABEL: &str = "dev.app-mcp.host";
/// Windows `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的值名。
pub const WINDOWS_RUN_VALUE: &str = "app-mcp-host";
/// Windows 注册表键（相对 HKCU）。
pub const WINDOWS_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Windows 下无控制台窗口的常驻程序名（`windows_subsystem = "windows"`，与 app-mcp-host 同目录）。
pub const WINDOWS_BACKGROUND_EXE: &str = "app-mcp-hostw.exe";

/// 服务的启动方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    /// 可执行文件绝对路径（Windows 上为 [`WINDOWS_BACKGROUND_EXE`]）。
    pub exe: PathBuf,
    /// 配置目录绝对路径。
    pub home: PathBuf,
    /// 按需启动时由服务管理器持有的监听套接字；`None` = 登录自启。
    pub on_demand: Option<OnDemandSockets>,
}

/// 按需启动时服务管理器代为监听的套接字（spec/protocol.md 1.9）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnDemandSockets {
    /// HTTP 监听（`/app`、`/mcp`、`/healthz`），必须是 `IP:端口`。
    pub listen: std::net::SocketAddr,
    /// 本地 IPC 套接字的绝对路径；`None` = 不开 IPC。
    pub ipc: Option<PathBuf>,
}

impl OnDemandSockets {
    /// 由 Host 设置得出：`listen` 必须是 `IP:端口`；IPC 端点必须是 `unix:<绝对路径>`。
    ///
    /// @error 监听地址是主机名或端口 0、IPC 端点是命名管道（Windows 没有按需启动）。
    pub fn from_settings(listen: &str, ipc_endpoint: Option<&str>) -> anyhow::Result<Self> {
        let listen: std::net::SocketAddr = listen
            .parse()
            .map_err(|_| anyhow::anyhow!("按需启动需要 IP:端口 形式的监听地址（服务管理器代为监听），而不是 {listen}"))?;
        // @why systemd 忽略 `ListenStream=…:0`（不绑定），launchd 亦无随机端口语义：客户端需要固定地址才能连接即启动。
        if listen.port() == 0 {
            anyhow::bail!("按需启动需要固定端口（服务管理器代为监听，客户端连接即启动），不能用端口 0");
        }
        let ipc = match ipc_endpoint.map(app_mcp_protocol::Endpoint::parse) {
            None => None,
            // Endpoint::parse 已保证 unix: 为 POSIX 绝对路径（不按编译平台判定）。
            Some(Ok(app_mcp_protocol::Endpoint::Unix(path))) => Some(path),
            Some(Ok(other)) => anyhow::bail!("按需启动只支持 unix:<绝对路径> 形式的本地 IPC 端点，而不是 {other}"),
            Some(Err(e)) => anyhow::bail!("本地 IPC 端点不合法：{e}"),
        };
        Ok(Self { listen, ipc })
    }
}

impl ServiceSpec {
    /// 启动参数（不含可执行文件）。
    pub fn args(&self) -> Vec<String> {
        vec![
            "serve".into(),
            "--home".into(),
            self.home.to_string_lossy().into_owned(),
        ]
    }

    fn argv(&self) -> Vec<String> {
        let mut v = vec![self.exe.to_string_lossy().into_owned()];
        v.extend(self.args());
        v
    }
}

// ---------------------------------------------------------------------------
// 平台操作
// ---------------------------------------------------------------------------

/// 服务文件（或注册表项）的位置说明。
pub fn location() -> anyhow::Result<String> {
    platform::location()
}

/// 当前运行的可执行文件（解析符号链接后的绝对路径）。
pub fn current_exe() -> anyhow::Result<PathBuf> {
    let exe =
        std::env::current_exe().map_err(|e| anyhow::anyhow!("无法确定当前可执行文件路径：{e}"))?;
    Ok(std::fs::canonicalize(&exe)
        .map(|p| strip_verbatim(&p))
        .unwrap_or(exe))
}

/// 当前平台上服务应使用的可执行文件。
pub fn service_exe() -> anyhow::Result<PathBuf> {
    Ok(background_exe_for(&current_exe()?))
}

/// Windows 上 `canonicalize` 返回 `\\?\D:\...` 形式：去掉前缀（UNC 路径 `\\?\UNC\` 保留原样）。
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// Windows：同目录下的 `app-mcp-hostw.exe`；其他平台：自身。
pub fn background_exe_for(exe: &Path) -> PathBuf {
    if cfg!(windows) {
        exe.with_file_name(WINDOWS_BACKGROUND_EXE)
    } else {
        exe.to_path_buf()
    }
}

/// 写入服务文件并设为登录自启。
pub fn install(spec: &ServiceSpec) -> anyhow::Result<String> {
    platform::install(spec)
}

/// 取消登录自启并删除服务文件（同时停止由服务管理器运行的实例）。
pub fn uninstall() -> anyhow::Result<bool> {
    platform::uninstall()
}

/// 是否已安装。
pub fn installed() -> anyhow::Result<bool> {
    platform::installed()
}

/// 已安装的是否为按需启动方式（`setup` 重新安装时保持原方式）。
pub fn on_demand_installed() -> anyhow::Result<bool> {
    platform::on_demand_installed()
}

/// 由服务管理器启动（Windows：直接以无窗口方式启动后台进程）。
pub fn start(spec: &ServiceSpec) -> anyhow::Result<()> {
    platform::start(spec)
}

/// 服务管理器是否管理常驻进程（systemd / launchd 是；Windows 登录启动项只负责启动）。
pub const MANAGED_BY_OS: bool = !cfg!(windows);

/// 由服务管理器停止（Windows 上为空操作，由调用方按 pid 结束）。
pub fn stop() -> anyhow::Result<()> {
    platform::stop()
}

/// 服务管理器报告的状态（一行文字）；平台没有时为 `None`。
pub fn manager_status() -> Option<String> {
    platform::manager_status()
}

#[allow(dead_code)]
fn run(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("无法执行 {program}：{e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        anyhow::bail!(
            "{program} {} 失败（{}）：{}",
            args.join(" "),
            out.status,
            if stderr.is_empty() { &stdout } else { &stderr }
        );
    }
    Ok(stdout)
}

/// 按 pid 结束进程（Windows 上服务管理器不管理进程时使用）。
pub fn kill_pid(pid: u32) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|e| anyhow::anyhow!("无法执行 taskkill：{e}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "taskkill 失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        run("kill", &["-TERM", &pid.to_string()]).map(|_| ())
    }
}

#[cfg(test)]
mod tests;
