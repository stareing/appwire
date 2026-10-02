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
            Some(Ok(app_mcp_protocol::Endpoint::Unix(path))) if path.is_absolute() => Some(path),
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
// 纯函数：服务文件内容
// ---------------------------------------------------------------------------

/// systemd 的 `ExecStart` 参数引用：总是加双引号，转义 `\` `"`，`%` → `%%`（说明符），`$` → `$$`（变量展开）。
fn systemd_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `~/.config/systemd/user/app-mcp-host.service` 的内容。
pub fn systemd_unit(spec: &ServiceSpec) -> String {
    let exec = spec
        .argv()
        .iter()
        .map(|a| systemd_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    // 按需启动：由套接字单元激活，不随登录启动（没有 [Install]）；空闲退出码 0，Restart=on-failure 不会重启。
    let (unit_extra, install) = if spec.on_demand.is_some() {
        (format!("Requires={SYSTEMD_SOCKET_UNIT}\nAfter={SYSTEMD_SOCKET_UNIT}\n"), String::new())
    } else {
        (String::new(), "\n[Install]\nWantedBy=default.target\n".to_owned())
    };
    format!(
        "# 由 app-mcp-host service install 生成；设置见 {home}/config.json。\n\
         [Unit]\n\
         Description=app-mcp Host（本机 App 的 MCP 服务）\n\
         {unit_extra}\
         StartLimitIntervalSec=60\n\
         StartLimitBurst=5\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         {install}",
        home = spec.home.display(),
    )
}

/// systemd 路径值的转义：`%` → `%%`（说明符）；路径中的空白与控制字符不能出现在 `ListenStream=` 中，显式失败。
fn systemd_path(p: &Path) -> anyhow::Result<String> {
    let s = p.to_str().ok_or_else(|| anyhow::anyhow!("路径不是 UTF-8：{}", p.display()))?;
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        anyhow::bail!("套接字路径含空白或控制字符，无法写入 systemd 单元：{s:?}");
    }
    Ok(s.replace('%', "%%"))
}

/// `~/.config/systemd/user/app-mcp-host.socket` 的内容（按需启动）。
///
/// 一个单元里放 HTTP 与 IPC 两个 `ListenStream`：`FileDescriptorName=` 对单元内所有 fd 生效，Host 按地址族区分
/// （[`crate::activation`]）。IPC 套接字 `0600`、目录 `0700`（与 Host 自己绑定时相同，spec/protocol.md 1.4）。
///
/// @error IPC 路径不能写入单元（非 UTF-8、含空白）。
pub fn systemd_socket_unit(spec: &ServiceSpec, sockets: &OnDemandSockets) -> anyhow::Result<String> {
    let ipc = match &sockets.ipc {
        Some(p) => format!("ListenStream={}\nSocketMode=0600\nDirectoryMode=0700\n", systemd_path(p)?),
        None => String::new(),
    };
    Ok(format!(
        "# 由 app-mcp-host service install --on-demand 生成；设置见 {home}/config.json。\n\
         # 首个连接时 systemd 启动 {SYSTEMD_UNIT}，Host 空闲后退出（lifecycle.idleExitMs）。\n\
         [Unit]\n\
         Description=app-mcp Host 按需启动套接字\n\
         \n\
         [Socket]\n\
         ListenStream={listen}\n\
         {ipc}\
         FileDescriptorName={name}\n\
         Service={SYSTEMD_UNIT}\n\
         \n\
         [Install]\n\
         WantedBy=sockets.target\n",
        home = spec.home.display(),
        listen = sockets.listen,
        name = crate::activation::SYSTEMD_FD_NAME,
    ))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 按需启动的 `Sockets` 字典（键 [`crate::activation::LAUNCHD_HTTP_KEY`] / [`crate::activation::LAUNCHD_IPC_KEY`]）。
fn launchd_sockets(sockets: &OnDemandSockets) -> String {
    let family = if sockets.listen.is_ipv4() { "IPv4" } else { "IPv6" };
    let ipc = sockets.ipc.as_ref().map_or_else(String::new, |p| {
        format!(
            "    <key>{key}</key>\n\
             \x20   <dict>\n\
             \x20     <key>SockPathName</key>\n\
             \x20     <string>{path}</string>\n\
             \x20     <key>SockPathMode</key>\n\
             \x20     <integer>{mode}</integer>\n\
             \x20   </dict>\n",
            key = crate::activation::LAUNCHD_IPC_KEY,
            path = xml_escape(&p.to_string_lossy()),
            mode = app_mcp_protocol::naming::launchd::SOCK_PATH_MODE,
        )
    });
    format!(
        "\x20 <key>Sockets</key>\n\
         \x20 <dict>\n\
         \x20   <key>{key}</key>\n\
         \x20   <dict>\n\
         \x20     <key>SockNodeName</key>\n\
         \x20     <string>{ip}</string>\n\
         \x20     <key>SockServiceName</key>\n\
         \x20     <string>{port}</string>\n\
         \x20     <key>SockType</key>\n\
         \x20     <string>stream</string>\n\
         \x20     <key>SockFamily</key>\n\
         \x20     <string>{family}</string>\n\
         \x20   </dict>\n\
         {ipc}\
         \x20 </dict>\n",
        key = crate::activation::LAUNCHD_HTTP_KEY,
        ip = sockets.listen.ip(),
        port = sockets.listen.port(),
    )
}

/// `~/Library/LaunchAgents/dev.app-mcp.host.plist` 的内容。
///
/// 登录自启：`RunAtLoad`，`KeepAlive.SuccessfulExit = false`：异常退出时重启；正常退出（包括“已有实例在运行”退出码 0）不重启。
/// 按需启动：`Sockets`（launchd 代为监听，首个连接时启动），不设 `RunAtLoad` / `KeepAlive`（`SuccessfulExit` 隐含
/// `RunAtLoad`，launchd.plist(5)）；异常退出后由下一个连接再次启动。
/// stdout / stderr 丢弃：日志已写入 `<home>/logs/`（按大小轮转），避免 launchd 日志无限增长。
pub fn launchd_plist(spec: &ServiceSpec) -> String {
    let args = spec
        .argv()
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml_escape(a)))
        .collect::<String>();
    let start = match &spec.on_demand {
        Some(sockets) => launchd_sockets(sockets),
        None => "\x20 <key>RunAtLoad</key>\n\
                 \x20 <true/>\n\
                 \x20 <key>KeepAlive</key>\n\
                 \x20 <dict>\n\
                 \x20   <key>SuccessfulExit</key>\n\
                 \x20   <false/>\n\
                 \x20 </dict>\n"
            .to_owned(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{LAUNCHD_LABEL}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         {args}\
         \x20 </array>\n\
         {start}\
         \x20 <key>ThrottleInterval</key>\n\
         \x20 <integer>5</integer>\n\
         \x20 <key>ProcessType</key>\n\
         \x20 <string>Background</string>\n\
         \x20 <key>StandardOutPath</key>\n\
         \x20 <string>/dev/null</string>\n\
         \x20 <key>StandardErrorPath</key>\n\
         \x20 <string>/dev/null</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// 按 Windows（MSVC CRT / `CommandLineToArgvW`）规则引用一个参数。
pub fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Windows `Run` 项的命令行（值数据）。可执行文件总是加引号（路径含空格时 `Run` 需要）。
pub fn windows_run_command(spec: &ServiceSpec) -> String {
    let exe = spec.exe.to_string_lossy();
    let mut s = format!("\"{exe}\"");
    for a in spec.args() {
        s.push(' ');
        s.push_str(&windows_quote(&a));
    }
    s
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

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;

    fn unit_dir() -> anyhow::Result<PathBuf> {
        let base = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("无法确定 ~/.config 目录"))?;
        Ok(base.join("systemd").join("user"))
    }

    fn unit_path() -> anyhow::Result<PathBuf> {
        Ok(unit_dir()?.join(SYSTEMD_UNIT))
    }

    fn socket_path() -> anyhow::Result<PathBuf> {
        Ok(unit_dir()?.join(SYSTEMD_SOCKET_UNIT))
    }

    fn write(path: &Path, text: &str) -> anyhow::Result<()> {
        std::fs::write(path, text).map_err(|e| anyhow::anyhow!("写入 {} 失败：{e}", path.display()))
    }

    fn systemctl(args: &[&str]) -> anyhow::Result<String> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        run("systemctl", &all)
    }

    /// systemd 用户实例是否可用。
    pub fn available() -> Result<(), String> {
        match std::process::Command::new("systemctl")
            .args(["--user", "show-environment"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => Err("systemd 用户实例不可用（systemctl --user 失败）".into()),
            Err(_) => Err("未找到 systemctl（系统没有使用 systemd）".into()),
        }
    }

    fn require() -> anyhow::Result<()> {
        available().map_err(|e| {
            anyhow::anyhow!(
                "{e}。本机无法安装登录自启服务：请改为在登录脚本或桌面环境的自启动项中运行 `app-mcp-host serve`（已在运行时会自动退出，可重复执行）。"
            )
        })
    }

    pub fn location() -> anyhow::Result<String> {
        Ok(unit_path()?.display().to_string())
    }

    pub fn installed() -> anyhow::Result<bool> {
        Ok(unit_path()?.exists())
    }

    pub fn on_demand_installed() -> anyhow::Result<bool> {
        Ok(socket_path()?.exists())
    }

    pub fn install(spec: &ServiceSpec) -> anyhow::Result<String> {
        require()?;
        let path = unit_path()?;
        let socket = socket_path()?;
        std::fs::create_dir_all(unit_dir()?)?;
        match &spec.on_demand {
            Some(sockets) => {
                let socket_text = systemd_socket_unit(spec, sockets)?;
                write(&path, &systemd_unit(spec))?;
                write(&socket, &socket_text)?;
                systemctl(&["daemon-reload"])?;
                // 原登录自启方式的实例占着端口 / 套接字：先停下并取消随登录启动，监听交给 systemd。
                let _ = systemctl(&["disable", SYSTEMD_UNIT]);
                let _ = systemctl(&["stop", SYSTEMD_UNIT]);
                systemctl(&["enable", SYSTEMD_SOCKET_UNIT])?;
                // 重新安装时让新的监听地址生效。
                systemctl(&["restart", SYSTEMD_SOCKET_UNIT])?;
                Ok(format!("{}、{}", socket.display(), path.display()))
            }
            None => {
                if socket.exists() {
                    let _ = systemctl(&["disable", "--now", SYSTEMD_SOCKET_UNIT]);
                    std::fs::remove_file(&socket)
                        .map_err(|e| anyhow::anyhow!("删除 {} 失败：{e}", socket.display()))?;
                }
                write(&path, &systemd_unit(spec))?;
                systemctl(&["daemon-reload"])?;
                systemctl(&["enable", SYSTEMD_UNIT])?;
                // 重新安装时让新的 unit 生效。
                systemctl(&["restart", SYSTEMD_UNIT])?;
                Ok(path.display().to_string())
            }
        }
    }

    pub fn uninstall() -> anyhow::Result<bool> {
        let files = [socket_path()?, unit_path()?];
        if !files.iter().any(|p| p.exists()) {
            return Ok(false);
        }
        let manager = available().is_ok();
        if manager {
            // 先停套接字：否则停服务后新连接会再次激活它。
            let _ = systemctl(&["disable", "--now", SYSTEMD_SOCKET_UNIT]);
            let _ = systemctl(&["disable", "--now", SYSTEMD_UNIT]);
        }
        for f in files.iter().filter(|p| p.exists()) {
            std::fs::remove_file(f).map_err(|e| anyhow::anyhow!("删除 {} 失败：{e}", f.display()))?;
        }
        if manager {
            let _ = systemctl(&["daemon-reload"]);
            let _ = systemctl(&["reset-failed", SYSTEMD_SOCKET_UNIT, SYSTEMD_UNIT]);
        }
        Ok(true)
    }

    /// 按需启动：启动套接字单元（之后的第一个连接启动 Host）；登录自启：启动服务。
    pub fn start(_spec: &ServiceSpec) -> anyhow::Result<()> {
        require()?;
        let unit = if on_demand_installed()? { SYSTEMD_SOCKET_UNIT } else { SYSTEMD_UNIT };
        systemctl(&["start", unit]).map(|_| ())
    }

    /// 按需启动时连同套接字单元一起停（否则下一个连接会再次启动 Host）。
    pub fn stop() -> anyhow::Result<()> {
        require()?;
        if on_demand_installed()? {
            systemctl(&["stop", SYSTEMD_SOCKET_UNIT])?;
        }
        systemctl(&["stop", SYSTEMD_UNIT]).map(|_| ())
    }

    pub fn manager_status() -> Option<String> {
        available().ok()?;
        let out = |args: &[&str]| {
            std::process::Command::new("systemctl")
                .arg("--user")
                .args(args)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        };
        let enabled = out(&["is-enabled", SYSTEMD_UNIT])?;
        let active = out(&["is-active", SYSTEMD_UNIT])?;
        if on_demand_installed().unwrap_or(false) {
            let socket_enabled = out(&["is-enabled", SYSTEMD_SOCKET_UNIT])?;
            let socket_active = out(&["is-active", SYSTEMD_SOCKET_UNIT])?;
            return Some(format!(
                "systemd 按需启动：{SYSTEMD_SOCKET_UNIT} {socket_enabled}，{socket_active}；{SYSTEMD_UNIT} {active}（空闲时未运行属正常）"
            ));
        }
        Some(format!("systemd：{enabled}，{active}"))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    fn plist_path() -> anyhow::Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法确定用户主目录"))?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")))
    }

    fn domain() -> anyhow::Result<String> {
        Ok(format!("gui/{}", run("id", &["-u"])?))
    }

    fn target() -> anyhow::Result<String> {
        Ok(format!("{}/{LAUNCHD_LABEL}", domain()?))
    }

    fn loaded() -> bool {
        target()
            .map(|t| run("launchctl", &["print", &t]).is_ok())
            .unwrap_or(false)
    }

    pub fn location() -> anyhow::Result<String> {
        Ok(plist_path()?.display().to_string())
    }

    pub fn installed() -> anyhow::Result<bool> {
        Ok(plist_path()?.exists())
    }

    pub fn on_demand_installed() -> anyhow::Result<bool> {
        Ok(std::fs::read_to_string(plist_path()?).is_ok_and(|t| t.contains("<key>Sockets</key>")))
    }

    pub fn install(spec: &ServiceSpec) -> anyhow::Result<String> {
        let path = plist_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // launchd 只创建套接字文件、不创建目录：按 Host 自己绑定时的要求建 0700 目录（spec/protocol.md 1.4）。
        if let Some(dir) = spec.on_demand.as_ref().and_then(|s| s.ipc.as_deref()).and_then(Path::parent) {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| anyhow::anyhow!("创建 {} 失败：{e}", dir.display()))?;
        }
        if loaded() {
            let _ = run("launchctl", &["bootout", &target()?]);
        }
        std::fs::write(&path, launchd_plist(spec))
            .map_err(|e| anyhow::anyhow!("写入 {} 失败：{e}", path.display()))?;
        let p = path.display().to_string();
        run("launchctl", &["bootstrap", &domain()?, &p])?;
        Ok(p)
    }

    pub fn uninstall() -> anyhow::Result<bool> {
        let path = plist_path()?;
        if loaded() {
            let _ = run("launchctl", &["bootout", &target()?]);
        }
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path)
            .map_err(|e| anyhow::anyhow!("删除 {} 失败：{e}", path.display()))?;
        Ok(true)
    }

    pub fn start(_spec: &ServiceSpec) -> anyhow::Result<()> {
        if !loaded() {
            let p = plist_path()?.display().to_string();
            run("launchctl", &["bootstrap", &domain()?, &p])?;
        }
        if on_demand_installed()? {
            // 按需启动：载入即由 launchd 代为监听，第一个连接启动 Host。
            return Ok(());
        }
        run("launchctl", &["kickstart", &target()?]).map(|_| ())
    }

    pub fn stop() -> anyhow::Result<()> {
        if on_demand_installed()? {
            // 按需启动：卸载作业（launchd 关闭代为监听的套接字），否则下一个连接会再次启动 Host；`start` 重新 bootstrap。
            return run("launchctl", &["bootout", &target()?]).map(|_| ());
        }
        // SIGTERM → serve 正常退出（退出码 0），KeepAlive.SuccessfulExit=false 不会重启。
        run("launchctl", &["kill", "SIGTERM", &target()?]).map(|_| ())
    }

    pub fn manager_status() -> Option<String> {
        Some(if loaded() {
            "launchd：已加载".into()
        } else {
            "launchd：未加载".into()
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_value() -> Option<String> {
        let key = wide(WINDOWS_RUN_KEY);
        let name = wide(WINDOWS_RUN_VALUE);
        let mut buf = vec![0u16; 4096];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY：缓冲区与长度匹配；键名 / 值名为以 0 结尾的 UTF-16。
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..chars]);
        Some(s.trim_end_matches('\0').to_owned())
    }

    pub fn location() -> anyhow::Result<String> {
        Ok(format!(r"HKCU\{WINDOWS_RUN_KEY}\{WINDOWS_RUN_VALUE}"))
    }

    pub fn installed() -> anyhow::Result<bool> {
        Ok(read_value().is_some())
    }

    pub fn on_demand_installed() -> anyhow::Result<bool> {
        Ok(false)
    }

    pub fn install(spec: &ServiceSpec) -> anyhow::Result<String> {
        if spec.on_demand.is_some() {
            anyhow::bail!(
                "Windows 没有与 systemd / launchd 套接字激活对等的机制，不支持按需启动；请去掉 --on-demand 使用登录自启（常驻成本见 crates/host/README.md「按需启动」）"
            );
        }
        if !spec.exe.exists() {
            anyhow::bail!(
                "找不到 {}：Windows 常驻进程使用无控制台窗口的 {WINDOWS_BACKGROUND_EXE}（与 app-mcp-host.exe 一同构建 / 发布，放在同一目录）。",
                spec.exe.display()
            );
        }
        let key = wide(WINDOWS_RUN_KEY);
        let name = wide(WINDOWS_RUN_VALUE);
        let data = wide(&windows_run_command(spec));
        // SAFETY：data 为以 0 结尾的 UTF-16，长度按字节计（含结尾 0）。
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if rc != ERROR_SUCCESS {
            anyhow::bail!("写入注册表 {} 失败（错误码 {rc}）", location()?);
        }
        location()
    }

    pub fn uninstall() -> anyhow::Result<bool> {
        if read_value().is_none() {
            return Ok(false);
        }
        let key = wide(WINDOWS_RUN_KEY);
        let name = wide(WINDOWS_RUN_VALUE);
        // SAFETY：键名 / 值名为以 0 结尾的 UTF-16。
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        if rc != ERROR_SUCCESS {
            anyhow::bail!("删除注册表 {} 失败（错误码 {rc}）", location()?);
        }
        Ok(true)
    }

    pub fn start(spec: &ServiceSpec) -> anyhow::Result<()> {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if !spec.exe.exists() {
            anyhow::bail!("找不到 {}", spec.exe.display());
        }
        std::process::Command::new(&spec.exe)
            .args(spec.args())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("启动 {} 失败：{e}", spec.exe.display()))
    }

    pub fn stop() -> anyhow::Result<()> {
        Ok(())
    }

    pub fn manager_status() -> Option<String> {
        read_value().map(|v| format!("登录启动项：{v}"))
    }
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
mod tests {
    use super::*;

    fn unix_spec() -> ServiceSpec {
        ServiceSpec {
            exe: PathBuf::from("/opt/app mcp/bin/app-mcp-host"),
            home: PathBuf::from("/home/u/.app-mcp"),
            on_demand: None,
        }
    }

    #[test]
    fn systemd_unit_snapshot() {
        let expected = r#"# 由 app-mcp-host service install 生成；设置见 /home/u/.app-mcp/config.json。
[Unit]
Description=app-mcp Host（本机 App 的 MCP 服务）
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
ExecStart="/opt/app mcp/bin/app-mcp-host" "serve" "--home" "/home/u/.app-mcp"
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"#;
        assert_eq!(systemd_unit(&unix_spec()), expected);
    }


    fn on_demand(ipc: Option<&str>) -> OnDemandSockets {
        OnDemandSockets { listen: "127.0.0.1:7717".parse().unwrap(), ipc: ipc.map(PathBuf::from) }
    }

    #[test]
    fn systemd_on_demand_units_snapshot() {
        let spec = ServiceSpec { on_demand: Some(on_demand(Some("/run/user/1000/app-mcp/hub.sock"))), ..unix_spec() };
        let expected = r#"# 由 app-mcp-host service install 生成；设置见 /home/u/.app-mcp/config.json。
[Unit]
Description=app-mcp Host（本机 App 的 MCP 服务）
Requires=app-mcp-host.socket
After=app-mcp-host.socket
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
ExecStart="/opt/app mcp/bin/app-mcp-host" "serve" "--home" "/home/u/.app-mcp"
Restart=on-failure
RestartSec=2
"#;
        assert_eq!(systemd_unit(&spec), expected, "按需启动的服务不随登录启动（没有 [Install]）");
        let expected = r#"# 由 app-mcp-host service install --on-demand 生成；设置见 /home/u/.app-mcp/config.json。
# 首个连接时 systemd 启动 app-mcp-host.service，Host 空闲后退出（lifecycle.idleExitMs）。
[Unit]
Description=app-mcp Host 按需启动套接字

[Socket]
ListenStream=127.0.0.1:7717
ListenStream=/run/user/1000/app-mcp/hub.sock
SocketMode=0600
DirectoryMode=0700
FileDescriptorName=app-mcp-host
Service=app-mcp-host.service

[Install]
WantedBy=sockets.target
"#;
        assert_eq!(systemd_socket_unit(&spec, spec.on_demand.as_ref().unwrap()).unwrap(), expected);

        let no_ipc = on_demand(None);
        let text = systemd_socket_unit(&spec, &no_ipc).unwrap();
        assert_eq!(text.matches("ListenStream=").count(), 1);
        assert!(!text.contains("SocketMode"));
        let v6 = OnDemandSockets { listen: "[::1]:7737".parse().unwrap(), ipc: Some(PathBuf::from("/run/a%b/hub.sock")) };
        let text = systemd_socket_unit(&spec, &v6).unwrap();
        assert!(text.contains("ListenStream=[::1]:7737\n"), "{text}");
        assert!(text.contains("ListenStream=/run/a%%b/hub.sock\n"), "% 是 systemd 说明符：{text}");
        let spaced = OnDemandSockets { ipc: Some(PathBuf::from("/run/a b/hub.sock")), ..on_demand(None) };
        assert!(systemd_socket_unit(&spec, &spaced).is_err(), "ListenStream 不能含空白");
    }

    #[test]
    fn launchd_on_demand_plist_snapshot() {
        let spec = ServiceSpec {
            exe: PathBuf::from("/Users/u/.cargo/bin/app-mcp-host"),
            home: PathBuf::from("/Users/u/.app-mcp"),
            on_demand: Some(on_demand(Some("/Users/u/.app-mcp/run/hub.sock"))),
        };
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.app-mcp.host</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/u/.cargo/bin/app-mcp-host</string>
    <string>serve</string>
    <string>--home</string>
    <string>/Users/u/.app-mcp</string>
  </array>
  <key>Sockets</key>
  <dict>
    <key>Http</key>
    <dict>
      <key>SockNodeName</key>
      <string>127.0.0.1</string>
      <key>SockServiceName</key>
      <string>7717</string>
      <key>SockType</key>
      <string>stream</string>
      <key>SockFamily</key>
      <string>IPv4</string>
    </dict>
    <key>Ipc</key>
    <dict>
      <key>SockPathName</key>
      <string>/Users/u/.app-mcp/run/hub.sock</string>
      <key>SockPathMode</key>
      <integer>384</integer>
    </dict>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>/dev/null</string>
  <key>StandardErrorPath</key>
  <string>/dev/null</string>
</dict>
</plist>
"#;
        let text = launchd_plist(&spec);
        assert_eq!(text, expected);
        assert!(!text.contains("RunAtLoad") && !text.contains("KeepAlive"), "按需启动不能隐含 RunAtLoad");
        let v6 = ServiceSpec { on_demand: Some(OnDemandSockets { listen: "[::1]:7737".parse().unwrap(), ipc: None }), ..spec };
        let text = launchd_plist(&v6);
        assert!(text.contains("<string>::1</string>") && text.contains("<string>IPv6</string>"), "{text}");
        assert!(!text.contains("<key>Ipc</key>"));
    }

    #[test]
    fn on_demand_sockets_from_settings() {
        let s = OnDemandSockets::from_settings("127.0.0.1:7717", Some("unix:/run/user/1/app-mcp/hub.sock")).unwrap();
        assert_eq!(s, on_demand(Some("/run/user/1/app-mcp/hub.sock")));
        assert_eq!(OnDemandSockets::from_settings("127.0.0.1:7717", None).unwrap().ipc, None);
        assert!(OnDemandSockets::from_settings("localhost:7717", None).is_err(), "服务管理器需要 IP");
        assert!(OnDemandSockets::from_settings("127.0.0.1:0", None).is_err(), "systemd 不绑定端口 0");
        assert!(OnDemandSockets::from_settings("127.0.0.1:7717", Some(r"pipe:\\.\pipe\x")).is_err());
        assert!(OnDemandSockets::from_settings("127.0.0.1:7717", Some("ws://127.0.0.1:1")).is_err());
    }

    #[test]
    fn systemd_quoting() {
        assert_eq!(systemd_quote(r#"a"b\c%d$e"#), r#""a\"b\\c%%d$$e""#);
    }

    #[test]
    fn launchd_plist_snapshot() {
        let spec = ServiceSpec {
            exe: PathBuf::from("/Users/u/.cargo/bin/app-mcp-host"),
            home: PathBuf::from("/Users/u/.app-mcp & co"),
            on_demand: None,
        };
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.app-mcp.host</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/u/.cargo/bin/app-mcp-host</string>
    <string>serve</string>
    <string>--home</string>
    <string>/Users/u/.app-mcp &amp; co</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>/dev/null</string>
  <key>StandardErrorPath</key>
  <string>/dev/null</string>
</dict>
</plist>
"#;
        assert_eq!(launchd_plist(&spec), expected);
    }

    #[test]
    fn windows_run_snapshot() {
        let spec = ServiceSpec {
            exe: PathBuf::from(r"C:\Program Files\app-mcp\app-mcp-hostw.exe"),
            home: PathBuf::from(r"C:\Users\Zhang San\.app-mcp"),
            on_demand: None,
        };
        assert_eq!(
            windows_run_command(&spec),
            r#""C:\Program Files\app-mcp\app-mcp-hostw.exe" serve --home "C:\Users\Zhang San\.app-mcp""#
        );
        let spec = ServiceSpec {
            exe: PathBuf::from(r"D:\tools\app-mcp-hostw.exe"),
            home: PathBuf::from(r"D:\cfg\"),
            on_demand: None,
        };
        assert_eq!(
            windows_run_command(&spec),
            r#""D:\tools\app-mcp-hostw.exe" serve --home D:\cfg\"#
        );
    }

    #[test]
    fn windows_quoting_rules() {
        assert_eq!(windows_quote("plain"), "plain");
        assert_eq!(windows_quote(""), r#""""#);
        assert_eq!(windows_quote(r"a b\"), r#""a b\\""#);
        assert_eq!(windows_quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(windows_quote(r#"a\"b c"#), r#""a\\\"b c""#);
    }

    #[test]
    fn verbatim_prefix() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\D:\a\b.exe")),
            PathBuf::from(r"D:\a\b.exe")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\x")),
            PathBuf::from(r"\\?\UNC\srv\x")
        );
        assert_eq!(
            strip_verbatim(Path::new("/usr/bin/x")),
            PathBuf::from("/usr/bin/x")
        );
    }

    #[test]
    fn background_exe() {
        let exe = Path::new("/x/app-mcp-host");
        if cfg!(windows) {
            assert!(background_exe_for(exe).ends_with(WINDOWS_BACKGROUND_EXE));
        } else {
            assert_eq!(background_exe_for(exe), exe);
        }
    }
}
