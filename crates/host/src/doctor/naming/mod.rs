//! doctor 的名字服务检查组（spec/naming.md 第 11 节，检查项 ID 以 `naming.` 开头）。
//!
//! 按平台从 [`NAMING_CHECKS`] 表中选出要运行的检查（各平台的连接器均已实现，不做假检查）。
//! 每项检查分两步：探测（读文件、问总线、跑 adb，都有超时）→ 评估（纯函数，单元测试用假数据覆盖）。
//!
//! 只读：不激活任何名字（`ListActivatableNames` / `ListNames` 不触发激活）、不打开 `appmcp-` 管道（只列名字）、不 `bindService`、
//! 不改系统设置。

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use serde::Serialize;

use super::{Check, Level};

mod android;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod dbus;
// @why 探测只在 macOS 上运行；评估规则在各平台测试。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod launchd;
mod pipes;
mod registration;

/// 名字服务探测的单项超时（总线调用、每条 adb 命令、列出管道；B-08）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 平台。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Windows,
    MacOs,
    Other,
}

impl Platform {
    /// 编译目标的平台。
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Platform::Linux
        } else if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Other
        }
    }
}

/// 探测所需的环境（目录、总线地址、adb）；测试中以临时目录构造。
#[derive(Clone, Debug)]
pub struct NamingEnv {
    /// App 登记文件目录（spec/naming.md 5.3），用户级在前。
    pub registration_dirs: Vec<PathBuf>,
    /// D-Bus 会话服务激活文件目录（spec/naming.md 4.1），用户级在前。
    pub dbus_service_dirs: Vec<PathBuf>,
    /// D-Bus 地址；`None` = 当前用户的会话总线。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // @why 只有 Linux 的 naming.dbus 检查读取
    pub dbus_address: Option<String>,
    /// PATH 中的 `adb`。
    pub adb: Option<PathBuf>,
    pub timeout: Duration,
}

impl NamingEnv {
    /// 按当前用户与平台的约定位置构造。
    ///
    /// @input 用户级数据目录（[`crate::data_home::user_data_home`]，与 `app install` 写入的位置相同）；
    /// Linux 另加 `$XDG_DATA_DIRS`（缺省 `/usr/local/share:/usr/share`）。
    pub fn from_system() -> Self {
        let user = crate::data_home::user_data_home();
        let system: Vec<PathBuf> = if cfg!(target_os = "linux") { xdg_data_dirs(std::env::var_os("XDG_DATA_DIRS")) } else { Vec::new() };
        let data_dirs: Vec<PathBuf> = user.into_iter().chain(system).collect();
        Self {
            registration_dirs: data_dirs.iter().map(|d| d.join("app-mcp").join("apps")).collect(),
            dbus_service_dirs: data_dirs.iter().map(|d| d.join("dbus-1").join("services")).collect(),
            dbus_address: None,
            adb: super::find_in_path("adb"),
            timeout: PROBE_TIMEOUT,
        }
    }
}

/// `$XDG_DATA_DIRS` 中的绝对路径；未设置或为空时取规范缺省值。
fn xdg_data_dirs(value: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let value = value.filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::env::split_paths(&value).filter(|p| p.is_absolute()).collect()
}

/// 一条发现：级别 + 说明（+ 修复建议）。一项检查的级别取其发现中最严重的一个。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub level: Level,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Finding {
    fn new(level: Level, text: impl Into<String>) -> Self {
        Self { level, text: text.into(), hint: None }
    }
    fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }
}

/// 级别的严重程度（汇总时取最大）。
fn severity(l: Level) -> u8 {
    match l {
        Level::Skip => 0,
        Level::Ok => 1,
        Level::Info => 2,
        Level::Warn => 3,
        Level::Error => 4,
    }
}

/// 最严重的级别；没有发现时为 `fallback`。
fn worst<'a>(findings: impl IntoIterator<Item = &'a Finding>, fallback: Level) -> Level {
    findings.into_iter().map(|f| f.level).max_by_key(|l| severity(*l)).unwrap_or(fallback)
}

/// 发现中的修复建议，去重后以「；」连接。
fn joined_hints<'a>(findings: impl IntoIterator<Item = &'a Finding>) -> Option<String> {
    let mut hints: Vec<&str> = Vec::new();
    for h in findings.into_iter().filter_map(|f| f.hint.as_deref()) {
        if !hints.contains(&h) {
            hints.push(h);
        }
    }
    (!hints.is_empty()).then(|| hints.join("；"))
}

/// 名字服务错误码（spec/naming.md 第 12 节，尚未并入 `ConnectionErrorCode`）。
fn with_naming_code(mut c: Check, code: &'static str) -> Check {
    c.code = Some(code);
    c
}

type CheckFuture<'a> = Pin<Box<dyn Future<Output = Check> + Send + 'a>>;

/// 检查表的一行。
struct NamingCheck {
    id: &'static str,
    platforms: &'static [Platform],
    run: for<'a> fn(&'a NamingEnv) -> CheckFuture<'a>,
}

const DESKTOP: &[Platform] = &[Platform::Linux, Platform::Windows, Platform::MacOs];
const ANY: &[Platform] = &[Platform::Linux, Platform::Windows, Platform::MacOs, Platform::Other];

/// 名字服务检查表（C-12：按平台选择，不堆分支）。
const NAMING_CHECKS: &[NamingCheck] = &[
    NamingCheck { id: registration::ID, platforms: DESKTOP, run: run_registrations },
    NamingCheck { id: dbus::ID, platforms: &[Platform::Linux], run: run_dbus },
    NamingCheck { id: pipes::ID, platforms: &[Platform::Windows], run: run_pipes },
    NamingCheck { id: launchd::ID, platforms: &[Platform::MacOs], run: run_launchd },
    NamingCheck { id: android::ID, platforms: ANY, run: run_android },
];

fn run_registrations(env: &NamingEnv) -> CheckFuture<'_> {
    Box::pin(async move { registration::check(env) })
}
fn run_dbus(env: &NamingEnv) -> CheckFuture<'_> {
    Box::pin(dbus::check(env))
}
fn run_pipes(env: &NamingEnv) -> CheckFuture<'_> {
    Box::pin(pipes::check(env))
}
fn run_launchd(env: &NamingEnv) -> CheckFuture<'_> {
    Box::pin(launchd::check(env))
}
fn run_android(env: &NamingEnv) -> CheckFuture<'_> {
    Box::pin(android::check(env))
}

/// `platform` 上要运行的检查（按表顺序）。
fn selected(platform: Platform) -> impl Iterator<Item = &'static NamingCheck> {
    NAMING_CHECKS.iter().filter(move |c| c.platforms.contains(&platform))
}

/// 运行本平台的名字服务检查。
pub async fn run(env: &NamingEnv) -> Vec<Check> {
    let mut out = Vec::new();
    for c in selected(Platform::current()) {
        let check = (c.run)(env).await;
        debug_assert_eq!(check.id, c.id, "检查表的 ID 与检查结果一致");
        out.push(check);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_for(platform: Platform) -> Vec<&'static str> {
        selected(platform).map(|c| c.id).collect()
    }

    #[test]
    fn check_table_selects_by_platform() {
        assert_eq!(ids_for(Platform::Linux), ["naming.registrations", "naming.dbus", "naming.android"]);
        assert_eq!(ids_for(Platform::Windows), ["naming.registrations", "naming.pipes", "naming.android"]);
        assert_eq!(ids_for(Platform::MacOs), ["naming.registrations", "naming.launchd", "naming.android"]);
        assert_eq!(ids_for(Platform::Other), ["naming.android"]);
    }

    #[test]
    fn worst_level_and_hints() {
        let f = [
            Finding::new(Level::Info, "a").hint("x"),
            Finding::new(Level::Error, "b").hint("y"),
            Finding::new(Level::Warn, "c").hint("x"),
        ];
        assert_eq!(worst(&f, Level::Ok), Level::Error);
        assert_eq!(worst(&f[..0], Level::Ok), Level::Ok);
        assert_eq!(joined_hints(&f).as_deref(), Some("x；y"));
        assert_eq!(joined_hints(&f[..0]), None);
    }

    #[cfg(unix)]
    #[test]
    fn xdg_data_dirs_default_and_filter() {
        assert_eq!(xdg_data_dirs(None), [PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]);
        assert_eq!(xdg_data_dirs(Some("".into())).len(), 2);
        assert_eq!(xdg_data_dirs(Some("relative:/opt/share".into())), [PathBuf::from("/opt/share")]);
    }
}
