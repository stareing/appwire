//! 纯函数：生成服务文件内容（systemd unit / socket、launchd plist、Windows `Run` 项命令行）及其引用规则。

use std::path::Path;

use super::{LAUNCHD_LABEL, OnDemandSockets, SYSTEMD_SOCKET_UNIT, SYSTEMD_UNIT, ServiceSpec};

/// systemd 的 `ExecStart` 参数引用：总是加双引号，转义 `\` `"`，`%` → `%%`（说明符），`$` → `$$`（变量展开）。
pub(super) fn systemd_quote(s: &str) -> String {
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
