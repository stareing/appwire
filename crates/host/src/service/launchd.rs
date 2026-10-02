//! macOS 平台操作：launchd LaunchAgent 的 plist 读写与 `launchctl` 调用。

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
