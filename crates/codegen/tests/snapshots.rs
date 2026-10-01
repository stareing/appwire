//! 快照测试：用 tests/fixtures/shop.json 为每个 target 生成输出，与 tests/snapshots/<target>/ 比较。
//!
//! 设置环境变量 `UPDATE_SNAPSHOTS=1` 时改为写入（更新）期望文件。

use std::path::{Path, PathBuf};

use app_mcp_codegen::{Options, Output, Target, generate_from_str};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn generate(target: Target, options: &Options) -> Output {
    let text = std::fs::read_to_string(crate_dir().join("tests/fixtures/shop.json"))
        .expect("读取示例清单");
    let (output, manifest_warnings) =
        generate_from_str(&text, target, options).expect("示例清单应当合法");
    assert!(manifest_warnings.is_empty(), "{manifest_warnings:?}");
    output
}

/// 列出目录下的全部文件（相对路径，排序）。
fn list_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn check_target(target: Target) {
    let output = generate(target, &Options::default());
    let dir = crate_dir().join("tests/snapshots").join(target.name());
    let mut actual: Vec<(PathBuf, String)> = output
        .files
        .iter()
        .map(|f| (f.path.clone(), f.contents.clone()))
        .collect();
    // 警告也纳入快照
    let warnings: String = output.warnings.iter().map(|w| format!("{w}\n")).collect();
    actual.push((PathBuf::from("warnings.txt"), warnings));
    actual.sort();

    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        let _ = std::fs::remove_dir_all(&dir);
        for (path, contents) in &actual {
            let full = dir.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("创建快照目录");
            }
            std::fs::write(&full, contents).expect("写入快照");
        }
        return;
    }

    let expected_files = list_files(&dir);
    let actual_files: Vec<PathBuf> = actual.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(
        actual_files, expected_files,
        "{target} 生成的文件列表与快照不一致（UPDATE_SNAPSHOTS=1 可更新）"
    );
    for (path, contents) in &actual {
        let expected = std::fs::read_to_string(dir.join(path)).expect("读取快照");
        if &expected != contents {
            let line = expected
                .lines()
                .zip(contents.lines())
                .position(|(a, b)| a != b)
                .map(|i| i + 1)
                .unwrap_or_else(|| expected.lines().count().min(contents.lines().count()) + 1);
            panic!(
                "{target} 的 {} 与快照不一致（第 {line} 行起）；UPDATE_SNAPSHOTS=1 可更新\n--- 期望\n{}\n--- 实际\n{}",
                path.display(),
                expected.lines().nth(line - 1).unwrap_or(""),
                contents.lines().nth(line - 1).unwrap_or("")
            );
        }
    }
}

#[test]
fn snapshot_typescript() {
    check_target(Target::TypeScript);
}

#[test]
fn snapshot_csharp() {
    check_target(Target::CSharp);
}

#[test]
fn snapshot_swift() {
    check_target(Target::Swift);
}

#[test]
fn snapshot_kotlin() {
    check_target(Target::Kotlin);
}

#[test]
fn snapshot_python() {
    check_target(Target::Python);
}

#[test]
fn snapshot_dart() {
    check_target(Target::Dart);
}

#[test]
fn snapshot_swift_app_intents() {
    check_target(Target::SwiftAppIntents);
}

#[test]
fn snapshot_kotlin_appfunctions() {
    check_target(Target::KotlinAppFunctions);
}

#[test]
fn snapshot_windows_app_actions() {
    check_target(Target::WindowsAppActions);
}

#[test]
fn snapshot_harmony_insight_intents() {
    check_target(Target::HarmonyInsightIntents);
}

#[test]
fn every_target_has_a_snapshot_test() {
    // 新增 target 时提醒补快照测试
    assert_eq!(Target::ALL.len(), 10);
}

#[test]
fn native_targets_share_typed_files() {
    let opts = Options::default();
    let swift = generate(Target::Swift, &opts);
    let intents = generate(Target::SwiftAppIntents, &opts);
    assert_eq!(
        swift.files[0], intents.files[0],
        "App Intents 应原样包含 Swift 类型文件"
    );
    let kotlin = generate(Target::Kotlin, &opts);
    let functions = generate(Target::KotlinAppFunctions, &opts);
    assert_eq!(kotlin.files[0], functions.files[0]);
    let csharp = generate(Target::CSharp, &opts);
    let windows = generate(Target::WindowsAppActions, &opts);
    assert_eq!(csharp.files[0], windows.files[0]);
}

#[test]
fn cli_writes_files() {
    let out = std::env::temp_dir().join(format!("app-mcp-codegen-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_app-mcp-codegen"))
        .arg("--manifest")
        .arg(crate_dir().join("tests/fixtures/shop.json"))
        .args([
            "--target",
            "kotlin",
            "--package",
            "com.example.shop",
            "--module",
            "MyShop",
        ])
        .arg("--out")
        .arg(&out)
        .output()
        .expect("运行 CLI");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let text = std::fs::read_to_string(out.join("MyShopTools.kt")).expect("生成的文件");
    assert!(text.contains("package com.example.shop"));
    assert!(text.contains("interface MyShopToolHandlers"));
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(stderr.contains("警告"), "降级应输出警告：{stderr}");
    let _ = std::fs::remove_dir_all(&out);

    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_app-mcp-codegen"))
        .arg("--manifest")
        .arg(crate_dir().join("tests/fixtures/shop.json"))
        .args(["--target", "cobol", "--out"])
        .arg(&out)
        .output()
        .expect("运行 CLI");
    assert!(!bad.status.success());
}

/// 原生意图 target 只生成 `surface: app` 的工具，`view` 工具记警告跳过；类型化接口不受影响；页面工具不参与生成。
#[test]
fn native_targets_skip_view_tools() {
    let text = serde_json::json!({
        "manifestVersion": 1, "appId": "shop", "name": "示例商城",
        "tools": [
            { "name": "orders.search", "description": "搜索订单", "inputSchema": { "type": "object" } },
            { "name": "cart.highlight", "description": "高亮购物车条目", "inputSchema": { "type": "object" }, "surface": "view" }
        ],
        "pages": [{ "name": "cart", "tools": [
            { "name": "cart.checkout", "description": "结算", "inputSchema": { "type": "object" }, "surface": "view" }
        ] }]
    })
    .to_string();
    let native = [
        Target::SwiftAppIntents,
        Target::KotlinAppFunctions,
        Target::WindowsAppActions,
        Target::HarmonyInsightIntents,
    ];
    for target in Target::ALL {
        let (output, _) = generate_from_str(&text, target, &Options::default()).expect("清单合法");
        let all: String = output.files.iter().map(|f| f.contents.as_str()).collect();
        assert!(!all.contains("cart.checkout") && !all.contains("CartCheckout"), "{target}: 页面工具不参与生成");
        let skipped = output.warnings.iter().any(|w| w.tool == "cart.highlight" && w.message.contains("view"));
        if native.contains(&target) {
            assert!(skipped, "{target}: view 工具应记警告");
            assert!(!all.contains("CartHighlight"), "{target}: view 工具不生成意图");
            assert!(all.contains("OrdersSearch"), "{target}: app 工具照常生成");
        } else {
            assert!(!skipped, "{target}: 类型化接口不过滤");
            assert!(all.contains("CartHighlight") || all.contains("cart_highlight"), "{target}: 类型化接口含 view 工具");
        }
    }
}
