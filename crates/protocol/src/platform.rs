//! 目标平台分类（spec/protocol.md 1.3）：App 是否运行在系统沙箱中、平台有没有默认本地 IPC 端点。
//!
//! 判断只看目标三元组的 `target_family` / `target_os` / `target_env`，写成纯函数（[`Target`] 的方法）
//! 便于按任意目标参数化测试；[`Target::CURRENT`] 是编译目标本身。默认端点（[`crate::endpoint`]）、
//! 登记文件目录（[`crate::registry::default_home`]）与 Host 用户核对（[`crate::identity::expected_host_user`]）
//! 都以这里为唯一依据。
//!
//! @why 鸿蒙（HarmonyOS NEXT / OpenHarmony）目标 `aarch64-` / `x86_64-unknown-linux-ohos` 的 `target_os` 是
//! `linux`，只能由 `target_env = "ohos"` 区分；只看 `target_os` 会把它当成桌面 Linux。

/// 平台默认本地 IPC 端点的种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcKind {
    /// Unix 域套接字（`unix:`）。
    Unix,
    /// Windows 命名管道（`pipe:`）。
    Pipe,
}

/// 目标平台（目标三元组中与端点选择有关的三项）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target<'a> {
    /// `target_family`：`unix` / `windows` / 空（如 WASM）。
    pub family: &'a str,
    /// `target_os`：`linux` / `macos` / `windows` / `android` / `ios` …
    pub os: &'a str,
    /// `target_env`：`gnu` / `musl` / `msvc` / `ohos` / 空 …
    pub env: &'a str,
}

/// 系统沙箱化的移动平台的 `target_os`（每个 App 独立 uid / 沙箱，不能与 Host 共享套接字或文件）。
const SANDBOXED_OS: &[&str] = &["android", "ios", "tvos", "watchos", "visionos"];

/// 系统沙箱化平台的 `target_env`（鸿蒙：`target_os` 为 `linux`）。
const SANDBOXED_ENV: &[&str] = &["ohos"];

/// 编译目标的 `target_env`（std 没有对应的常量）。
const CURRENT_ENV: &str = if cfg!(target_env = "ohos") {
    "ohos"
} else if cfg!(target_env = "gnu") {
    "gnu"
} else if cfg!(target_env = "musl") {
    "musl"
} else if cfg!(target_env = "msvc") {
    "msvc"
} else {
    ""
};

impl Target<'static> {
    /// 编译目标本身。
    pub const CURRENT: Self = Self {
        family: std::env::consts::FAMILY,
        os: std::env::consts::OS,
        env: CURRENT_ENV,
    };
}

impl Target<'_> {
    /// App 是否运行在系统沙箱中（Android、iOS 系、鸿蒙）：与 Host 不共享文件系统，用户（uid）不可比较，
    /// Host 通常在另一台机器上经 `adb reverse` / `hdc rport` 转发。
    pub fn is_app_sandboxed(&self) -> bool {
        SANDBOXED_OS.contains(&self.os) || SANDBOXED_ENV.contains(&self.env)
    }

    /// 是否与 Host 共享文件系统（桌面 Unix / Windows）：只有这时才有默认 IPC 端点与登记文件。
    pub fn shares_host_filesystem(&self) -> bool {
        !self.is_app_sandboxed() && matches!(self.family, "unix" | "windows")
    }

    /// 平台是否允许 App 自行把界面带到前台（spec/protocol.md 3.4，`navigate_in_background` 的平台缺省）：桌面可以恢复 / 激活
    /// 自己的窗口；沙箱化的移动平台不可以（Android 10+ 限制后台启动 Activity，iOS、鸿蒙同理），WASM（浏览器标签页）也不可以。
    pub fn allows_self_foreground(&self) -> bool {
        !self.is_app_sandboxed() && matches!(self.family, "unix" | "windows")
    }

    /// 平台默认 IPC 端点的种类；没有默认 IPC 端点（沙箱平台、WASM）时为 `None`，SDK 默认用回环 WebSocket。
    pub fn default_ipc_kind(&self) -> Option<IpcKind> {
        if !self.shares_host_filesystem() {
            return None;
        }
        Some(if self.family == "windows" { IpcKind::Pipe } else { IpcKind::Unix })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(family: &'static str, os: &'static str, env: &'static str) -> Target<'static> {
        Target { family, os, env }
    }

    /// 按目标三元组参数化：每个目标的沙箱判定与默认 IPC 种类。
    #[test]
    fn classifies_targets() {
        let cases = [
            // (三元组, 目标, 沙箱, 默认 IPC)
            ("x86_64-unknown-linux-gnu", t("unix", "linux", "gnu"), false, Some(IpcKind::Unix)),
            ("x86_64-unknown-linux-musl", t("unix", "linux", "musl"), false, Some(IpcKind::Unix)),
            ("aarch64-apple-darwin", t("unix", "macos", ""), false, Some(IpcKind::Unix)),
            ("x86_64-unknown-freebsd", t("unix", "freebsd", ""), false, Some(IpcKind::Unix)),
            ("x86_64-pc-windows-msvc", t("windows", "windows", "msvc"), false, Some(IpcKind::Pipe)),
            ("x86_64-pc-windows-gnu", t("windows", "windows", "gnu"), false, Some(IpcKind::Pipe)),
            ("aarch64-linux-android", t("unix", "android", ""), true, None),
            ("aarch64-apple-ios", t("unix", "ios", ""), true, None),
            ("aarch64-apple-visionos", t("unix", "visionos", ""), true, None),
            ("aarch64-unknown-linux-ohos", t("unix", "linux", "ohos"), true, None),
            ("x86_64-unknown-linux-ohos", t("unix", "linux", "ohos"), true, None),
            ("armv7-unknown-linux-ohos", t("unix", "linux", "ohos"), true, None),
            ("wasm32-unknown-unknown", t("", "unknown", ""), false, None),
        ];
        for (triple, target, sandboxed, ipc) in cases {
            assert_eq!(target.is_app_sandboxed(), sandboxed, "{triple}");
            assert_eq!(target.default_ipc_kind(), ipc, "{triple}");
            assert_eq!(target.shares_host_filesystem(), ipc.is_some(), "{triple}");
            assert_eq!(target.allows_self_foreground(), ipc.is_some(), "{triple}：桌面可自行回到前台，沙箱平台 / WASM 不可");
        }
    }

    /// 编译目标的分类与 cfg 判定一致。
    #[test]
    fn current_target_matches_cfg() {
        let current = Target::CURRENT;
        assert_eq!(cfg!(target_env = "ohos"), current.env == "ohos");
        let sandboxed = cfg!(any(
            target_os = "android",
            target_os = "ios",
            target_os = "tvos",
            target_os = "watchos",
            target_os = "visionos",
            target_env = "ohos"
        ));
        assert_eq!(current.is_app_sandboxed(), sandboxed);
        let expected = if sandboxed || !cfg!(any(unix, windows)) {
            None
        } else if cfg!(windows) {
            Some(IpcKind::Pipe)
        } else {
            Some(IpcKind::Unix)
        };
        assert_eq!(current.default_ipc_kind(), expected);
    }
}
