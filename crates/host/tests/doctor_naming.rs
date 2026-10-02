//! 集成测试：`app-mcp-host doctor` 的名字服务检查组（spec/naming.md 第 11 节），真实进程、临时目录。
//!
//! 不使用用户的会话总线与 adb：`DBUS_SESSION_BUS_ADDRESS` 指向不存在的套接字，`PATH` 中没有 adb，
//! 登记目录与激活目录在临时 `XDG_DATA_HOME` / `XDG_DATA_DIRS` 下。Host 不启动（临时配置目录）。
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use app_mcp_protocol::naming::dbus as names;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("app-mcp-doctor-naming-{:032x}", rand::random::<u128>()));
        for d in ["home", "data/app-mcp/apps", "data/dbus-1/services", "sys", "bin"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let config = json!({
            "listen": "127.0.0.1:0",
            "ipcEndpoint": format!("unix:{}", dir.join("home/run/hub.sock").display()),
            "log": { "file": false },
        });
        std::fs::write(dir.join("home/config.json"), config.to_string()).unwrap();
        Self(dir)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_private(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn doctor_json(s: &Scratch) -> (Value, Duration) {
    let started = Instant::now();
    let out = Command::new(BIN)
        .args(["doctor", "--json", "--home"])
        .arg(s.0.join("home"))
        .env_remove("APP_MCP_HOME")
        .env("XDG_DATA_HOME", s.0.join("data"))
        .env("XDG_DATA_DIRS", s.0.join("sys"))
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", s.0.join("no-bus").display()))
        .env("PATH", s.0.join("bin"))
        .stdin(Stdio::null())
        .output()
        .expect("doctor");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("doctor --json 输出不是 JSON（{e}）：{}", String::from_utf8_lossy(&out.stdout))
    });
    (report, started.elapsed())
}

fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("缺少检查 {id}：{report}"))
}

#[test]
fn naming_checks_report_broken_registration_and_missing_bus() {
    let s = Scratch::new();
    // 登记了一个程序已被删除的 App：登记文件与激活文件都指向不存在的程序。
    let gone = "/nonexistent/app-mcp-doctor/shop";
    write_private(
        &s.0.join("data/app-mcp/apps/my-shop.json"),
        &json!({
            "registrationVersion": 1, "appId": "my-shop", "name": "商城", "source": "manual",
            "executable": gone, "activation": { "kind": "dbus", "target": "dev.appmcp.App.my_shop" },
        })
        .to_string(),
    );
    write_private(&s.0.join("data/dbus-1/services").join(names::service_file_name("my-shop")), &names::service_file("my-shop", gone));

    let (report, took) = doctor_json(&s);
    assert!(took < Duration::from_secs(20), "总线缺失时不挂起：{took:?}");

    let reg = check(&report, "naming.registrations");
    assert_eq!(reg["status"], "error", "{reg}");
    assert!(reg["summary"].as_str().unwrap().contains(&format!("程序 {gone} 不存在")), "{reg}");
    assert!(reg["hint"].as_str().unwrap().contains("app-mcp-host app install"), "{reg}");

    let dbus = check(&report, "naming.dbus");
    assert_eq!(dbus["status"], "error", "激活文件指向的程序不存在：{dbus}");
    assert_eq!(dbus["code"], "NAME_NOT_FOUND", "{dbus}");
    assert_eq!(dbus["details"]["bus"]["reachable"], false, "{dbus}");
    assert!(dbus["summary"].as_str().unwrap().contains("会话总线不可达"), "{dbus}");
    assert_eq!(dbus["details"]["serviceFiles"].as_array().map(Vec::len), Some(1), "{dbus}");

    let android = check(&report, "naming.android");
    assert_eq!((android["status"].as_str(), android["summary"].as_str()), (Some("skip"), Some("PATH 中没有 adb")));
}

#[test]
fn naming_checks_are_quiet_without_registrations() {
    let s = Scratch::new();
    let (report, _) = doctor_json(&s);
    for id in ["naming.registrations", "naming.dbus"] {
        assert_eq!(check(&report, id)["status"], "info", "{id}：{report}");
    }
}
