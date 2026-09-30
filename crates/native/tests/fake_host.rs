//! 端到端：NativeClient 对接 `examples/fake_host`（各语言绑定用的同一个工具）。

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;

use app_mcp_native::{
    CallHandle, ErrorKind, NativeClient, NativeConfig, ReadHandle, ResourceReader, ResourceSpec,
    ToolHandler, ToolSpec,
};
use serde_json::{Value, json};

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let sum = args["a"].as_i64().unwrap_or(0) + args["b"].as_i64().unwrap_or(0);
        // 从另一个线程完成
        std::thread::spawn(move || {
            call.complete(Some(&json!({ "sum": sum }).to_string()), vec![])
                .unwrap()
        });
    }
}

struct Fails;
impl ToolHandler for Fails {
    fn invoke(&self, call: CallHandle) {
        call.fail(ErrorKind::UserRejected, "不行").unwrap();
    }
}

struct State;
impl ResourceReader for State {
    fn read(&self, read: ReadHandle) {
        read.complete(r#"{"count":3}"#).unwrap();
    }
}

fn fake_host_path() -> Option<std::path::PathBuf> {
    // target/<profile>/deps/<test> → target/<profile>/examples/fake_host
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let path = dir
        .join("examples")
        .join(format!("fake_host{}", std::env::consts::EXE_SUFFIX));
    path.exists().then_some(path)
}

#[test]
fn native_client_against_fake_host() {
    let Some(bin) = fake_host_path() else {
        eprintln!("未找到 fake_host 可执行文件，跳过（先运行 cargo build --example fake_host）");
        return;
    };
    let mut child = Command::new(bin)
        .args(["--invoke", "math.add", "--args", r#"{"a":2,"b":3}"#])
        .args([
            "--read",
            "app.state",
            "--invoke",
            "nope.fail",
            "--invoke",
            "missing",
        ])
        .args(["--timeout-ms", "8000"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first = lines.next().unwrap().unwrap();
    let addr = first
        .strip_prefix("LISTENING ")
        .expect("LISTENING 行")
        .to_owned();

    let mut config = NativeConfig::new("fake-test", "Fake");
    config.host_url = format!("ws://{addr}");
    let client = NativeClient::new(config, None).unwrap();
    client
        .register_tool(ToolSpec::new("math.add", "加法"), Arc::new(Add))
        .unwrap();
    client
        .register_tool(ToolSpec::new("nope.fail", "失败"), Arc::new(Fails))
        .unwrap();
    client
        .register_resource(
            ResourceSpec {
                name: "app.state".into(),
                description: "状态".into(),
                mime_type: None,
            },
            Arc::new(State),
        )
        .unwrap();
    client.start();

    let out: Vec<Value> = lines
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    let status = child.wait().unwrap();
    assert!(status.success(), "fake_host 退出码 {status:?}");
    assert_eq!(out.len(), 5, "{out:#?}");
    assert_eq!(
        out[0],
        json!({ "type": "tools", "tools": ["math.add", "nope.fail"], "resources": ["app.state"] })
    );
    assert_eq!(
        out[1],
        json!({ "type": "invoke", "name": "math.add", "result": { "data": { "sum": 5 } } })
    );
    assert_eq!(out[2]["result"]["contents"], json!({ "count": 3 }));
    assert_eq!(out[3]["error"]["data"]["kind"], "USER_REJECTED");
    assert_eq!(out[4]["error"]["data"]["kind"], "TOOL_NOT_FOUND");
}
