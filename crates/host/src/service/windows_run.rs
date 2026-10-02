//! Windows 平台操作：当前用户注册表 `Run` 项的读写，以及直接以无窗口方式启动后台进程。

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
