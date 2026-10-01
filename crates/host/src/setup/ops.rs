//! setup / uninstall 的外部操作边界：运行 Agent 命令（[`CommandRunner`]）与 Host 服务操作（[`HostOps`]）。
//!
//! 真实实现为 [`SystemRunner`]、[`SystemHost`]；测试以假实现替换（不调用真实 `claude`、不安装真实服务）。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use app_mcp_protocol::registry::EndpointRegistry;

use crate::config::{AppHome, Settings};
use crate::doctor::{self, Report};
use crate::service;

/// 命令的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CmdOutput {
    /// 退出码；被信号结束时为 `None`。
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout 与 stderr 合并（解析提示文字用）。
    pub fn text(&self) -> String {
        match (self.stdout.trim().is_empty(), self.stderr.trim().is_empty()) {
            (_, true) => self.stdout.clone(),
            (true, false) => self.stderr.clone(),
            (false, false) => format!("{}\n{}", self.stdout, self.stderr),
        }
    }
}

/// 运行外部命令。
pub trait CommandRunner {
    /// PATH 中的程序。
    fn which(&self, program: &str) -> Option<PathBuf>;
    /// 在 `cwd` 中运行，stdin 为空；超时视为错误。
    fn run(&self, program: &Path, args: &[String], cwd: &Path) -> impl Future<Output = anyhow::Result<CmdOutput>>;
}

/// Agent 命令的默认超时：`claude mcp get` 会对服务器做健康检查，留足余量。
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// 真实的命令执行。
#[derive(Clone, Debug)]
pub struct SystemRunner {
    pub timeout: Duration,
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self { timeout: COMMAND_TIMEOUT }
    }
}

impl CommandRunner for SystemRunner {
    fn which(&self, program: &str) -> Option<PathBuf> {
        doctor::find_in_path(program)
    }

    async fn run(&self, program: &Path, args: &[String], cwd: &Path) -> anyhow::Result<CmdOutput> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let out = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| anyhow::anyhow!("{} {} 超过 {} 秒未返回", program.display(), args.join(" "), self.timeout.as_secs()))?
            .map_err(|e| anyhow::anyhow!("无法执行 {}：{e}", program.display()))?;
        Ok(CmdOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// `service install` 的结果（setup 用到的部分）。
#[derive(Clone, Debug)]
pub struct ServiceOutcome {
    /// 服务文件 / 注册表项位置。
    pub location: String,
    /// 安装前的提示（端口检查等）。
    pub messages: Vec<String>,
    /// 等待 `/healthz` 的结果：运行中实例的登记信息，或未就绪原因。
    pub registry: Result<EndpointRegistry, String>,
}

/// Host 服务操作。
pub trait HostOps {
    /// `<home>/config.json` 合并后的设置。
    fn settings(&self, home: &AppHome) -> anyhow::Result<Settings>;
    /// 服务文件位置（dry-run 输出用）。
    fn service_location(&self) -> String;
    /// 以 `exe` 安装并启动登录自启服务，等待就绪。`restart_running`：二进制已更新，先结束仍在运行的旧实例
    /// （服务管理器不管理进程的平台需要）。
    fn install_service(&self, home: &AppHome, exe: &Path, restart_running: bool) -> impl Future<Output = anyhow::Result<ServiceOutcome>>;
    /// 卸载服务；返回是否曾安装。
    fn uninstall_service(&self, home: &AppHome) -> impl Future<Output = anyhow::Result<bool>>;
    /// 本配置目录运行中的实例（登记文件 + `/healthz` 确认）。
    fn running(&self, home: &AppHome) -> impl Future<Output = Option<EndpointRegistry>>;
    /// `doctor` 全部检查。
    fn doctor(&self, home: &AppHome) -> impl Future<Output = anyhow::Result<Report>>;
}

/// 真实的服务操作：复用 `service install / uninstall`、`/healthz` 探测与 `doctor`。
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHost;

impl HostOps for SystemHost {
    fn settings(&self, home: &AppHome) -> anyhow::Result<Settings> {
        crate::service_settings(home)
    }

    fn service_location(&self) -> String {
        service::location().unwrap_or_else(|e| format!("（无法确定：{e}）"))
    }

    async fn install_service(&self, home: &AppHome, exe: &Path, restart_running: bool) -> anyhow::Result<ServiceOutcome> {
        if restart_running
            && !service::MANAGED_BY_OS
            && let Some(reg) = crate::running_instance(home).await
        {
            service::kill_pid(reg.identity.pid)?;
        }
        let args = crate::cli::ServeArgs {
            hub: crate::cli::HubArgs {
                home: crate::cli::HomeArg { home: Some(home.dir.clone()) },
                ..Default::default()
            },
            ..Default::default()
        };
        let out = crate::install_service(&args, exe.to_path_buf()).await?;
        Ok(ServiceOutcome { location: out.location, messages: out.messages, registry: out.registry })
    }

    async fn uninstall_service(&self, home: &AppHome) -> anyhow::Result<bool> {
        crate::uninstall_service(home).await
    }

    async fn running(&self, home: &AppHome) -> Option<EndpointRegistry> {
        crate::running_instance(home).await
    }

    async fn doctor(&self, home: &AppHome) -> anyhow::Result<Report> {
        let s = crate::service_settings(home)?;
        Ok(doctor::run(home, &s).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_text_merges_streams() {
        let o = |out: &str, err: &str| CmdOutput { code: Some(1), stdout: out.into(), stderr: err.into() };
        assert_eq!(o("a", "").text(), "a");
        assert_eq!(o("", "b").text(), "b");
        assert_eq!(o("a", "b").text(), "a\nb");
        assert!(!o("a", "").success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn system_runner_runs_and_times_out() {
        let r = SystemRunner::default();
        let sh = r.which("sh").expect("sh");
        let out = r.run(&sh, &["-c".into(), "echo hi; exit 3".into()], Path::new("/")).await.unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout.trim(), "hi");
        let slow = SystemRunner { timeout: Duration::from_millis(100) };
        assert!(slow.run(&sh, &["-c".into(), "sleep 5".into()], Path::new("/")).await.is_err());
    }
}
