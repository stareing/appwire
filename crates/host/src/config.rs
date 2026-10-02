//! 配置目录与配置文件（`~/.app-mcp/config.json`）。
//!
//! 优先级：命令行参数 > 配置文件 > 默认值。配置目录：`--home` > 环境变量 `APP_MCP_HOME` > `~/.app-mcp`。

use std::path::{Path, PathBuf};

use anyhow::Context;

mod file;
mod overrides;
mod settings;

pub use file::{AuthMode, FileConfig, HttpSection, LifecycleSection, LogSection, McpSection, ToolsSection};
pub use overrides::Overrides;
pub use settings::Settings;

/// 默认的 HTTP 监听地址：同一端口承载 `/app`（App 连接）、`/mcp`、`/healthz`。
pub const DEFAULT_LISTEN_ADDR: &str = app_mcp_protocol::DEFAULT_LISTEN_ADDR;
/// 关闭本地 IPC 服务时 `ipcEndpoint` / `--ipc-endpoint` 的取值。
pub const IPC_NONE: &str = "none";
/// 配置目录环境变量。
pub const HOME_ENV: &str = app_mcp_protocol::registry::HOME_ENV;

/// 配置目录（`~/.app-mcp`）及其中的固定文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppHome {
    pub dir: PathBuf,
}

impl AppHome {
    /// `--home` > `APP_MCP_HOME` > `~/.app-mcp`。结果为绝对路径。
    pub fn resolve(explicit: Option<&Path>) -> anyhow::Result<Self> {
        let dir = match explicit {
            Some(d) => d.to_path_buf(),
            None => match std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
                Some(d) => PathBuf::from(d),
                None => dirs::home_dir()
                    .context("无法确定用户主目录；请用 --home 或 APP_MCP_HOME 指定配置目录")?
                    .join(".app-mcp"),
            },
        };
        Ok(Self {
            dir: absolute(&dir)?,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.json")
    }
    pub fn token_file(&self) -> PathBuf {
        self.dir.join("token")
    }
    /// 策略规则（spec/hub-api.md 3.13）：启动时加载，`app-mcp-host policy reload` 重载。
    pub fn policy_file(&self) -> PathBuf {
        self.dir.join("policy.json")
    }
    /// 已登记的 Agent 与其令牌（第 16 项 N5；0600）。
    pub fn agents_file(&self) -> PathBuf {
        self.dir.join("agents.json")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }
    pub fn manifest_dir(&self) -> PathBuf {
        self.dir.join("manifests")
    }
    /// 运行时目录：单实例锁 `hub.lock` 与登记文件 `endpoints.json`（spec/protocol.md 1.5、1.7）。
    pub fn run_dir(&self) -> PathBuf {
        app_mcp_protocol::registry::run_dir(&self.dir)
    }
    pub fn registry_file(&self) -> PathBuf {
        self.run_dir().join(app_mcp_protocol::registry::REGISTRY_FILE)
    }
    /// 持久状态目录（`HubConfig::state_dir`）：休眠记录 `dormant/<appId>.json`（spec/hub-api.md 3.5「持久化」）。
    pub fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }
}

/// 转为绝对路径（不要求存在），并展开开头的 `~/`。
pub fn absolute(p: &Path) -> anyhow::Result<PathBuf> {
    let p = expand_tilde(p);
    if p.is_absolute() {
        return Ok(p);
    }
    Ok(std::env::current_dir().context("无法读取当前目录")?.join(p))
}

fn expand_tilde(p: &Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_path_buf();
    };
    let rest = if s == "~" {
        Some("")
    } else {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    };
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests;
