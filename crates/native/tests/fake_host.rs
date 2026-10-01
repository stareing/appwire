//! 端到端：NativeClient 对接 `examples/fake_host`（各语言绑定用的同一个工具）。

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;

use app_mcp_native::{
    CallHandle, CallResult, ContentAnnotations, ErrorKind, NativeClient, NativeConfig, ReadHandle, ResourceReader,
    ResourceSpec, ResultStatus, ToolAnnotations, ToolHandler, ToolOptions, ToolSpec,
};
use serde_json::{Value, json};

#[path = "../src/test_support.rs"]
mod test_support;

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let sum = args["a"].as_i64().unwrap_or(0) + args["b"].as_i64().unwrap_or(0);
        // 从另一个线程完成
        std::thread::spawn(move || {
            call.report_progress(1.0, Some(2.0), Some("相加")).unwrap();
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

/// 新构建的 fake_host（`cargo test` 不刷新 examples/fake_host，见 src/test_support.rs）。
fn fake_host() -> std::path::PathBuf {
    test_support::fake_host_path().unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn native_client_against_fake_host() {
    let bin = fake_host();
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
    assert_eq!(out.len(), 6, "{out:#?}");
    assert_eq!(
        out[0],
        json!({ "type": "tools", "tools": ["math.add", "nope.fail"], "resources": ["app.state"] })
    );
    assert_eq!(out[1]["type"], "progress");
    assert_eq!((&out[1]["progress"], &out[1]["total"], &out[1]["message"]), (&json!(1.0), &json!(2.0), &json!("相加")));
    assert!(out[1]["callId"].is_string());
    assert_eq!(
        out[2],
        json!({ "type": "invoke", "name": "math.add", "result": { "data": { "sum": 5 } } })
    );
    assert_eq!(out[3]["result"]["contents"], json!({ "count": 3 }));
    assert_eq!(out[4]["error"]["data"]["kind"], "USER_REJECTED");
    assert_eq!(out[5]["error"]["data"]["kind"], "TOOL_NOT_FOUND");
}

struct Submit;
impl ToolHandler for Submit {
    fn invoke(&self, call: CallHandle) {
        call.complete_with(CallResult {
            data_json: Some(r#"{"orderId":"o1"}"#.into()),
            status: ResultStatus::Pending,
            state_resource: Some("order.state".into()),
            summary: Some("已提交，等待用户在 App 内付款".into()),
            annotations: Some(ContentAnnotations { priority: Some(0.5), ..ContentAnnotations::default() }),
            ..CallResult::default()
        })
        .unwrap();
    }
}

/// 工具选项（MCP 注解、输出 schema）与完整结果（状态、摘要、内容标注）原样到达 Host。
#[test]
fn tool_options_and_call_result_reach_host() {
    let bin = fake_host();
    let mut child = Command::new(bin)
        .args(["--tool-info", "--invoke", "order.submit", "--timeout-ms", "8000"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let addr = lines.next().unwrap().unwrap().strip_prefix("LISTENING ").expect("LISTENING 行").to_owned();
    let mut config = NativeConfig::new("fake-test", "Fake");
    config.host_url = format!("ws://{addr}");
    let client = NativeClient::new(config, None).unwrap();
    let options = ToolOptions {
        annotations: Some(ToolAnnotations { idempotent_hint: Some(false), open_world_hint: Some(true), ..Default::default() }),
        output_schema_json: Some(r#"{"type":"object","properties":{"orderId":{"type":"string"}}}"#.into()),
    };
    let tool = client.register_tool_with(ToolSpec::new("order.submit", "下单"), options, Arc::new(Submit)).unwrap();
    // 非法 outputSchema：注册失败，不影响已注册的工具
    let bad = ToolOptions { output_schema_json: Some("{".into()), ..ToolOptions::default() };
    assert!(tool.update_with(ToolSpec::new("order.submit", "下单"), bad).is_err());
    client.start();

    let out: Vec<Value> = lines.map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        out[0]["toolInfo"]["order.submit"],
        json!({
            "risk": "write",
            "annotations": { "idempotentHint": false, "openWorldHint": true },
            "outputSchema": { "type": "object", "properties": { "orderId": { "type": "string" } } }
        })
    );
    assert_eq!(
        out[1]["result"],
        json!({
            "data": { "orderId": "o1" }, "status": "pending", "stateResource": "order.state",
            "summary": "已提交，等待用户在 App 内付款", "annotations": { "priority": 0.5 }
        })
    );
}
