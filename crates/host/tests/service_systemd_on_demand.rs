//! 真实安装 systemd `--user` 按需启动服务（Linux，spec/protocol.md 1.9）。会在当前用户的 systemd 中临时安装
//! `app-mcp-host.socket` + `app-mcp-host.service`，因此默认忽略，需手动运行：
//!
//! ```bash
//! cargo test -p app-mcp-host --test service_systemd_on_demand -- --ignored
//! ```
//!
//! 临时配置目录、按进程号选的高位端口（不碰 7717）、临时目录中的 IPC 套接字、空闲 1.5 秒退出；测试结束（含失败）一定卸载。systemd 用户实例不可用、或本机已装有 app-mcp-host 单元（不覆盖用户自己的服务）时跳过。
//!
//! 流程：install --on-demand → 服务未运行、套接字在监听 → 连 IPC 套接字即启动 Host（/healthz 回 pid）→ 登记文件给出 systemd
//! 实际绑定的 TCP 端口 → 空闲退出（服务 inactive、套接字仍 active）→ 连 TCP 再次启动（新 pid、同一端口）→ uninstall。
#![cfg(all(unix, not(target_os = "macos")))]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use app_mcp_protocol::registry::EndpointRegistry;

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");
const IDLE_MS: u64 = 1500;
const T: Duration = Duration::from_secs(20);

fn host(home: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(BIN).args(args).arg("--home").arg(home).env_remove("APP_MCP_HOME").output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

fn systemctl(args: &[&str]) -> String {
    let out = Command::new("systemctl").arg("--user").args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// 测试结束一定卸载并删除临时目录。
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = host(&self.0, &["service", "uninstall"]);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unit_dir() -> PathBuf {
    dirs::config_dir().unwrap().join("systemd/user")
}

/// `GET /healthz`（`Connection: close`），返回响应中的 pid。
fn healthz_pid(mut s: impl Read + Write) -> u32 {
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
    let mut text = String::new();
    s.read_to_string(&mut text).unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}：{text}"));
    u32::try_from(v["pid"].as_u64().unwrap()).unwrap()
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + T;
    while !f() {
        assert!(Instant::now() < deadline, "等待超时：{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "会在当前用户的 systemd 中临时安装按需启动服务；手动运行 -- --ignored"]
fn systemd_on_demand_install_activate_idle_exit_reactivate_uninstall() {
    let available = Command::new("systemctl").args(["--user", "show-environment"]).output().is_ok_and(|o| o.status.success());
    if !available {
        eprintln!("跳过：systemd 用户实例不可用");
        return;
    }
    let units = [unit_dir().join("app-mcp-host.service"), unit_dir().join("app-mcp-host.socket")];
    if units.iter().any(|p| p.exists()) {
        eprintln!("跳过：本机已安装 app-mcp-host 单元，不覆盖");
        return;
    }
    let home = std::env::temp_dir().join(format!("app-mcp-sd-od-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let _cleanup = Cleanup(home.clone());
    let sock = home.join("ipc").join("hub.sock");
    let ipc = format!("unix:{}", sock.display());
    let idle = IDLE_MS.to_string();
    // @why 按需启动不接受端口 0（systemd 不绑定 `…:0`），不能用"绑定端口 0 再从登记文件读"的做法；按进程号选一个高位端口，
    // `service install` 的端口预检保证安装时空闲，随后由 systemd 立即绑定（手动运行的测试，竞争窗口可接受）。
    let listen = format!("127.0.0.1:{}", 20_000 + std::process::id() % 20_000);
    let (code, out) = host(
        &home,
        &["service", "install", "--on-demand", "--listen", &listen, "--ipc-endpoint", &ipc, "--idle-exit-ms", &idle],
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("按需启动服务"), "{out}");
    let socket_unit = std::fs::read_to_string(&units[1]).unwrap();
    assert!(socket_unit.contains(&format!("ListenStream={listen}\n")) && socket_unit.contains(&format!("ListenStream={}\n", sock.display())), "{socket_unit}");
    assert!(!std::fs::read_to_string(&units[0]).unwrap().contains("WantedBy"), "按需启动的服务不随登录启动");
    assert_eq!(systemctl(&["is-active", "app-mcp-host.socket"]), "active");
    assert_eq!(systemctl(&["is-enabled", "app-mcp-host.socket"]), "enabled");

    // 安装时的就绪检查已连过 TCP（连接即启动）：等它空闲退出，再从 IPC 启动
    wait_until("安装后的实例空闲退出", || systemctl(&["is-active", "app-mcp-host.service"]) == "inactive");
    // 连 IPC 套接字即启动
    let first = healthz_pid(std::os::unix::net::UnixStream::connect(&sock).unwrap());
    assert_eq!(systemctl(&["is-active", "app-mcp-host.service"]), "active");
    let registry = home.join("run").join("endpoints.json");
    let reg = EndpointRegistry::read(&registry).unwrap().unwrap();
    assert_eq!(reg.identity.pid, first);
    assert_eq!(reg.ipc_endpoint.as_deref(), Some(ipc.as_str()));
    let addr = reg.listen.clone().unwrap();
    assert_eq!(addr, listen, "登记文件应给出 systemd 绑定的地址");

    // 空闲退出：服务停止，套接字仍由 systemd 监听
    wait_until("空闲退出", || systemctl(&["is-active", "app-mcp-host.service"]) == "inactive");
    assert_eq!(systemctl(&["is-active", "app-mcp-host.socket"]), "active");
    assert!(!registry.exists(), "空闲退出应删除登记文件");
    let log = std::fs::read_to_string(home.join("logs").join("app-mcp-host.log")).unwrap();
    assert!(log.contains("按需启动（systemd 套接字激活）") && log.contains("退出（下一个连接"), "{log}");

    // 连 TCP 再次启动：新进程、同一端口
    let second = healthz_pid(std::net::TcpStream::connect(&addr).unwrap());
    assert_ne!(second, first);
    let (code, out) = host(&home, &["service", "status"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("按需启动"), "{out}");

    let (code, out) = host(&home, &["service", "uninstall"]);
    assert_eq!(code, 0, "{out}");
    assert!(units.iter().all(|p| !p.exists()));
    assert_ne!(systemctl(&["is-active", "app-mcp-host.socket"]), "active");
    assert!(std::net::TcpStream::connect(&addr).is_err(), "卸载后不应再监听");
}
