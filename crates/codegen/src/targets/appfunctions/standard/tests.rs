//! Android 系统意图映射的分支表测试（T-09）：每个动词的过滤器与谓词、ACTION_SEND 区分规则、参数类型规则与兜底。
//! 生成的 Kotlin 在真实 Intent 上的行为由 scripts/verify.sh 的 Robolectric 用例检查。

use serde_json::{Value as Json, json};

use super::table::{ANDROID_INTENTS, lookup};
use super::*;

fn model(tools: Json) -> Model {
    let manifest = json!({"manifestVersion": 1, "appId": "demo", "name": "Demo", "tools": tools});
    let manifest = serde_json::from_value(manifest).expect("清单");
    crate::schema::build_filtered(&manifest, None, None, |_| true)
}

fn tool(name: &str, verb: &str, properties: Json, required: &[&str]) -> Json {
    json!({"name": name, "description": "d", "implements": [format!("{verb}@1")],
           "inputSchema": {"type": "object", "properties": properties, "required": required}})
}

fn run(tools: Json) -> (Vec<GeneratedFile>, Vec<String>) {
    let m = model(tools);
    let mut warnings = Vec::new();
    let files = generate(&m, "demo.app", &mut warnings);
    (files, warnings.iter().map(ToString::to_string).collect())
}

fn contents<'a>(files: &'a [GeneratedFile], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.contents.as_str())
        .unwrap_or_else(|| panic!("缺少 {suffix}"))
}

/// 每个动词一个最小实现者。
fn minimal(verb: &str) -> Json {
    let (props, required): (Json, &[&str]) = match verb {
        "message.send" => (json!({"to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}}), &["to", "text"]),
        "calendar.create" => (json!({"title": {"type": "string"}, "start": {"type": "string"}}), &["title", "start"]),
        "media.play" => (json!({"query": {"type": "string"}}), &["query"]),
        "file.share" => (json!({"files": {"type": "array", "items": {"type": "string"}}}), &["files"]),
        "link.open" => (json!({"url": {"type": "string"}}), &["url"]),
        "navigation.start" => (
            json!({"destination": {"type": "object", "properties": {"lat": {"type": "number"}, "lng": {"type": "number"}}}}),
            &["destination"],
        ),
        other => panic!("未知动词 {other}"),
    };
    tool(&format!("t.{}", verb.replace('.', "_")), verb, props, required)
}

/// 一个过滤器：(action, scheme, mime, BROWSABLE)。
type FilterRow = (&'static str, &'static [&'static str], Option<&'static str>, bool);
/// 一个动词：(动词, 过滤器, 谓词, 提取函数)。
type VerbRow = (&'static str, &'static [FilterRow], &'static [&'static str], &'static str);

const EXPECTED: [VerbRow; 6] = [
    (
        "message.send",
        &[
            ("android.intent.action.SENDTO", &["smsto", "sms", "mmsto", "mms", "mailto"], None, false),
            ("android.intent.action.SEND", &[], Some("text/plain"), false),
        ],
        &["isMessageSendTo", "isTextSend"],
        "messageSend",
    ),
    (
        "calendar.create",
        &[("android.intent.action.INSERT", &[], Some("vnd.android.cursor.dir/event"), false)],
        &["isCalendarInsert"],
        "calendarCreate",
    ),
    ("media.play", &[("android.media.action.MEDIA_PLAY_FROM_SEARCH", &[], None, false)], &["isMediaSearch"], "mediaPlay"),
    (
        "file.share",
        &[("android.intent.action.SEND", &[], Some("*/*"), false), ("android.intent.action.SEND_MULTIPLE", &[], Some("*/*"), false)],
        &["isFileSend"],
        "fileShare",
    ),
    ("link.open", &[("android.intent.action.VIEW", &["https"], None, true)], &["isHttpsView"], "linkOpen"),
    ("navigation.start", &[("android.intent.action.VIEW", &["geo"], None, false)], &["isGeoView"], "navigationStart"),
];

/// 映射表逐项：过滤器、谓词与提取函数，且 support 代码中定义了它们。
#[test]
fn table_rows_match_spec() {
    assert_eq!(ANDROID_INTENTS.len(), EXPECTED.len());
    for (verb, filters, predicates, extractor) in EXPECTED {
        let row = lookup(verb, 1).unwrap_or_else(|| panic!("{verb} 缺少映射"));
        let got: Vec<FilterRow> =
            row.filters.iter().map(|f| (f.action, f.schemes, f.mime, f.browsable)).collect();
        assert_eq!(got, filters.to_vec(), "{verb}");
        assert_eq!(row.predicates, predicates, "{verb}");
        assert_eq!(row.extractor, extractor, "{verb}");
        for name in predicates.iter().chain([&extractor]) {
            assert!(row.code.contains(&format!("fun {name}(intent: Intent)")), "{verb}: 代码中缺少 {name}");
        }
        // 词表的必填参数都有来源
        let def = app_mcp_protocol::intents::lookup(verb, 1).expect("词表");
        for p in def.required {
            assert!(row.params.iter().any(|q| q.name == p.name), "{verb}: 必填参数 {} 没有来源", p.name);
        }
    }
    assert!(lookup("message.send", 2).is_none(), "未知主版本不映射");
    assert!(lookup("notes.create", 1).is_none());
}

/// 每个动词都生成：过滤器进 manifest，谓词进 parse，提取代码进 support。
#[test]
fn every_verb_generates_filter_and_parser() {
    for (verb, filters, predicates, extractor) in EXPECTED {
        let (files, warnings) = run(json!([minimal(verb)]));
        assert!(warnings.is_empty(), "{verb}: {warnings:?}");
        let kt = contents(&files, "DemoStandardIntents.kt");
        let xml = contents(&files, "DemoStandardIntentFilters.xml");
        for (action, schemes, mime, browsable) in filters {
            assert!(xml.contains(&format!("<action android:name=\"{action}\" />")), "{verb}: {xml}");
            for s in *schemes {
                assert!(xml.contains(&format!("<data android:scheme=\"{s}\" />")), "{verb}");
            }
            if let Some(mime) = mime {
                assert!(xml.contains(&format!("<data android:mimeType=\"{mime}\" />")), "{verb}");
            }
            assert_eq!(xml.contains("category.BROWSABLE"), *browsable, "{verb}");
        }
        let cond: Vec<String> = predicates.iter().map(|p| format!("DemoStandardIntentSupport.{p}(intent)")).collect();
        assert!(kt.contains(&format!("{} -> bind(", cond.join(" || "))), "{verb}: {kt}");
        assert!(kt.contains(&format!("fun {extractor}(intent: Intent)")), "{verb}");
        assert!(kt.contains("else -> DemoStandardIntentResult.Unrecognized"), "{verb}: 兜底");
        // 只输出已绑定动词的代码
        for (other, _, _, other_extractor) in EXPECTED {
            if other != verb {
                assert!(!kt.contains(&format!("fun {other_extractor}(")), "{verb} 不应含 {other}");
            }
        }
    }
}

/// ACTION_SEND 的区分规则：带 EXTRA_STREAM 归 file.share，不带归 message.send；两者谓词互斥且覆盖 ACTION_SEND。
#[test]
fn action_send_split_by_stream() {
    let message = lookup("message.send", 1).expect("映射");
    let file = lookup("file.share", 1).expect("映射");
    assert!(message.code.contains(
        "fun isTextSend(intent: Intent): Boolean = intent.action == Intent.ACTION_SEND && !hasStream(intent)"
    ));
    assert!(file.code.contains("(intent.action == Intent.ACTION_SEND && hasStream(intent)) || intent.action == Intent.ACTION_SEND_MULTIPLE"));
    assert!(table::HELPERS.contains("fun hasStream(intent: Intent): Boolean = intent.hasExtra(Intent.EXTRA_STREAM)"));
    // 两者都绑定时两条分支都在 parse 中，且 support 中各只出现一次
    let (files, warnings) = run(json!([minimal("message.send"), minimal("file.share")]));
    assert!(warnings.is_empty(), "{warnings:?}");
    let kt = contents(&files, "StandardIntents.kt");
    assert_eq!(kt.matches("fun hasStream(").count(), 1);
    assert!(kt.contains("DemoStandardIntentSupport.isMessageSendTo(intent) || DemoStandardIntentSupport.isTextSend(intent) -> bind("));
    assert!(kt.contains("DemoStandardIntentSupport.isFileSend(intent) -> bind("));
}

/// 同一 action 的谓词互斥：VIEW 按 scheme（https / geo）区分，SEND 按 EXTRA_STREAM 区分。
#[test]
fn shared_actions_are_disjoint() {
    let view = ["isHttpsView", "isGeoView"];
    let codes: Vec<&str> = ANDROID_INTENTS.iter().map(|i| i.code).collect();
    let all = codes.join("\n");
    assert!(all.contains("intent.action == Intent.ACTION_VIEW && scheme(intent) == \"https\""));
    assert!(all.contains("intent.action == Intent.ACTION_VIEW && scheme(intent) == \"geo\""));
    for p in view {
        assert_eq!(ANDROID_INTENTS.iter().filter(|i| i.predicates.contains(&p)).count(), 1);
    }
}

/// 参数规则：可选参数不在 schema 中不传；枚举限定取值；对象只保留可解码的键；类型不符时可选参数跳过、必填则不生成。
#[test]
fn arg_rules_follow_tool_schema() {
    let (files, warnings) = run(json!([
        tool("player.play", "media.play", json!({
            "query": {"type": "string"},
            "kind": {"type": "string", "enum": ["song", "video"]}}), &["query"]),
        tool("map.go", "navigation.start", json!({
            "destination": {"type": "object", "properties": {
                "name": {"type": "string"}, "lat": {"type": "integer"}, "lng": {"type": "number"}}}}), &["destination"]),
        tool("mail.send", "message.send", json!({
            "to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"},
            "subject": {"type": "integer"}}), &["to", "text"]),
    ]));
    let kt = contents(&files, "StandardIntents.kt");
    assert!(kt.contains(r#"Arg("kind", required = false, allowed = setOf("song", "video"))"#), "{kt}");
    assert!(kt.contains(r#"Arg("destination", required = true, keys = setOf("name", "lng"))"#), "{kt}");
    assert!(kt.contains(r#"listOf(DemoStandardIntentSupport.Arg("to", required = true), DemoStandardIntentSupport.Arg("text", required = true)),"#));
    assert!(!kt.contains(r#"Arg("subject""#) && !kt.contains(r#"Arg("attachments""#), "schema 外与类型不符的参数不传");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("mail.send") && warnings[0].contains("subject") && warnings[0].contains("不传此参数"));
}

/// 不生成的情况：工具必填参数无法从 Intent 得到、必填参数类型不符、动词无映射；全部跳过时不输出文件。
#[test]
fn skips_unbindable_tools() {
    let (files, warnings) = run(json!([
        tool("a.extra", "link.open", json!({"url": {"type": "string"}, "tab": {"type": "integer"}}), &["url", "tab"]),
        tool("b.place", "navigation.start", json!({
            "destination": {"type": "object", "properties": {"city": {"type": "string"}}}}), &["destination"]),
        tool("c.unknown", "notes.create", json!({}), &[]),
    ]));
    assert!(files.is_empty(), "{files:?}");
    // 按工具顺序：参数规则在 standard_intents::bindings 内按工具执行（去重之前）
    let expect = [("a.extra", "必填参数无法从"), ("b.place", "不生成系统意图"), ("c.unknown", "不在标准意图词表中")];
    assert_eq!(warnings.len(), expect.len(), "{warnings:?}");
    for ((tool, text), w) in expect.iter().zip(&warnings) {
        assert!(w.contains(tool) && w.contains(text), "{w}");
    }
}

/// Kotlin 名称冲突：工具名的 Pascal 形式与 Kotlin 内建类型同名时改名；风险原样传递；XML 注释不出现 `--`。
#[test]
fn names_and_comments_are_safe() {
    let (files, _) = run(json!([
        {"name": "string", "description": "d", "risk": "payment", "implements": ["link.open@1"],
         "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}},
    ]));
    let kt = contents(&files, "StandardIntents.kt");
    assert!(!kt.contains("class String("), "{kt}");
    assert!(kt.contains("override val risk: String get() = \"payment\""));
    let (files, _) = run(json!([tool("a--b", "link.open", json!({"url": {"type": "string"}}), &["url"])]));
    let xml = contents(&files, "StandardIntentFilters.xml");
    let comments: Vec<&str> = xml.split("<!--").skip(1).map(|s| s.split("-->").next().unwrap_or("")).collect();
    assert!(comments.iter().all(|c| !c.contains("--")), "{xml}");
    assert!(xml.contains("a- -b"));
}
