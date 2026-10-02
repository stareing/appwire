//! 测试支持（feature `test-support`，Linux）：临时私有 D-Bus 会话总线（spec/naming.md 4.1 的测试环境）。
//!
//! 测试不得占用用户的会话总线：每个 [`PrivateBus`] 在临时目录中启动一个 `dbus-daemon --session`，
//! 激活目录取自临时的 `XDG_DATA_HOME`（`<dir>/data/dbus-1/services`，与 `app-mcp-host app install` 写入的位置相同），
//! 被丢弃时结束进程并删除目录。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// 一个私有会话总线。
#[derive(Debug)]
pub struct PrivateBus {
    child: Child,
    dir: PathBuf,
    address: String,
}

/// 在 `PATH` 中查找 `dbus-daemon`；没有时为 `None`（调用方据此跳过测试）。
pub fn find_dbus_daemon() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("dbus-daemon")).find(|p| p.is_file())
}

impl PrivateBus {
    /// 启动；本机没有 `dbus-daemon` 时返回 `Ok(None)`。
    pub fn start() -> Result<Option<Self>, String> {
        Self::start_with_env(&[])
    }

    /// 同 [`PrivateBus::start`]，另给 `dbus-daemon` 进程（及其激活的服务进程）设置环境变量。
    pub fn start_with_env(env: &[(&str, &std::ffi::OsStr)]) -> Result<Option<Self>, String> {
        let Some(daemon) = find_dbus_daemon() else { return Ok(None) };
        let dir = std::env::temp_dir().join(format!("app-mcp-bus-{:032x}", rand::random::<u128>()));
        let services = dir.join("data").join("dbus-1").join("services");
        for d in [&services, &dir.join("sys"), &dir.join("run")] {
            std::fs::create_dir_all(d).map_err(|e| format!("无法创建 {}：{e}", d.display()))?;
        }
        let socket = dir.join("bus");
        let config = dir.join("session.conf");
        std::fs::write(&config, config_text(&socket)).map_err(|e| format!("无法写 {}：{e}", config.display()))?;
        let mut cmd = Command::new(&daemon);
        cmd.envs(env.iter().copied());
        let mut child = cmd
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("XDG_DATA_DIRS", dir.join("sys"))
            .env("XDG_RUNTIME_DIR", dir.join("run"))
            // @why 被激活的进程继承本进程环境：去掉用户会话总线地址，避免测试 App 误连用户总线。
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DBUS_STARTER_ADDRESS")
            .env_remove("DBUS_STARTER_BUS_TYPE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("无法启动 {}：{e}", daemon.display()))?;
        let mut line = String::new();
        let read = child
            .stdout
            .take()
            .map(|out| BufReader::new(out).read_line(&mut line))
            .ok_or_else(|| "dbus-daemon 没有标准输出".to_owned());
        let address = line.trim().to_owned();
        if !matches!(read, Ok(Ok(n)) if n > 0) || address.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("dbus-daemon 未打印地址：{read:?}"));
        }
        Ok(Some(Self { child, dir, address }))
    }

    /// 总线地址（`unix:path=…`）。
    pub fn address(&self) -> &str {
        &self.address
    }

    /// 临时 `XDG_DATA_HOME`（激活文件写在其下 `dbus-1/services`）。
    pub fn data_home(&self) -> PathBuf {
        self.dir.join("data")
    }

    /// 会话服务激活目录。
    pub fn services_dir(&self) -> PathBuf {
        self.data_home().join("dbus-1").join("services")
    }

    /// 临时目录（测试可放其他文件）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config_text(socket: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <standard_session_servicedirs/>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        socket.display()
    )
}
