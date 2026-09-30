//! 真实安装 systemd `--user` 服务（Linux）。会在当前用户的 systemd 中临时安装 `app-mcp-host.service`，
//! 因此默认忽略，需手动运行：
//!
//! ```bash
//! cargo test -p app-mcp-host --test service_systemd -- --ignored
//! ```
//!
//! 使用临时配置目录与随机端口；测试结束（含失败）一定卸载。systemd 用户实例不可用、
//! 或本机已安装了 app-mcp-host.service（不覆盖用户自己的服务）时跳过。
#![cfg(all(unix, not(target_os = "macos")))]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn host(home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(BIN)
        .args(args)
        .arg("--home")
        .arg(home)
        .env_remove("APP_MCP_HOME")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), text)
}

/// 测试结束一定卸载并删除临时目录。
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = host(&self.0, &["service", "uninstall"]);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unit_path() -> PathBuf {
    dirs::config_dir()
        .unwrap()
        .join("systemd/user/app-mcp-host.service")
}

#[test]
#[ignore = "会在当前用户的 systemd 中临时安装服务；手动运行 -- --ignored"]
fn systemd_install_status_stop_start_uninstall() {
    let available = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !available {
        eprintln!("跳过：systemd 用户实例不可用");
        return;
    }
    if unit_path().exists() {
        eprintln!("跳过：本机已安装 {}，不覆盖", unit_path().display());
        return;
    }
    let home = std::env::temp_dir().join(format!("app-mcp-systemd-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let _cleanup = Cleanup(home.clone());
    let ws = format!("127.0.0.1:{}", free_port());
    let http = format!("127.0.0.1:{}", free_port());

    let (code, out) = host(
        &home,
        &["service", "install", "--ws-addr", &ws, "--http", &http],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("已在运行"), "{out}");
    let unit = std::fs::read_to_string(unit_path()).unwrap();
    assert!(
        unit.contains(&format!(
            "ExecStart=\"{}\" \"serve\" \"--home\"",
            std::fs::canonicalize(BIN).unwrap().display()
        )),
        "{unit}"
    );
    let config = std::fs::read_to_string(home.join("config.json")).unwrap();
    assert!(config.contains(&http), "{config}");
    assert!(home.join("token").exists());

    let (code, out) = host(&home, &["service", "status"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("已安装") && out.contains("enabled") && out.contains("active"),
        "{out}"
    );

    let (code, out) = host(&home, &["service", "stop"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = host(&home, &["service", "status"]);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("未运行"), "{out}");

    let (code, out) = host(&home, &["service", "start"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("已在运行"), "{out}");

    let (code, out) = host(&home, &["service", "uninstall"]);
    assert_eq!(code, 0, "{out}");
    assert!(!unit_path().exists());
    let (code, out) = host(&home, &["service", "status"]);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("未安装"), "{out}");
}
