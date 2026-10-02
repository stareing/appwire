//! `app-mcp-host app install / uninstall`：按名寻址的 App 登记（spec/naming.md 4.1、4.3、5.3）。
//!
//! 只写用户目录，不需要特权：
//! - App 登记文件 `<数据目录>/app-mcp/apps/<appId>.json`（`source: "manual"`）：Linux 数据目录为 `$XDG_DATA_HOME`，
//!   激活方式 `dbus`；Windows 为 `%LOCALAPPDATA%`，激活方式 `exec`（Hub 直接运行程序并追加 `--app-mcp-activation`），
//!   它是 Windows 上 Hub 发现 App 的唯一来源（Hub 经目录变更通知即时看到，无需重启）；
//! - 仅 Linux：会话服务激活文件 `<数据目录>/dbus-1/services/dev.appmcp.App.<id>.service`（`Name=` 与文件名一致，
//!   `Exec="<程序>" --app-mcp-activation`），写入后调用 `ReloadConfig`。
//! - 仅 macOS（4.4）：数据目录为 `~/Library/Application Support`，激活方式 `launchd`（`target` = 套接字路径
//!   `<home>/run/apps/<appId>.sock`）；用户 LaunchAgent `<数据目录的上级>/LaunchAgents/dev.appmcp.App.<id>.plist`
//!   （默认 `~/Library/LaunchAgents`，按需套接字，不常驻），写入后 `launchctl bootstrap gui/<uid>`；撤销时先 `launchctl bootout`。
//!
//! 给出静态清单时另复制到 `<home>/manifests/<appId>.json`：Host 启动时加载，App 未运行也能列出工具并按名激活
//! （登记文件的 `manifest` 指向该副本）。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context;
use app_mcp_protocol::naming::registration::{self, Activation, REGISTRATION_VERSION, Registration};
use app_mcp_protocol::naming::{Address, pipe as pipe_names};
use sha2::{Digest, Sha256};

use crate::cli::{AppAction, AppInstallArgs, AppTargetArgs, AppUninstallArgs};
use crate::config::{AppHome, absolute};

/// 写入（或将删除）的文件位置。
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    /// D-Bus 激活文件（仅 Linux）。
    pub service_file: Option<PathBuf>,
    /// launchd 用户 Agent 的 plist 与按需套接字（仅 macOS，[`LaunchdPaths`]）。
    pub launchd: Option<LaunchdPaths>,
    pub registration_file: PathBuf,
    pub manifest_copy: PathBuf,
}

/// macOS launchd 的两个位置（spec/naming.md 4.4）。
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchdPaths {
    /// `<数据目录的上级>/LaunchAgents/dev.appmcp.App.<appId>.plist`（默认 `~/Library/LaunchAgents`）。
    pub plist: PathBuf,
    /// `<home>/run/apps/<appId>.sock`：launchd 创建与监听，Hub 连接即激活。
    pub socket: PathBuf,
}

impl LaunchdPaths {
    /// 用户 LaunchAgents 目录：数据目录的上级下的 `LaunchAgents`（默认 `~/Library/LaunchAgents`；doctor 读同一位置）。
    pub fn agents_dir(data_home: &Path) -> PathBuf {
        data_home.parent().unwrap_or(data_home).join("LaunchAgents")
    }

    /// @why LaunchAgents 目录由数据目录（`~/Library/Application Support`）推出，`--data-home` 指向临时目录时一并隔离。
    pub fn new(data_home: &Path, home: &AppHome, app_id: &str) -> Self {
        use app_mcp_protocol::naming::launchd as names;
        Self {
            plist: Self::agents_dir(data_home).join(names::plist_file_name(app_id)),
            socket: home.run_dir().join("apps").join(names::socket_file_name(app_id)),
        }
    }
}

impl Paths {
    /// `data_home` 为数据目录（Linux `$XDG_DATA_HOME`，Windows `%LOCALAPPDATA%`）。
    pub fn new(data_home: &Path, home: &AppHome, app_id: &str) -> Self {
        Self {
            service_file: service_file_path(data_home, app_id),
            launchd: cfg!(target_os = "macos").then(|| LaunchdPaths::new(data_home, home, app_id)),
            registration_file: registration::apps_dir(data_home).join(registration::file_name(app_id)),
            manifest_copy: home.manifest_dir().join(format!("{app_id}.json")),
        }
    }

    fn all(&self) -> Vec<&PathBuf> {
        self.service_file
            .iter()
            .chain(self.launchd.iter().flat_map(|l| [&l.plist, &l.socket]))
            .chain([&self.registration_file, &self.manifest_copy])
            .collect()
    }
}

#[cfg(target_os = "linux")]
fn service_file_path(data_home: &Path, app_id: &str) -> Option<PathBuf> {
    let name = app_mcp_protocol::naming::dbus::service_file_name(app_id);
    Some(data_home.join("dbus-1").join("services").join(name))
}

#[cfg(not(target_os = "linux"))]
fn service_file_path(_data_home: &Path, _app_id: &str) -> Option<PathBuf> {
    None
}

/// 本平台的激活方式（spec/naming.md 5.3 `activation`）。
#[cfg(target_os = "linux")]
fn activation_for(address: &Address, _paths: &Paths) -> anyhow::Result<Activation> {
    Ok(Activation {
        kind: registration::kinds::DBUS.to_owned(),
        target: app_mcp_protocol::naming::dbus::bus_name(address),
    })
}

/// Windows：`exec`，`target` 留空 = 运行 `executable`；macOS：`launchd`，`target` = 套接字路径（4.4）。
#[cfg(not(target_os = "linux"))]
fn activation_for(_address: &Address, paths: &Paths) -> anyhow::Result<Activation> {
    match &paths.launchd {
        Some(l) => launchd_activation(&l.socket),
        None => Ok(Activation { kind: registration::kinds::EXEC.to_owned(), target: String::new() }),
    }
}

/// `launchd` 激活方式：套接字须能放进 `sockaddr_un`（macOS 103 字节），否则安装前即失败（IPC_PATH_TOO_LONG）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launchd_activation(socket: &Path) -> anyhow::Result<Activation> {
    #[cfg(unix)]
    app_mcp_protocol::endpoint::check_unix_socket_path(socket).map_err(|issue| anyhow::anyhow!("{issue}（可用 --home 指定较短的目录）"))?;
    let target = socket.to_str().with_context(|| format!("套接字路径不是 UTF-8：{}", socket.display()))?;
    Ok(Activation { kind: registration::kinds::LAUNCHD.to_owned(), target: target.to_owned() })
}

pub async fn cmd(action: AppAction) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        cfg!(any(target_os = "linux", target_os = "macos", windows)),
        "本平台尚未实现按名寻址的 App 登记（spec/naming.md 4.0：目前有 Linux D-Bus、Windows 命名管道与 macOS launchd）"
    );
    let (target, reload) = match action {
        AppAction::Install(args) => {
            let paths = install(&args)?;
            println!("已登记 {}：", args.app_id);
            if let Some(service) = &paths.service_file {
                println!("  激活文件  {}", service.display());
            }
            if let Some(l) = &paths.launchd {
                println!("  Agent     {}", l.plist.display());
                println!("  套接字    {}", l.socket.display());
                #[cfg(target_os = "macos")]
                if !args.target.no_reload {
                    launchd::load(&args.app_id, l).await?;
                }
            }
            println!("  登记文件  {}", paths.registration_file.display());
            if args.manifest.is_some() {
                println!("  清单      {}（Host 重启后生效）", paths.manifest_copy.display());
            }
            (args.target, true)
        }
        AppAction::Uninstall(args) => {
            #[cfg(target_os = "macos")]
            if !args.target.no_reload {
                launchd::unload(&args.app_id).await;
            }
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

/// 数据目录：`--data-home` > 平台默认（[`crate::data_home::user_data_home`]，与 doctor、Hub 读取的位置相同）。
fn data_home(target: &AppTargetArgs) -> anyhow::Result<PathBuf> {
    if let Some(d) = &target.data_home {
        return absolute(d);
    }
    crate::data_home::user_data_home()
        .with_context(|| format!("无法确定用户数据目录（{}）；请用 --data-home 指定", crate::data_home::SOURCE))
}

/// 登记：校验参数后写登记文件、（Linux）激活文件与清单副本。返回写入的位置。
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
    // @why Windows 的 canonicalize 返回 `\\?\C:\…`：写成普通形式，与进程映像路径一致（spec/naming.md 4.3 身份核对）。
    let exec_text = pipe_names::strip_verbatim(
        exec.to_str().with_context(|| format!("程序路径不是 UTF-8：{}", exec.display()))?,
    );

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
    let manifest_copy = match &manifest {
        Some(_) => Some(
            paths.manifest_copy.to_str().map(pipe_names::strip_verbatim).with_context(|| {
                format!("清单副本路径不是 UTF-8：{}", paths.manifest_copy.display())
            })?,
        ),
        None => None,
    };
    let registration = Registration {
        registration_version: REGISTRATION_VERSION,
        app_id: args.app_id.clone(),
        name,
        source: "manual".to_owned(),
        manifest: manifest_copy,
        manifest_sha256: manifest.as_ref().map(|(_, _, b)| format!("{:x}", Sha256::digest(b))),
        executable: Some(exec_text.clone()),
        activation: activation_for(&address, &paths)?,
    };
    let plist = match &paths.launchd {
        Some(_) => Some(
            app_mcp_protocol::naming::launchd::agent_plist(&args.app_id, &exec_text, &registration.activation.target)
                .map_err(|e| anyhow::anyhow!("无法生成 launchd plist：{e}"))?,
        ),
        None => None,
    };

    if let Some((_, _, bytes)) = &manifest {
        write_file(&paths.manifest_copy, bytes)?;
    }
    let mut json = serde_json::to_string_pretty(&registration)?;
    json.push('\n');
    write_file(&paths.registration_file, json.as_bytes())?;
    if let Some(service) = &paths.service_file {
        write_file(service, app_mcp_protocol::naming::dbus::service_file(&args.app_id, &exec_text).as_bytes())?;
    }
    if let (Some(l), Some(plist)) = (&paths.launchd, &plist) {
        write_file(&l.plist, plist.as_bytes())?;
        prepare_socket_dir(&l.socket)?;
    }
    Ok(paths)
}

/// 套接字目录：只有当前用户可进入（`0700`，spec/protocol.md 1.4 同样的要求，Hub 拨号前核对）；删除残留的旧套接字
/// （launchd 是否先删除再绑定未见文档，U-23）。
#[cfg_attr(not(unix), allow(dead_code))]
fn prepare_socket_dir(socket: &Path) -> anyhow::Result<()> {
    let dir = socket.parent().with_context(|| format!("{} 没有上级目录", socket.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("创建目录 {} 失败", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("设置 {} 的权限失败", dir.display()))?;
    }
    match std::fs::remove_file(socket) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("删除旧套接字 {} 失败：{e}", socket.display())),
    }
}

/// `launchctl bootstrap / bootout`（gui/<uid> 域，launchctl(1)）；每条命令 5 秒超时。
#[cfg(target_os = "macos")]
mod launchd {
    use std::path::Path;
    use std::time::Duration;

    use anyhow::Context;
    use app_mcp_protocol::naming::launchd as names;

    use super::LaunchdPaths;

    const LAUNCHCTL: &str = "/bin/launchctl";
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn domain() -> String {
        format!("gui/{}", app_mcp_protocol::endpoint::current_uid())
    }

    async fn launchctl(args: &[&str]) -> anyhow::Result<crate::doctor::command::ToolOutput> {
        crate::doctor::command::run_tool(Path::new(LAUNCHCTL), args, TIMEOUT)
            .await
            .map_err(|e| anyhow::anyhow!("launchctl {}：{e}", args.join(" ")))
    }

    /// 载入作业：先卸下同名旧作业（未载入时的失败忽略），再 `bootstrap`。launchd 随即创建并监听套接字，不启动 App。
    pub(super) async fn load(app_id: &str, paths: &LaunchdPaths) -> anyhow::Result<()> {
        unload(app_id).await;
        let plist = paths.plist.to_str().with_context(|| format!("plist 路径不是 UTF-8：{}", paths.plist.display()))?;
        let out = launchctl(&["bootstrap", &domain(), plist]).await?;
        anyhow::ensure!(
            out.success,
            "launchctl bootstrap {} {plist} 失败：{}（检查 plist：plutil -lint {plist}）",
            domain(),
            out.stderr.trim()
        );
        Ok(())
    }

    /// 卸下作业（`bootout gui/<uid>/<label>`）；未载入等失败只提示。
    pub(super) async fn unload(app_id: &str) {
        let target = format!("{}/{}", domain(), names::label(app_id));
        match launchctl(&["bootout", &target]).await {
            Ok(o) if o.success => println!("已卸下 launchd 作业 {target}"),
            Ok(_) => {}
            Err(e) => eprintln!("提示：未能卸下 launchd 作业（{e}）"),
        }
    }
}

/// 撤销登记：删除 install 写入的文件，返回实际删除的路径。
pub fn uninstall(args: &AppUninstallArgs) -> anyhow::Result<Vec<PathBuf>> {
    Address::new(&args.app_id, None).map_err(|e| anyhow::anyhow!("--app-id 无效：{e}"))?;
    let home = AppHome::resolve(args.target.home.home.as_deref())?;
    let paths = Paths::new(&data_home(&args.target)?, &home, &args.app_id);
    let mut removed = Vec::new();
    for p in paths.all() {
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
/// Windows 不需要：Hub 经目录变更通知读到登记文件。
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
        let exe = std::fs::canonicalize(&exe).unwrap();
        let exe_text = pipe_names::strip_verbatim(exe.to_str().unwrap());
        let reg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.registration_file).unwrap()).unwrap();
        assert_eq!(paths.registration_file, dir.join("data").join("app-mcp").join("apps").join("my-shop.json"));
        assert_eq!(reg["registrationVersion"], 1);
        assert_eq!(reg["appId"], "my-shop");
        assert_eq!(reg["name"], "我的商城");
        assert_eq!(reg["source"], "manual");
        assert_eq!(reg["executable"], exe_text);
        assert_eq!(reg["manifestSha256"].as_str().map(str::len), Some(64));
        assert_eq!(reg["manifest"].as_str().map(PathBuf::from), Some(paths.manifest_copy.clone()));
        assert!(paths.manifest_copy.is_file());
        // Hub 读登记文件用的是同一个类型与校验（spec/naming.md 5.3）。
        let parsed = registration::parse(&std::fs::read_to_string(&paths.registration_file).unwrap(), "my-shop").unwrap();
        assert_eq!(parsed.executable.as_deref(), Some(exe_text.as_str()));
        if cfg!(target_os = "linux") {
            assert_eq!(reg["activation"]["kind"], "dbus");
            assert_eq!(reg["activation"]["target"], "dev.appmcp.App.my_shop");
            let service_path = paths.service_file.clone().unwrap();
            assert_eq!(service_path, dir.join("data/dbus-1/services/dev.appmcp.App.my_shop.service"));
            let service = std::fs::read_to_string(&service_path).unwrap();
            assert_eq!(
                service,
                format!("[D-BUS Service]\nName=dev.appmcp.App.my_shop\nExec=\"{}\" --app-mcp-activation\n", exe.display())
            );
        } else if cfg!(target_os = "macos") {
            // macOS：launchd 按需套接字（spec/naming.md 4.4），Agent plist 与套接字目录随登记写出。
            let l = paths.launchd.clone().unwrap();
            assert_eq!(reg["activation"]["kind"], "launchd");
            assert_eq!(reg["activation"]["target"], l.socket.to_str().unwrap());
            assert_eq!(l.plist, dir.join("LaunchAgents/dev.appmcp.App.my-shop.plist"));
            let plist = std::fs::read_to_string(&l.plist).unwrap();
            assert!(plist.contains(&format!("<string>{}</string>", exe.display())), "{plist}");
            assert!(l.socket.parent().unwrap().is_dir());
        } else {
            // Windows：Hub 直接运行 executable（spec/naming.md 4.3 `exec`），没有激活文件；路径不带 `\\?\`。
            assert_eq!(reg["activation"]["kind"], "exec");
            assert_eq!(reg["activation"]["target"], "");
            assert!(paths.service_file.is_none());
            assert!(!exe_text.starts_with(r"\\?\"), "{exe_text}");
        }
        // 幂等：再装一次覆盖。
        install(&args).unwrap();

        // macOS 的套接字由 launchd 创建（此处未载入作业，不存在）：只数存在的文件。
        let existing = paths.all().iter().filter(|p| p.exists()).count();
        let removed =
            uninstall(&AppUninstallArgs { app_id: "my-shop".into(), target: target(&dir) }).unwrap();
        assert_eq!(removed.len(), existing);
        assert!(existing >= 2, "至少有登记文件与清单副本");
        assert!(paths.all().iter().all(|p| !p.exists()));
        let again = uninstall(&AppUninstallArgs { app_id: "my-shop".into(), target: target(&dir) }).unwrap();
        assert!(again.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// macOS 的登记位置、激活方式与套接字目录（纯逻辑，各平台都运行；launchctl 只在 Mac 上验证）。
    #[cfg(unix)]
    #[test]
    fn launchd_layout_and_activation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let home = AppHome::resolve(Some(&dir.join("home"))).unwrap();
        let data = dir.join("Library").join("Application Support");
        let l = LaunchdPaths::new(&data, &home, "my-shop");
        assert_eq!(l.plist, dir.join("Library/LaunchAgents/dev.appmcp.App.my-shop.plist"));
        assert_eq!(l.socket, home.run_dir().join("apps").join("my-shop.sock"));

        let a = launchd_activation(&l.socket).unwrap();
        assert_eq!((a.kind.as_str(), a.target.as_str()), ("launchd", l.socket.to_str().unwrap()));
        let long = PathBuf::from(format!("/{}/x.sock", "d".repeat(200)));
        assert!(launchd_activation(&long).unwrap_err().to_string().contains("IPC_PATH_TOO_LONG"));

        // 套接字目录 0700，残留的旧套接字被删除。
        std::fs::create_dir_all(l.socket.parent().unwrap()).unwrap();
        std::fs::write(&l.socket, b"stale").unwrap();
        prepare_socket_dir(&l.socket).unwrap();
        let mode = std::fs::metadata(l.socket.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(!l.socket.exists());
        prepare_socket_dir(&l.socket).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `app install` 写入的登记目录就是 Hub 的 launchd 连接器读取的目录（spec/naming.md 5.3）。
    #[cfg(target_os = "macos")]
    #[test]
    fn matches_launchd_connector_apps_dir() {
        let ours = crate::data_home::user_data_home().map(|d| registration::apps_dir(&d));
        assert_eq!(ours, app_mcp_hub::connector::LaunchdConnector::default_apps_dir());
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
