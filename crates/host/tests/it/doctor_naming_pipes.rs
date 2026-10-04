//! 集成测试（Windows）：`app-mcp-host doctor` 的 `naming.pipes`（spec/naming.md 第 11 节），真实进程、真实命名管道。
//!
//! 登记目录在临时 `LOCALAPPDATA` 下；appId 带随机后缀，不与机器上真实 App 的管道相撞。Host 不启动（临时配置目录）；
//! `PATH` 只含 System32（没有 adb）。
#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use app_mcp_protocol::naming::{Address, pipe as names};
use serde_json::{Value, json};
use tokio::net::windows::named_pipe::ServerOptions;

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("app-mcp-doctor-pipes-{:032x}", rand::random::<u128>()));
        for d in ["home", "local/app-mcp/apps"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let ipc = format!(r"pipe:\\.\pipe\app-mcp-doctor-test-{:016x}", rand::random::<u64>());
        let config = json!({ "listen": "127.0.0.1:0", "ipcEndpoint": ipc, "log": { "file": false } });
        std::fs::write(dir.join("home/config.json"), config.to_string()).unwrap();
        Self(dir)
    }
    fn register(&self, app_id: &str, executable: &str, target: &str) {
        let reg = json!({
            "registrationVersion": 1, "appId": app_id, "name": app_id, "source": "manual",
            "executable": executable, "activation": { "kind": "exec", "target": target },
        });
        std::fs::write(self.0.join("local/app-mcp/apps").join(format!("{app_id}.json")), reg.to_string()).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn doctor_json(s: &Scratch) -> Value {
    let system32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join("System32");
    let out = Command::new(BIN)
        .args(["doctor", "--json", "--home"])
        .arg(s.0.join("home"))
        .env_remove("APP_MCP_HOME")
        .env("LOCALAPPDATA", s.0.join("local"))
        .env("PATH", system32)
        .stdin(Stdio::null())
        .output()
        .expect("doctor");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor --json 输出不是 JSON（{e}）：{}", String::from_utf8_lossy(&out.stdout)))
}

fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("缺少检查 {id}：{report}"))
}

fn pipe_name(sid: &str, app_id: &str) -> String {
    names::pipe_name(sid, &Address::new(app_id, None).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn pipes_check_lists_without_opening() {
    let s = Scratch::new();
    let sid = app_mcp_protocol::endpoint::win::current_user_sid().unwrap();
    let tag = format!("{:08x}", rand::random::<u32>());
    let (running, idle, stray) = (format!("doc-run-{tag}"), format!("doc-idle-{tag}"), format!("doc-stray-{tag}"));
    let missing = s.0.join("gone.exe").to_string_lossy().into_owned();
    s.register(&running, BIN, "");
    s.register(&idle, BIN, &missing);
    // 运行中的 App（已登记）与没有登记文件的管道；doctor 只能列名字，不能打开它们。
    let running_pipe = ServerOptions::new().first_pipe_instance(true).create(pipe_name(&sid, &running)).unwrap();
    let stray_pipe = ServerOptions::new().first_pipe_instance(true).create(pipe_name(&sid, &stray)).unwrap();

    let report = doctor_json(&s);
    let c = check(&report, "naming.pipes");
    let summary = c["summary"].as_str().unwrap();
    assert_eq!(c["status"], "error", "{c}");
    assert!(summary.contains(&format!("{running}：运行中（管道存在）")), "{c}");
    assert!(summary.contains(&format!("{idle}：未运行，激活程序 {missing} 不存在")), "{c}");
    assert!(summary.contains("没有对应的登记文件") && summary.contains(&stray), "{c}");
    let apps = c["details"]["apps"].as_array().unwrap();
    let entry = apps.iter().find(|a| a["appId"] == running.as_str()).unwrap();
    assert_eq!((entry["running"].as_bool(), entry["pipe"].as_str()), (Some(true), Some(pipe_name(&sid, &running).as_str())));
    assert_eq!(c["details"]["sid"], sid.as_str());

    // 只读：doctor 没有连接任何管道（有客户端连过时 connect 立即完成）。
    for (name, pipe) in [("running", &running_pipe), ("stray", &stray_pipe)] {
        assert!(tokio::time::timeout(Duration::from_millis(300), pipe.connect()).await.is_err(), "doctor 打开了 {name} 管道");
    }
}

#[test]
fn pipes_check_is_quiet_without_registrations() {
    let s = Scratch::new();
    let report = doctor_json(&s);
    let c = check(&report, "naming.pipes");
    // 机器上可能有其他（真实）App 的管道：没有登记时它们报"注意"，否则为信息。
    assert!(c["status"] == "info" || c["status"] == "warn", "{c}");
    assert_eq!(c["details"]["apps"].as_array().map(Vec::len), Some(0), "{c}");
}
