//! Host 身份（spec/protocol.md 1.6）：握手结果、`/healthz` 与登记文件中携带的
//! `service` / `version` / `user` / `pid`，以及 SDK 侧的核对规则。
//!
//! - `service` 固定为 [`SERVICE_NAME`]：SDK 据此判断对端确实是 app-mcp Host（而不是占用了端口的其他程序）。
//! - `user` 为 Host 进程的操作系统用户：Unix 为十进制有效 uid，Windows 为用户 SID（`S-1-5-21-…`）。
//!   原生 SDK 核对它与自己的用户相同（[`expected_host_user`]），防止连到本机其他用户的 Host。

use serde::{Deserialize, Serialize};

use crate::HelloResult;

/// Host 身份中的服务名。
pub const SERVICE_NAME: &str = "app-mcp";

/// 一个 Host 进程的身份。`/healthz`、登记文件（[`crate::registry`]）与诊断共用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    /// 固定为 [`SERVICE_NAME`]。
    pub service: String,
    /// Host / Hub 版本。
    pub version: String,
    /// 进程的操作系统用户（见模块说明）；取不到时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// 进程号。
    pub pid: u32,
}

impl HostIdentity {
    /// 当前进程的身份。
    pub fn current(version: &str) -> Self {
        Self {
            service: SERVICE_NAME.to_owned(),
            version: version.to_owned(),
            user: current_user(),
            pid: std::process::id(),
        }
    }

    /// 是否为 app-mcp。
    pub fn is_app_mcp(&self) -> bool {
        self.service == SERVICE_NAME
    }
}

/// 当前进程的操作系统用户标识：Unix 为十进制有效 uid，Windows 为用户 SID；其他平台为 `None`。
pub fn current_user() -> Option<String> {
    #[cfg(unix)]
    {
        Some(crate::endpoint::current_uid().to_string())
    }
    #[cfg(windows)]
    {
        crate::endpoint::win::current_user_sid().ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// SDK 期望的 Host 用户：桌面平台为 [`current_user`]；Android / iOS 为 `None`（App 各有独立 uid，
/// Host 运行在另一台机器上并经 `adb reverse` 等转发，用户不可比较）。
pub fn expected_host_user() -> Option<String> {
    if cfg!(any(target_os = "android", target_os = "ios")) {
        None
    } else {
        current_user()
    }
}

/// 核对握手结果中的 Host 身份。`Err` 为面向用户的原因（中文）。
///
/// - `service` 存在且不是 [`SERVICE_NAME`] → 对端不是 app-mcp Host；
/// - 给出了 `expected_user`，且结果带 `user` 而与之不同 → Host 属于其他用户。
///
/// 旧 Host 不带这些字段：无法核对，视为通过。
pub fn check_hello(result: &HelloResult, expected_user: Option<&str>) -> Result<(), String> {
    if let Some(service) = result.service.as_deref()
        && service != SERVICE_NAME
    {
        return Err(format!(
            "对端不是 app-mcp Host（service = {service:?}）：该端口被其他程序占用。请检查端点配置，或停止占用端口的程序"
        ));
    }
    if let (Some(expected), Some(user)) = (expected_user, result.user.as_deref())
        && expected != user
    {
        let pid = result.pid.map(|p| format!("，pid {p}")).unwrap_or_default();
        return Err(format!(
            "该端点上的 app-mcp Host 属于其他用户（{user}{pid}，本进程用户 {expected}）：不会连接其他用户的 Host。\
             请启动自己的 Host（app-mcp-host serve），或用 APP_MCP_ENDPOINT 指定自己的端点"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_round_trip() {
        let id = HostIdentity::current("1.2.3");
        assert!(id.is_app_mcp());
        assert_eq!(id.pid, std::process::id());
        #[cfg(any(unix, windows))]
        assert!(id.user.is_some());
        let v = serde_json::to_value(&id).unwrap();
        assert_eq!(v["service"], "app-mcp");
        assert_eq!(serde_json::from_value::<HostIdentity>(v).unwrap(), id);
    }

    #[test]
    fn hello_identity_check() {
        let mut r = HelloResult::default();
        // 旧 Host：没有身份字段
        assert!(check_hello(&r, Some("1000")).is_ok());
        r.service = Some("app-mcp".into());
        r.user = Some("1000".into());
        assert!(check_hello(&r, Some("1000")).is_ok());
        assert!(check_hello(&r, None).is_ok());
        let e = check_hello(&r, Some("1001")).unwrap_err();
        assert!(e.contains("其他用户"), "{e}");
        r.service = Some("other".into());
        let e = check_hello(&r, None).unwrap_err();
        assert!(e.contains("不是 app-mcp"), "{e}");
    }
}
