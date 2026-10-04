//! 集成测试：`app-mcp-host validate <清单> [--against <旧清单>] [--json]`（真实进程，临时文件；不涉及 Host 运行）。
//!
//! 退出码：无变化 / 只有可能破坏 0、有破坏性 3、清单无效 1（spec/manifest.md 第 6 节）。

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

use crate::serve::run_to_exit;

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");

/// 临时目录（Drop 时删除）。
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("app-mcp-validate-{tag}-{}-{n:x}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, v: &Value) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, serde_json::to_vec_pretty(v).unwrap()).unwrap();
        path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest(tools: Value, pages: Value) -> Value {
    json!({"manifestVersion": 1, "appId": "shop", "name": "Shop", "tools": tools, "pages": pages})
}

fn send_tool(input: Value, extra: Value) -> Value {
    let mut t = json!({"name": "mail.send", "description": "发送邮件", "inputSchema": input});
    for (k, v) in extra.as_object().unwrap() {
        t[k] = v.clone();
    }
    t
}

fn schema(required: &[&str]) -> Value {
    json!({"type": "object", "properties": {"to": {"type": "string"}, "body": {"type": "string"}}, "required": required})
}

/// 运行 `validate new --against old [extra...]`，返回 (退出码, stdout, stderr)。
fn validate(dir: &TempDir, old: &Value, new: &Value, extra: &[&str]) -> (i32, String, String) {
    let (old, new) = (dir.write("old.json", old), dir.write("new.json", new));
    let mut cmd = Command::new(BIN);
    cmd.arg("validate").arg(&new).arg("--against").arg(&old).args(extra);
    run_to_exit(cmd)
}

#[test]
fn unchanged_manifest_exits_zero() {
    let dir = TempDir::new("same");
    let m = manifest(json!([send_tool(schema(&["to"]), json!({}))]), json!([]));
    let (code, stdout, stderr) = validate(&dir, &m, &m, &[]);
    assert_eq!(code, 0, "stdout = {stdout}\nstderr = {stderr}");
    assert!(stdout.contains("无破坏性或可能破坏的变化"), "{stdout}");
    // 不带 --against：只校验
    let (code, stdout, _) = run_to_exit({
        let mut c = Command::new(BIN);
        c.arg("validate").arg(dir.0.join("new.json"));
        c
    });
    assert_eq!(code, 0);
    assert!(stdout.contains("清单有效"), "{stdout}");
}

#[test]
fn warnings_only_exit_zero_and_are_listed() {
    let dir = TempDir::new("warn");
    let old = manifest(json!([send_tool(schema(&["to"]), json!({"risk": "write"}))]), json!([]));
    let new = manifest(json!([send_tool(schema(&["to"]), json!({"risk": "payment"}))]), json!([]));
    let (code, stdout, stderr) = validate(&dir, &old, &new, &[]);
    assert_eq!(code, 0, "stdout = {stdout}\nstderr = {stderr}");
    assert!(stdout.contains("shop.mail.send") && stdout.contains("warning") && stdout.contains("/risk"), "{stdout}");
    assert!(!stdout.contains("breaking"), "{stdout}");
}

#[test]
fn breaking_change_exits_three() {
    let dir = TempDir::new("breaking");
    let old = manifest(json!([send_tool(schema(&["to"]), json!({}))]), json!([]));
    let new = manifest(json!([send_tool(schema(&["to", "body"]), json!({}))]), json!([]));
    let (code, stdout, stderr) = validate(&dir, &old, &new, &[]);
    assert_eq!(code, 3, "stdout = {stdout}\nstderr = {stderr}");
    assert!(stdout.contains("breaking") && stdout.contains("/inputSchema/properties/body"), "{stdout}");
    assert!(stdout.contains("deprecated"), "结论给出改名 + 弃用的建议：{stdout}");
}

#[test]
fn json_output_lists_tool_changes_with_page() {
    let dir = TempDir::new("json");
    let page = |tools: Value| json!([{"name": "orders", "tools": tools}]);
    // 工具名在整个清单内唯一（顶层与页面共用命名空间）
    let page_tool = |input: Value| json!({"name": "orders.reply", "description": "回复", "inputSchema": input});
    let deprecated = json!({"name": "old.tool", "description": "d", "inputSchema": {"type": "object"},
        "deprecated": {"message": "改用 mail.send"}});
    let old = manifest(
        json!([send_tool(schema(&[]), json!({})), deprecated]),
        page(json!([page_tool(schema(&[]))])),
    );
    let new = manifest(
        json!([send_tool(schema(&[]), json!({"surface": "view"}))]),
        page(json!([page_tool(schema(&["to"]))])),
    );
    let (code, stdout, stderr) = validate(&dir, &old, &new, &["--json"]);
    assert_eq!(code, 3, "stdout = {stdout}\nstderr = {stderr}");
    let v: Value = serde_json::from_str(&stdout).expect("stdout 为 JSON");
    assert_eq!(
        v,
        json!([
            {"tool": "mail.send", "removed": false, "changes": [
                {"level": "breaking", "path": "/surface", "message": v[0]["changes"][0]["message"]}
            ]},
            {"tool": "old.tool", "removed": true, "changes": []},
            {"page": "orders", "tool": "orders.reply", "removed": false, "changes": [
                {"level": "breaking", "path": "/inputSchema/properties/to", "message": v[2]["changes"][0]["message"]}
            ]}
        ])
    );
}

#[test]
fn invalid_manifests_are_errors() {
    let dir = TempDir::new("invalid");
    let good = manifest(json!([send_tool(schema(&[]), json!({}))]), json!([]));
    let bad = json!({"manifestVersion": 1, "appId": "Bad App", "name": "x"});
    // 旧清单无效
    let (code, _, stderr) = validate(&dir, &bad, &good, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("旧清单无效"), "{stderr}");
    // 新清单无效
    let (code, _, stderr) = validate(&dir, &good, &bad, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("清单无效"), "{stderr}");
    // 不同 App
    let mut other = good.clone();
    other["appId"] = json!("mail");
    let (code, _, stderr) = validate(&dir, &other, &good, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("appId 不同"), "{stderr}");
    // --json 需要 --against（clap 用法错误）
    let (code, _, _) = run_to_exit({
        let mut c = Command::new(BIN);
        c.arg("validate").arg(dir.0.join("new.json")).arg("--json");
        c
    });
    assert_eq!(code, 2);
}
