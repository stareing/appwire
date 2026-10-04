use app_mcp_protocol::intents::VOCABULARY;
use serde_json::{Value, json};

use super::*;
use crate::targets::app_intents::{self, AppIntentsOptions};

fn model(tools: Value) -> Model {
    let manifest = json!({"manifestVersion": 1, "appId": "demo", "name": "Demo", "tools": tools});
    let manifest = serde_json::from_value(manifest).expect("清单");
    crate::schema::build_filtered(&manifest, None, None, |_| true)
}

fn tool(name: &str, risk: &str, implements: &[&str], properties: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": "d", "risk": risk, "implements": implements,
           "inputSchema": {"type": "object", "properties": properties, "required": required}})
}

fn open_tool(name: &str) -> Value {
    tool(name, "read", &["link.open@1"], json!({"url": {"type": "string", "format": "uri"}}), &["url"])
}

fn run(tools: Value) -> (Option<String>, Vec<String>) {
    let m = model(tools);
    let mut warnings = Vec::new();
    let out = generate(&m, false, &mut warnings);
    (out, warnings.into_iter().map(|w| w.to_string()).collect())
}

/// 映射表每一项（T-09）：可生成、需要实体解析、无映射，覆盖词表全部动词。
#[test]
fn mapping_table_covers_vocabulary() {
    let expect: &[(&str, Option<(&str, bool)>)] = &[
        ("message.send", Some((".messages.sendMessage", false))),
        ("calendar.create", Some((".calendar.createEvent", false))),
        ("media.play", Some((".audio.playAudio", false))),
        ("file.share", None),
        ("link.open", Some((".browser.openURLInTab", true))),
        ("navigation.start", Some((".maps.startNavigation", false))),
    ];
    assert_eq!(VOCABULARY.len(), expect.len(), "词表新增动词时补映射");
    for def in &VOCABULARY {
        let (_, want) = expect.iter().find(|(v, _)| *v == def.verb).expect("词表动词都在期望表中");
        let got = apple_schema(def).map(|s| (s.expr, matches!(s.support, Support::Generate { .. })));
        assert_eq!(got, *want, "{}", def.verb);
    }
    let Support::Generate { availability, unavailable, params, entities, .. } = OPEN_URL_IN_TAB.support else {
        panic!("openURLInTab 应可生成");
    };
    assert_eq!(availability, "iOS 18.0, macOS 15.0, visionOS 2.0, *");
    assert_eq!(unavailable, ["tvOS", "watchOS"]);
    assert_eq!((params[0].name, params[0].swift_type, params[0].tool_param), ("url", "URL", "url"));
    assert_eq!((entities[0].name, entities[0].entity_schema), ("tab", ".browser.tab"));
}

#[test]
fn generates_open_url_in_tab() {
    let (out, warnings) = run(json!([open_tool("browser.open")]));
    assert!(warnings.is_empty(), "{warnings:?}");
    let out = out.expect("生成系统 schema 文件");
    for needle in [
        "未在真实 AppIntents 框架上编译验证",
        "  DemoBrowserTab：@AppEntity(schema: .browser.tab)",
        "@available(iOS 18.0, macOS 15.0, visionOS 2.0, *)\n@available(tvOS, unavailable)\n@available(watchOS, unavailable)\n@AppIntent(schema: .browser.openURLInTab)\npublic struct BrowserOpenOpenURLInTabSchemaIntent: AppIntent {",
        "    @Parameter\n    public var url: URL\n",
        "    @Parameter\n    public var tab: DemoBrowserTab\n",
        "let params = BrowserOpenParams(url: url.absoluteString)",
        "_ = try await DemoIntentRuntime.requireHandlers().browserOpen(params)",
        "return .result()",
    ] {
        assert!(out.contains(needle), "缺少 {needle:?}\n{out}");
    }
    assert!(!out.contains("requestConfirmation"), "只读工具不确认");
    assert!(out.contains("与本文件同一模块"));
}

/// 需要确认的风险等级沿用自定义 intent 的确认；可选的额外参数不传。
#[test]
fn confirms_risky_tool_and_skips_optional_extras() {
    let props = json!({"note": {"type": "string"}, "url": {"type": "string"}});
    let (out, _) = run(json!([tool("tab.load", "destructive", &["link.open@1"], props, &["url"])]));
    let out = out.expect("生成");
    assert!(out.contains("try await requestConfirmation(actionName: .`continue`, dialog: \"确认tab.load？\")"), "{out}");
    assert!(out.contains("let params = TabLoadParams(url: url.absoluteString)"), "{out}");
}

/// 需要实体解析的四个动词：各一条警告，不输出文件。
#[test]
fn entity_schemas_warn_and_emit_nothing() {
    let (out, warnings) = run(json!([
        tool("m.send", "write", &["message.send@1"],
             json!({"to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}}), &["to", "text"]),
        tool("c.add", "write", &["calendar.create@1"],
             json!({"title": {"type": "string"}, "start": {"type": "string"}}), &["title", "start"]),
        tool("p.play", "write", &["media.play@1"], json!({"query": {"type": "string"}}), &["query"]),
        tool("n.go", "write", &["navigation.start@1"], json!({"destination": {"type": "object"}}), &["destination"]),
    ]));
    assert!(out.is_none());
    let expect = [
        ("m.send", ".messages.sendMessage"),
        ("c.add", ".calendar.createEvent"),
        ("p.play", ".audio.playAudio"),
        ("n.go", ".maps.startNavigation"),
    ];
    assert_eq!(warnings.len(), expect.len(), "{warnings:?}");
    for ((tool, schema), w) in expect.iter().zip(&warnings) {
        assert!(w.contains(tool) && w.contains(schema) && w.contains("需要实体解析，未生成") && w.contains("27"), "{w}");
    }
}

/// 跳过规则（T-09），每条规则单独一个清单：无映射、词表外、不兼容、额外必填参数、重复绑定。
#[test]
fn skip_rules() {
    let cases = [
        (
            tool("f.share", "write", &["file.share@1"], json!({"files": {"type": "array", "items": {"type": "string"}}}), &["files"]),
            "在 Apple 上没有对应的系统意图",
        ),
        (tool("x.custom", "write", &["notes.create@1"], json!({}), &[]), "不在标准意图词表中"),
        (tool("bad.url", "read", &["link.open@1"], json!({"url": {"type": "integer"}}), &["url"]), "不兼容"),
        (
            tool("extra.required", "read", &["link.open@1"],
                 json!({"url": {"type": "string"}, "profile": {"type": "string"}}), &["url", "profile"]),
            "必填参数 profile 不在系统 schema `.browser.openURLInTab` 中",
        ),
    ];
    for (t, text) in cases {
        let name = t["name"].as_str().unwrap_or_default().to_string();
        let (out, warnings) = run(json!([t]));
        assert!(out.is_none(), "{name}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains(&name) && warnings[0].contains(text), "{}", warnings[0]);
    }
    let (out, warnings) = run(json!([open_tool("first.open"), open_tool("second.open")]));
    let out = out.expect("first.open 照常生成");
    assert!(out.contains("FirstOpenOpenURLInTabSchemaIntent") && !out.contains("SecondOpen"), "{out}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("second.open") && warnings[0].contains("已由工具 first.open 绑定"), "{}", warnings[0]);
}

/// 兼容性检查不要求声明类型：未声明类型的 url 不是 string，不生成。
#[test]
fn untyped_url_is_skipped() {
    let (out, warnings) = run(json!([tool("raw.open", "read", &["link.open@1"], json!({"url": {}}), &["url"])]));
    assert!(out.is_none());
    assert!(warnings.iter().any(|w| w.contains("参数 url 不是 string")), "{warnings:?}");
}

#[test]
fn entity_type_name_clash_is_skipped() {
    let props = json!({"url": {"type": "string"}, "browserTab": {"type": "string", "enum": ["a", "b"]}});
    let m = model(json!([tool("demo", "read", &["link.open@1"], props, &["url"])]));
    // 枚举类型名 = 工具 Pascal + 属性 Pascal = DemoBrowserTab，与 App 实体类型名相同
    assert!(m.types.iter().any(|t| t.name() == "DemoBrowserTab"), "前提：生成了同名类型");
    let mut warnings = Vec::new();
    assert!(generate(&m, false, &mut warnings).is_none());
    assert!(warnings[0].message.contains("DemoBrowserTab"), "{warnings:?}");
}

/// 与自定义 intent 同名时避让：单字母分段的工具名可以拼出 `URL`。
#[test]
fn intent_name_avoids_custom_intents() {
    let (out, _) = run(json!([
        open_tool("browser.open"),
        tool("browser.open.open.u.r.l.in.tab.schema", "read", &[], json!({}), &[]),
    ]));
    let out = out.expect("生成");
    assert!(!out.contains("struct BrowserOpenOpenURLInTabSchemaIntent:"), "{out}");
    assert!(out.contains("struct BrowserOpenOpenURLInTabSchemaIntent2: AppIntent"), "{out}");
}

/// 与 swift-app-intents 选项组合（T-09）：关闭时不输出；普通布局放根目录；扩展布局放共享包且不加 iOS 27 / 26.4 成员。
#[test]
fn file_placement_with_layouts() {
    let m = model(json!([open_tool("browser.open")]));
    let paths = |options: AppIntentsOptions, standard: bool| -> Vec<(String, String)> {
        let mut warnings = Vec::new();
        app_intents::generate(&m, &options, standard, &mut warnings)
            .into_iter()
            .map(|f| (f.path.display().to_string(), f.contents))
            .collect()
    };
    let off = paths(AppIntentsOptions::default(), false);
    assert!(off.iter().all(|(p, _)| !p.contains("StandardIntents")));
    let plain = paths(AppIntentsOptions::default(), true);
    assert_eq!(plain.len(), off.len() + 1);
    assert_eq!(plain[..off.len()], off[..], "其他文件不变");
    assert_eq!(plain.last().map(|(p, _)| p.as_str()), Some("DemoStandardIntents.swift"));

    let all = AppIntentsOptions { extension: true, execution_targets: true, cancellable: true };
    let ext = paths(all, true);
    let (path, contents) = ext.iter().find(|(p, _)| p.ends_with("DemoStandardIntents.swift")).expect("扩展布局输出");
    assert_eq!(path, "DemoIntents/Sources/DemoIntents/DemoStandardIntents.swift");
    assert!(contents.contains("放在共享包 Sources 目录中"));
    assert!(!contents.contains("allowedExecutionTargets") && !contents.contains("CancellableIntent"), "{contents}");
}
