//! 登记文件（spec/protocol.md 1.7）：运行中的 Host 把实际监听位置写到
//! `<配置目录>/run/endpoints.json`，供原生 SDK、命令行与测试读取。
//!
//! - 配置目录：环境变量 [`HOME_ENV`]（`APP_MCP_HOME`）> `~/.app-mcp`（与 `app-mcp-host --home` 的缺省一致）。
//! - 同一目录下的 [`LOCK_FILE`]（`hub.lock`）是单实例锁：Host 在任何监听之前取得它（`flock` / `LockFileEx`），
//!   退出时由操作系统释放。登记文件由持锁的 Host 原子写入（权限 0600），正常退出时删除；
//!   异常退出留下的旧文件会被下一个取得锁的 Host 覆盖。
//! - 本模块只负责路径与读取；写入、加锁在 `app-mcp-hub` 中实现。

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::identity::HostIdentity;

/// 配置目录环境变量。
pub const HOME_ENV: &str = "APP_MCP_HOME";
/// 配置目录下的运行时目录名。
pub const RUN_DIR: &str = "run";
/// 登记文件名。
pub const REGISTRY_FILE: &str = "endpoints.json";
/// 单实例锁文件名。
pub const LOCK_FILE: &str = "hub.lock";

/// 登记文件的内容。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointRegistry {
    /// Host 身份（`service` / `version` / `user` / `pid`）。
    #[serde(flatten)]
    pub identity: HostIdentity,
    /// HTTP 服务（`/app`、`/mcp`、`/healthz`）实际监听的地址（`host:port`）；未开启时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// 本地 IPC 端点（`unix:…` / `pipe:…`）；未开启时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_endpoint: Option<String>,
    /// 启动时刻（Unix 毫秒）。
    pub started_at_ms: u64,
}

impl EndpointRegistry {
    /// 读取登记文件：不存在时返回 `Ok(None)`；内容不合法时返回 `InvalidData`。
    pub fn read(path: &Path) -> io::Result<Option<Self>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}：{e}", path.display())))
    }

    /// App 连接端点：有 IPC 端点时用它，否则为 `ws://<listen>/app`。
    pub fn app_endpoint(&self) -> Option<String> {
        if let Some(ipc) = &self.ipc_endpoint {
            return Some(ipc.clone());
        }
        self.listen.as_ref().map(|addr| format!("ws://{addr}{}", crate::APP_PATH))
    }

    /// MCP Streamable HTTP 地址（`http://<listen>/mcp`）。
    pub fn mcp_url(&self) -> Option<String> {
        self.listen.as_ref().map(|addr| format!("http://{addr}{}", crate::MCP_PATH))
    }
}

/// 默认配置目录：`APP_MCP_HOME`（非空时；相对路径按当前目录）> `<主目录>/.app-mcp`。
/// Android / iOS / WASM 上为 `None`（这些平台不与 Host 共享文件系统）。
pub fn default_home() -> Option<PathBuf> {
    if cfg!(any(target_os = "android", target_os = "ios", not(any(unix, windows)))) {
        return None;
    }
    if let Some(dir) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
        return std::path::absolute(PathBuf::from(dir)).ok();
    }
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(home_var)
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .map(|h| h.join(".app-mcp"))
}

/// 配置目录下的运行时目录（锁与登记文件所在）。
pub fn run_dir(home: &Path) -> PathBuf {
    home.join(RUN_DIR)
}

/// 默认登记文件路径（[`default_home`]`/run/endpoints.json`）。
pub fn default_registry_path() -> Option<PathBuf> {
    default_home().map(|h| run_dir(&h).join(REGISTRY_FILE))
}

/// 读取默认登记文件并给出 App 连接端点；没有登记文件或内容不合法时为 `None`。
pub fn registered_app_endpoint() -> Option<String> {
    let path = default_registry_path()?;
    EndpointRegistry::read(&path).ok().flatten()?.app_endpoint()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EndpointRegistry {
        EndpointRegistry {
            identity: HostIdentity {
                service: "app-mcp".into(),
                version: "0.1.0".into(),
                user: Some("1000".into()),
                pid: 7,
            },
            listen: Some("127.0.0.1:7737".into()),
            ipc_endpoint: None,
            started_at_ms: 1,
        }
    }

    #[test]
    fn json_shape_and_endpoints() {
        let r = sample();
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["service"], "app-mcp");
        assert_eq!(v["listen"], "127.0.0.1:7737");
        assert_eq!(v["startedAtMs"], 1);
        assert!(v.get("ipcEndpoint").is_none());
        assert_eq!(r.app_endpoint().as_deref(), Some("ws://127.0.0.1:7737/app"));
        assert_eq!(r.mcp_url().as_deref(), Some("http://127.0.0.1:7737/mcp"));
        let with_ipc = EndpointRegistry {
            ipc_endpoint: Some("unix:/run/user/1/app-mcp/hub.sock".into()),
            ..r
        };
        assert_eq!(with_ipc.app_endpoint().as_deref(), Some("unix:/run/user/1/app-mcp/hub.sock"));
    }

    #[test]
    fn read_missing_and_invalid() {
        let dir = std::env::temp_dir().join(format!("app-mcp-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(REGISTRY_FILE);
        assert_eq!(EndpointRegistry::read(&path).unwrap(), None);
        std::fs::write(&path, "{").unwrap();
        assert_eq!(EndpointRegistry::read(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        std::fs::write(&path, serde_json::to_string(&sample()).unwrap()).unwrap();
        assert_eq!(EndpointRegistry::read(&path).unwrap(), Some(sample()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
