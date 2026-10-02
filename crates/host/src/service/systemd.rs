//! Linux 平台操作：systemd `--user` 的 unit 文件读写与 `systemctl` 调用。

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
