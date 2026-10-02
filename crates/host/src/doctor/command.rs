//! doctor 运行外部命令（`adb`）的唯一入口：有超时、不读标准输入、超时或丢弃时结束子进程。

use std::path::Path;
use std::time::Duration;

/// 外部命令的输出。
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 外部命令没能给出输出的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolFailure {
    /// 无法启动（程序不存在、无权限等）。
    Spawn(String),
    /// 超时（子进程已被结束）。
    Timeout(Duration),
}

impl std::fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolFailure::Spawn(e) => write!(f, "无法运行：{e}"),
            ToolFailure::Timeout(t) => write!(f, "{} 秒内没有返回", t.as_secs_f32()),
        }
    }
}

/// 运行 `program args…`，最长 `timeout`。
///
/// @side-effect 启动子进程；超时后子进程随 `Child` 丢弃被结束（`kill_on_drop`）。
/// @compat Windows 上不弹出控制台窗口（`CREATE_NO_WINDOW`）。
pub async fn run_tool(program: &Path, args: &[&str], timeout: Duration) -> Result<ToolOutput, ToolFailure> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(o)) => Ok(ToolOutput {
            success: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(ToolFailure::Spawn(e.to_string())),
        Err(_) => Err(ToolFailure::Timeout(timeout)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_program_and_timeout_are_reported() {
        let missing = run_tool(Path::new("/nonexistent/app-mcp-no-such-tool"), &[], Duration::from_secs(1)).await;
        assert!(matches!(missing, Err(ToolFailure::Spawn(_))), "{missing:?}");

        let started = std::time::Instant::now();
        let slow = run_tool(Path::new("/bin/sh"), &["-c", "sleep 5"], Duration::from_millis(200)).await;
        assert_eq!(slow.unwrap_err(), ToolFailure::Timeout(Duration::from_millis(200)));
        assert!(started.elapsed() < Duration::from_secs(3), "超时后不等待子进程结束");

        let ok = run_tool(Path::new("/bin/sh"), &["-c", "echo hi; echo err >&2; exit 3"], Duration::from_secs(5)).await.unwrap();
        assert!(!ok.success);
        assert_eq!((ok.stdout.trim(), ok.stderr.trim()), ("hi", "err"));
    }
}
