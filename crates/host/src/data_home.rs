//! 当前用户的数据目录：App 登记文件（spec/naming.md 5.3）与 D-Bus 激活文件（4.1）的根。
//!
//! `app install` / `app uninstall` 写入与 `doctor` 的名字服务检查读取同一位置，二者都经 [`user_data_home`]。

use std::ffi::OsString;
use std::path::PathBuf;

/// 平台默认数据目录的来源（用于"取不到"时的说明）。
#[cfg(windows)]
pub const SOURCE: &str = "环境变量 LOCALAPPDATA";
#[cfg(target_os = "macos")]
pub const SOURCE: &str = "用户主目录下的 Library/Application Support";
#[cfg(all(not(windows), not(target_os = "macos")))]
pub const SOURCE: &str = "$XDG_DATA_HOME 或用户主目录下的 .local/share";

/// 当前用户的数据目录；取不到时为 `None`（调用方说明来源 [`SOURCE`]）。
///
/// @invariant Windows 为 `%LOCALAPPDATA%`（绝对路径），与 Hub 的 `PipeConnector::default_apps_dir` 读取的根相同；
/// macOS 为 `~/Library/Application Support`；其他 Unix 为 `$XDG_DATA_HOME`（绝对路径时）否则 `~/.local/share`。
pub fn user_data_home() -> Option<PathBuf> {
    resolve(|key| std::env::var_os(key), dirs::home_dir())
}

/// [`user_data_home`] 的规则，环境变量与主目录由参数给出（测试注入）。
#[cfg(windows)]
fn resolve(var: impl Fn(&str) -> Option<OsString>, _home: Option<PathBuf>) -> Option<PathBuf> {
    absolute_var(&var, "LOCALAPPDATA")
}

#[cfg(target_os = "macos")]
fn resolve(_var: impl Fn(&str) -> Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    Some(home?.join("Library").join("Application Support"))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn resolve(var: impl Fn(&str) -> Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    absolute_var(&var, "XDG_DATA_HOME").or_else(|| Some(home?.join(".local").join("share")))
}

/// 环境变量的值，且须为绝对路径（相对路径按未设置处理）。
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn absolute_var(var: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<PathBuf> {
    var(key).map(PathBuf::from).filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| OsString::from(*v))
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    #[test]
    fn xdg_data_home_then_home_local_share() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(resolve(env(&[("XDG_DATA_HOME", "/data")]), home.clone()), Some(PathBuf::from("/data")));
        assert_eq!(resolve(env(&[("XDG_DATA_HOME", "rel")]), home.clone()), Some(PathBuf::from("/home/u/.local/share")));
        assert_eq!(resolve(env(&[]), home), Some(PathBuf::from("/home/u/.local/share")));
        assert_eq!(resolve(env(&[]), None), None);
    }

    #[cfg(windows)]
    #[test]
    fn localappdata_only_when_absolute() {
        assert_eq!(resolve(env(&[("LOCALAPPDATA", r"C:\Users\u\AppData\Local")]), None), Some(PathBuf::from(r"C:\Users\u\AppData\Local")));
        assert_eq!(resolve(env(&[("LOCALAPPDATA", "rel")]), Some(PathBuf::from(r"C:\Users\u"))), None);
        assert_eq!(resolve(env(&[]), Some(PathBuf::from(r"C:\Users\u"))), None);
    }

    /// 与 Hub 的 Windows 连接器读取同一登记目录（spec/naming.md 5.3）。
    #[cfg(windows)]
    #[test]
    fn matches_pipe_connector_apps_dir() {
        let ours = user_data_home().map(|d| app_mcp_protocol::naming::registration::apps_dir(&d));
        assert_eq!(ours, app_mcp_hub::connector::PipeConnector::default_apps_dir());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_application_support() {
        assert_eq!(resolve(env(&[]), Some(PathBuf::from("/Users/u"))), Some(PathBuf::from("/Users/u/Library/Application Support")));
        assert_eq!(resolve(env(&[]), None), None);
    }
}
