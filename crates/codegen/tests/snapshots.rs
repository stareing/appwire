//! 快照测试：用 tests/fixtures/shop.json 为每个 target 生成输出，与 tests/snapshots/<target>/ 比较。
//!
//! 设置环境变量 `UPDATE_SNAPSHOTS=1` 时改为写入（更新）期望文件。

use std::path::{Path, PathBuf};

use app_mcp_codegen::{AppIntentsOptions, Options, Output, Target, generate_from_str};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn generate(target: Target, options: &Options) -> Output {
    generate_fixture("shop.json", target, options)
}

fn generate_fixture(fixture: &str, target: Target, options: &Options) -> Output {
    let text = std::fs::read_to_string(crate_dir().join("tests/fixtures").join(fixture))
        .expect("读取示例清单");
    let (output, manifest_warnings) =
        generate_from_str(&text, target, options).expect("示例清单应当合法");
    // intents.json 有意声明词表外动词、deprecated.json 有意给必填参数标 deprecated（清单校验只给警告），其余示例清单不应有警告
    let expected: &[&str] = match fixture {
        "intents.json" => &["tools[7].implements[0]"],
        "deprecated.json" => &["tools[0].inputSchema.properties.status"],
        _ => &[],
    };
    let paths: Vec<String> = manifest_warnings.iter().map(|w| w.path.clone()).collect();
    assert_eq!(paths, expected, "{manifest_warnings:?}");
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
    check_snapshot(target, &Options::default(), target.name());
}

/// 以指定选项生成，与 tests/snapshots/<snapshot>/ 比较。
fn check_snapshot(target: Target, options: &Options, snapshot: &str) {
    check_fixture_snapshot("shop.json", target, options, snapshot);
}

/// 用 tests/fixtures/<fixture> 生成，与 tests/snapshots/<snapshot>/ 比较。
fn check_fixture_snapshot(fixture: &str, target: Target, options: &Options, snapshot: &str) {
    let output = generate_fixture(fixture, target, options);
    let dir = crate_dir().join("tests/snapshots").join(snapshot);
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
        "{snapshot} 生成的文件列表与快照不一致（UPDATE_SNAPSHOTS=1 可更新）"
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
                "{snapshot} 的 {} 与快照不一致（第 {line} 行起）；UPDATE_SNAPSHOTS=1 可更新\n--- 期望\n{}\n--- 实际\n{}",
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

fn app_intents_options(app_intents: AppIntentsOptions) -> Options {
    Options {
        app_intents,
        ..Options::default()
    }
}

/// 扩展布局（iOS 26 基线）：共享包 + 扩展入口 + App 侧包声明。
#[test]
fn snapshot_swift_app_intents_extension() {
    let options = app_intents_options(AppIntentsOptions {
        extension: true,
        ..AppIntentsOptions::default()
    });
    check_snapshot(Target::SwiftAppIntents, &options, "swift-app-intents-extension");
}

/// 扩展布局 + allowedExecutionTargets（iOS 27）+ CancellableIntent（iOS 26.4）。
#[test]
fn snapshot_swift_app_intents_extension_all_options() {
    let options = app_intents_options(AppIntentsOptions {
        extension: true,
        execution_targets: true,
        cancellable: true,
    });
    check_snapshot(Target::SwiftAppIntents, &options, "swift-app-intents-extension-ios27");
}

#[test]
fn app_intents_extension_contract() {
    let base = generate(Target::SwiftAppIntents, &Options::default());
    let swift = generate(Target::Swift, &Options::default());
    let ext = generate(
        Target::SwiftAppIntents,
        &app_intents_options(AppIntentsOptions {
            extension: true,
            ..AppIntentsOptions::default()
        }),
    );
    let find = |out: &Output, path: &str| {
        out.files
            .iter()
            .find(|f| f.path == Path::new(path))
            .unwrap_or_else(|| panic!("缺少 {path}"))
            .contents
            .clone()
    };
    // 类型文件原样放入共享包
    assert_eq!(find(&ext, "ShopIntents/Sources/ShopIntents/ShopTools.swift"), swift.files[0].contents);
    // swift-tools-version 必须是 Package.swift 第一行
    assert!(find(&ext, "ShopIntents/Package.swift").starts_with("// swift-tools-version: 6.0
"));
    // 扩展入口与 App 都按同一个 provider 名登记（开发者只实现一次）
    let setup = "ShopIntentRuntime.configure(ShopIntentHandlersProvider.self)";
    assert!(find(&ext, "ShopIntentsExtension/ShopIntentsExtension.swift").contains(setup));
    assert!(find(&ext, "App/ShopAppIntentsPackage.swift").contains(setup));
    let intents = find(&ext, "ShopIntents/Sources/ShopIntents/ShopAppIntents.swift");
    assert!(intents.contains("public struct ShopIntentsPackage: AppIntentsPackage {}"));
    assert!(!intents.contains("public static var handlers"), "扩展布局不再要求设置 handlers");
    // 未开启 execution_targets 时，foreground 工具给出警告；开启后不再警告
    assert!(ext.warnings.iter().any(|w| w.message.contains("foreground")));
    // 选项全部关闭时输出与历史一致（不含任何可选 API）
    let all: String = base.files.iter().map(|f| f.contents.as_str()).collect();
    for api in ["CancellableIntent", "allowedExecutionTargets", "AppIntentsExtension", "Synchronization"] {
        assert!(!all.contains(api), "缺省输出不应包含 {api}");
    }
}

#[test]
fn app_intents_execution_targets_follow_activation() {
    let out = generate(
        Target::SwiftAppIntents,
        &app_intents_options(AppIntentsOptions {
            execution_targets: true,
            ..AppIntentsOptions::default()
        }),
    );
    assert_eq!(out.files.len(), 2, "未开启扩展时仍是两个文件");
    let text = &out.files[1].contents;
    let targets_of = |intent: &str| {
        let start = text.find(&format!("public struct {intent}: AppIntent")).expect(intent);
        let rest = &text[start..];
        let line = rest
            .lines()
            .find(|l| l.contains("allowedExecutionTargets"))
            .expect("allowedExecutionTargets");
        line.trim().to_string()
    };
    // headless / background → App 或扩展；foreground（含缺省）→ 只在 App
    assert!(targets_of("CatalogSearchIntent").ends_with("{ [.main, .appIntentsExtension] }"));
    assert!(targets_of("CartAddIntent").ends_with("{ [.main, .appIntentsExtension] }"));
    assert!(targets_of("CartCheckoutIntent").ends_with("{ .main }"));
    assert!(targets_of("TodosClearIntent").ends_with("{ .main }"));
    assert!(text.contains("@available(iOS 27.0, macOS 27.0, *)"));
    let ext = generate(
        Target::SwiftAppIntents,
        &app_intents_options(AppIntentsOptions {
            extension: true,
            execution_targets: true,
            cancellable: false,
        }),
    );
    assert!(!ext.warnings.iter().any(|w| w.message.contains("foreground")));
}

#[test]
fn cli_rejects_app_intents_flags_for_other_targets() {
    let out = std::env::temp_dir().join(format!("app-mcp-codegen-cli-ai-{}", std::process::id()));
    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_app-mcp-codegen"))
        .arg("--manifest")
        .arg(crate_dir().join("tests/fixtures/shop.json"))
        .args(["--target", "kotlin", "--app-intents-extension", "--out"])
        .arg(&out)
        .output()
        .expect("运行 CLI");
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("swift-app-intents"));
    let _ = std::fs::remove_dir_all(&out);
}

/// `--standard-intents` 只用于有系统意图映射的原生 target（windows-app-actions 与类型化接口拒绝）。
#[test]
fn cli_rejects_standard_intents_for_other_targets() {
    for target in ["windows-app-actions", "typescript"] {
        let out = std::env::temp_dir().join(format!("app-mcp-codegen-cli-si-{}-{target}", std::process::id()));
        let bad = std::process::Command::new(env!("CARGO_BIN_EXE_app-mcp-codegen"))
            .arg("--manifest")
            .arg(crate_dir().join("tests/fixtures/intents.json"))
            .args(["--target", target, "--standard-intents", "--out"])
            .arg(&out)
            .output()
            .expect("运行 CLI");
        assert!(!bad.status.success(), "{target}");
        assert!(String::from_utf8_lossy(&bad.stderr).contains("--standard-intents"), "{target}");
        assert!(!out.exists(), "{target} 被拒绝时不应写文件");
    }
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

/// `--standard-intents`（spec/intents.md 第 3 节）：tests/fixtures/intents.json 六个试点动词各一个实现者，另含重复动词、
/// 词表外动词与未声明的工具。
fn check_standard_intents(target: Target) {
    let options = Options { standard_intents: true, ..Options::default() };
    check_fixture_snapshot("intents.json", target, &options, &format!("{}-standard-intents", target.name()));
}

#[test]
fn snapshot_swift_app_intents_standard_intents() {
    check_standard_intents(Target::SwiftAppIntents);
}

#[test]
fn snapshot_kotlin_appfunctions_standard_intents() {
    check_standard_intents(Target::KotlinAppFunctions);
}

#[test]
fn snapshot_harmony_insight_intents_standard_intents() {
    check_standard_intents(Target::HarmonyInsightIntents);
}

/// 选项关闭时，声明了 implements 的清单不产生任何系统意图输出：与去掉 implements 的同一清单逐字相同。
#[test]
fn standard_intents_off_ignores_implements() {
    let text = std::fs::read_to_string(crate_dir().join("tests/fixtures/intents.json")).expect("读取清单");
    let with = app_mcp_manifest::load_str(&text).expect("清单").manifest;
    let mut without = with.clone();
    without.tools.iter_mut().for_each(|t| t.implements.clear());
    for target in [Target::SwiftAppIntents, Target::KotlinAppFunctions, Target::HarmonyInsightIntents] {
        let on = app_mcp_codegen::generate(&with, target, &Options::default());
        let off = app_mcp_codegen::generate(&without, target, &Options::default());
        assert_eq!(on.files, off.files, "{target} 关闭 --standard-intents 时输出应与未声明 implements 相同");
    }
}

/// 弃用声明（spec/protocol.md 3.7）：tests/fixtures/deprecated.json 含工具级弃用（有 / 无 replacement 与 until）、可选与必填的
/// 弃用参数、嵌套对象中的弃用参数，message 含引号、`*/`、`$`、反斜杠、`<&`、换行等需按目标语言转义的字符。
#[test]
fn snapshot_deprecated_all_targets() {
    for target in Target::ALL {
        check_fixture_snapshot("deprecated.json", target, &Options::default(), &format!("deprecated-{}", target.name()));
    }
}

/// 弃用标注只来自声明：去掉 deprecated.json 中全部工具级与参数级弃用后，任何 target 的输出都不含弃用标注与说明。
#[test]
fn deprecation_markers_only_from_declarations() {
    let text = std::fs::read_to_string(crate_dir().join("tests/fixtures/deprecated.json")).expect("读取清单");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                map.remove("deprecated");
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut value);
    let plain = value.to_string();
    for target in Target::ALL {
        let (out, _) = generate_from_str(&plain, target, &Options::default()).expect("清单合法");
        let all: String = out.files.iter().map(|f| f.contents.as_str()).collect();
        for marker in ["deprecated", "Deprecated", "Obsolete", "CS0618", "DEPRECATION", "已弃用"] {
            assert!(!all.contains(marker), "{target}: 未声明弃用时不应输出 {marker}");
        }
        let (with, _) = generate_from_str(&text, target, &Options::default()).expect("清单合法");
        let all: String = with.files.iter().map(|f| f.contents.as_str()).collect();
        assert!(all.contains("已弃用") || all.contains("eprecated") || all.contains("Obsolete"), "{target}: 声明弃用时应有标注");
        assert_eq!(out.files.len(), with.files.len(), "{target}: 弃用的工具照常生成，文件数不变");
    }
}

/// 系统意图版本（`--standard-intents`）调用弃用的 handler 时同样不让生成代码自身产生弃用警告：
/// Swift 经 `<Module>DeprecatedToolCaller` 转发，Kotlin 局部 `@Suppress("DEPRECATION")`。
#[test]
fn standard_intents_call_deprecated_handlers_without_warnings() {
    let text = std::fs::read_to_string(crate_dir().join("tests/fixtures/intents.json")).expect("读取清单");
    let mut manifest = app_mcp_manifest::load_str(&text).expect("清单").manifest;
    let tool = manifest.tools.iter_mut().find(|t| t.name == "browser.open").expect("browser.open");
    tool.deprecated = Some(app_mcp_protocol::Deprecation { message: "改用 browser.visit".into(), replacement: None, until: None });
    let options = Options { standard_intents: true, ..Options::default() };
    let find = |out: &Output, suffix: &str| {
        out.files.iter().find(|f| f.path.to_string_lossy().ends_with(suffix)).unwrap_or_else(|| panic!("缺少 {suffix}")).contents.clone()
    };
    let swift = app_mcp_codegen::generate(&manifest, Target::SwiftAppIntents, &options);
    let standard = find(&swift, "HubStandardIntents.swift");
    assert!(standard.contains("HubDeprecatedToolCaller.shared.browserOpen(HubIntentRuntime.requireHandlers(), params)"), "{standard}");
    assert!(standard.contains("// 已弃用：改用 browser.visit"));
    let kotlin = app_mcp_codegen::generate(&manifest, Target::KotlinAppFunctions, &options);
    let standard = find(&kotlin, "HubStandardIntents.kt");
    let call = standard.lines().position(|l| l.contains("handlers.browserOpen(params)")).expect("browserOpen 调用");
    assert_eq!(standard.lines().nth(call - 1).map(str::trim), Some("@Suppress(\"DEPRECATION\")"), "{standard}");
}
