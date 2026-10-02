//! `app-mcp-host app install / uninstall`：按名寻址的 App 登记（spec/naming.md 4.1、5.3）。
//!
//! Linux 写两个文件（只写用户目录，不需要特权）：
//! - 会话服务激活文件 `<数据目录>/dbus-1/services/dev.appmcp.App.<id>.service`（`Name=` 与文件名一致，
//!   `Exec="<程序>" --app-mcp-activation`），写入后调用 `ReloadConfig`；
//! - App 登记文件 `<数据目录>/app-mcp/apps/<appId>.json`（`source: "manual"`，`activation.kind: "dbus"`）。
//!
//! 给出静态清单时另复制到 `<home>/manifests/<appId>.json`：Host 启动时加载，App 未运行也能列出工具并按名激活。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use app_mcp_protocol::naming::{Address, dbus as names};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::cli::{AppAction, AppInstallArgs, AppTargetArgs, AppUninstallArgs};
use crate::config::{AppHome, absolute};

/// App 登记文件格式版本（spec/naming.md 5.3）。
const REGISTRATION_VERSION: u32 = 1;

/// App 登记文件（spec/naming.md 5.3）。
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub registration_version: u32,
    pub app_id: String,
    pub name: String,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    pub executable: PathBuf,
    pub activation: Activation,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Activation {
    pub kind: &'static str,
    pub target: String,
}

/// 写入（或将删除）的文件位置。
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    pub service_file: PathBuf,
    pub registration_file: PathBuf,
    pub manifest_copy: PathBuf,
}

impl Paths {
    pub fn new(data_home: &Path, home: &AppHome, app_id: &str) -> Self {
        Self {
            service_file: data_home.join("dbus-1").join("services").join(names::service_file_name(app_id)),
            registration_file: data_home.join("app-mcp").join("apps").join(format!("{app_id}.json")),
            manifest_copy: home.manifest_dir().join(format!("{app_id}.json")),
        }
    }
}

pub async fn cmd(action: AppAction) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        cfg!(target_os = "linux"),
        "本平台尚未实现按名寻址的 App 登记（spec/naming.md 4.0：目前只有 Linux D-Bus）"
    );
    let (target, reload) = match action {
        AppAction::Install(args) => {
            let paths = install(&args)?;
            println!("已登记 {}：", args.app_id);
            println!("  激活文件  {}", paths.service_file.display());
            println!("  登记文件  {}", paths.registration_file.display());
            if args.manifest.is_some() {
                println!("  清单      {}（Host 重启后生效）", paths.manifest_copy.display());
            }
            (args.target, true)
        }
        AppAction::Uninstall(args) => {
            let removed = uninstall(&args)?;
            if removed.is_empty() {
                println!("{} 没有登记，无需撤销", args.app_id);
            }
            for p in &removed {
                println!("已删除 {}", p.display());
            }
            (args.target, !removed.is_empty())
        }
    };
    if reload && !target.no_reload {
        reload_bus().await;
    }
    Ok(ExitCode::SUCCESS)
}

/// 数据目录：`--data-home` > `$XDG_DATA_HOME`（绝对路径时）> `~/.local/share`。
fn data_home(target: &AppTargetArgs) -> anyhow::Result<PathBuf> {
    if let Some(d) = &target.data_home {
        return absolute(d);
    }
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|d| d.is_absolute()) {
        return Ok(d);
    }
    Ok(dirs::home_dir().context("无法确定用户主目录；请用 --data-home 指定")?.join(".local").join("share"))
}

/// 登记：校验参数后写激活文件、登记文件（与清单副本）。返回写入的位置。
pub fn install(args: &AppInstallArgs) -> anyhow::Result<Paths> {
    let address = Address::new(&args.app_id, None).map_err(|e| anyhow::anyhow!("--app-id 无效：{e}"))?;
    anyhow::ensure!(
        !app_mcp_manifest::is_reserved_app_id(&args.app_id),
        "appId「{}」是保留名（spec/naming.md 2.2）",
        args.app_id
    );
    let exec = absolute(&args.exec)?;
    let exec = std::fs::canonicalize(&exec).with_context(|| format!("程序 {} 不存在", exec.display()))?;
    anyhow::ensure!(exec.is_file(), "{} 不是文件", exec.display());
    let exec_text = exec.to_str().with_context(|| format!("程序路径不是 UTF-8：{}", exec.display()))?;

    let manifest = match &args.manifest {
        Some(m) => {
            let path = absolute(m)?;
            let loaded = app_mcp_manifest::load_file(&path).map_err(|e| anyhow::anyhow!("{e}"))?;
            anyhow::ensure!(
                loaded.manifest.app_id == args.app_id,
                "清单的 appId（{}）与 --app-id（{}）不一致",
                loaded.manifest.app_id,
                args.app_id
            );
            let bytes = std::fs::read(&path).with_context(|| format!("读取 {} 失败", path.display()))?;
            Some((path, loaded.manifest.name, bytes))
        }
        None => None,
    };
    let home = AppHome::resolve(args.target.home.home.as_deref())?;
    let paths = Paths::new(&data_home(&args.target)?, &home, &args.app_id);
    let name = args
        .name
        .clone()
        .or_else(|| manifest.as_ref().map(|(_, n, _)| n.clone()))
        .unwrap_or_else(|| args.app_id.clone());
    let registration = Registration {
        registration_version: REGISTRATION_VERSION,
        app_id: args.app_id.clone(),
        name,
        source: "manual",
        manifest: manifest.as_ref().map(|_| paths.manifest_copy.clone()),
        manifest_sha256: manifest.as_ref().map(|(_, _, b)| format!("{:x}", Sha256::digest(b))),
        executable: exec.clone(),
        activation: Activation { kind: "dbus", target: names::bus_name(&address) },
    };

    if let Some((_, _, bytes)) = &manifest {
        write_file(&paths.manifest_copy, bytes)?;
    }
    let mut json = serde_json::to_string_pretty(&registration)?;
    json.push('\n');
    write_file(&paths.registration_file, json.as_bytes())?;
    write_file(&paths.service_file, names::service_file(&args.app_id, exec_text).as_bytes())?;
    Ok(paths)
}

/// 撤销登记：删除 install 写入的文件，返回实际删除的路径。
pub fn uninstall(args: &AppUninstallArgs) -> anyhow::Result<Vec<PathBuf>> {
    Address::new(&args.app_id, None).map_err(|e| anyhow::anyhow!("--app-id 无效：{e}"))?;
    let home = AppHome::resolve(args.target.home.home.as_deref())?;
    let paths = Paths::new(&data_home(&args.target)?, &home, &args.app_id);
    let mut removed = Vec::new();
    for p in [&paths.service_file, &paths.registration_file, &paths.manifest_copy] {
        match std::fs::remove_file(p) {
            Ok(()) => removed.push(p.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::anyhow!("删除 {} 失败：{e}", p.display())),
        }
    }
    Ok(removed)
}

/// 原子写（临时文件 + 改名），必要时创建目录。
fn write_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().with_context(|| format!("{} 没有上级目录", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("创建目录 {} 失败", dir.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("写入 {} 失败", path.display()))
}

/// 请会话总线重新读取激活目录；失败只提示（dbus-daemon 也会经 inotify 发现用户目录中的变化）。
async fn reload_bus() {
    #[cfg(target_os = "linux")]
    {
        let connector = app_mcp_hub::connector::DbusConnector::new(None);
        if let Err(e) = connector.reload_config().await {
            eprintln!("提示：未能通知 D-Bus 重新读取激活文件（{e}）；登录会话的总线通常会自动发现，必要时重新登录");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::HomeArg;

    fn target(dir: &Path) -> AppTargetArgs {
        AppTargetArgs {
            home: HomeArg { home: Some(dir.join("home")) },
            data_home: Some(dir.join("data")),
            no_reload: true,
        }
    }

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("app-mcp-install-{:032x}", rand::random::<u128>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn install_writes_service_registration_and_manifest_then_uninstall_removes() {
        let dir = scratch();
        let exe = dir.join("my app");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        let manifest = dir.join("app-mcp.json");
        std::fs::write(
            &manifest,
            r#"{"manifestVersion":1,"appId":"my-shop","name":"我的商城","tools":[{"name":"a","description":"d","inputSchema":{"type":"object"}}]}"#,
        )
        .unwrap();
        let args = AppInstallArgs {
            app_id: "my-shop".into(),
            exec: exe.clone(),
            name: None,
            manifest: Some(manifest),
            target: target(&dir),
        };
        let paths = install(&args).unwrap();
        assert_eq!(paths.service_file, dir.join("data/dbus-1/services/dev.appmcp.App.my_shop.service"));
        let service = std::fs::read_to_string(&paths.service_file).unwrap();
        let exe = std::fs::canonicalize(&exe).unwrap();
        assert_eq!(
            service,
            format!("[D-BUS Service]\nName=dev.appmcp.App.my_shop\nExec=\"{}\" --app-mcp-activation\n", exe.display())
        );
        let reg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.registration_file).unwrap()).unwrap();
        assert_eq!(paths.registration_file, dir.join("data/app-mcp/apps/my-shop.json"));
        assert_eq!(reg["registrationVersion"], 1);
        assert_eq!(reg["appId"], "my-shop");
        assert_eq!(reg["name"], "我的商城");
        assert_eq!(reg["source"], "manual");
        assert_eq!(reg["activation"]["kind"], "dbus");
        assert_eq!(reg["activation"]["target"], "dev.appmcp.App.my_shop");
        assert_eq!(reg["executable"], exe.display().to_string());
        assert_eq!(reg["manifestSha256"].as_str().map(str::len), Some(64));
        assert!(paths.manifest_copy.is_file());
        // 幂等：再装一次覆盖。
        install(&args).unwrap();

        let removed =
            uninstall(&AppUninstallArgs { app_id: "my-shop".into(), target: target(&dir) }).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(!paths.service_file.exists() && !paths.registration_file.exists() && !paths.manifest_copy.exists());
        let again = uninstall(&AppUninstallArgs { app_id: "my-shop".into(), target: target(&dir) }).unwrap();
        assert!(again.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_rejects_bad_input() {
        let dir = scratch();
        let exe = dir.join("app");
        std::fs::write(&exe, b"x").unwrap();
        let mk = |id: &str, exec: PathBuf| AppInstallArgs {
            app_id: id.into(),
            exec,
            name: None,
            manifest: None,
            target: target(&dir),
        };
        assert!(install(&mk("Bad", exe.clone())).is_err(), "appId 格式");
        assert!(install(&mk("apps", exe.clone())).is_err(), "保留名");
        assert!(install(&mk("ok", dir.join("missing"))).is_err(), "程序不存在");
        let other = dir.join("other.json");
        std::fs::write(&other, r#"{"manifestVersion":1,"appId":"other","name":"x"}"#).unwrap();
        let mut a = mk("ok", exe);
        a.manifest = Some(other);
        assert!(install(&a).is_err(), "清单 appId 不一致");
        assert!(!dir.join("data").exists(), "失败时不写任何文件");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
