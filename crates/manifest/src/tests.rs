use super::*;
use serde_json::json;

fn example() -> Value {
    json!({
        "manifestVersion": 1,
        "appId": "shop",
        "name": "示例商城",
        "version": "1.2.0",
        "description": "一个用于演示的购物商城",
        "overview": {
            "summary": "演示用购物商城，可管理待办、浏览商品、操作购物车并结算",
            "body": "## 能力范围\n- 待办的增删改查",
            "locale": "zh-CN"
        },
        "launch": {
            "web": [{ "type": "url", "href": "http://localhost:5173/" }],
            "windows": [{ "type": "uri", "scheme": "shop-app" },
                        { "type": "aumid", "id": "Company.Shop_xxx!App" },
                        { "type": "exe", "path": "%LOCALAPPDATA%\\Shop\\Shop.exe" }],
            "macos": [{ "type": "bundle", "id": "com.company.shop" }],
            "linux": [{ "type": "dbus", "name": "com.company.Shop" },
                      { "type": "desktop", "file": "com.company.Shop.desktop" }]
        },
        "wake": {
            "web": [{ "kind": "web-url", "target": "http://localhost:5173/" }],
            "windows": [{ "kind": "aumid", "target": "Company.Shop_xxx!App" },
                        { "kind": "uri", "target": "shop-app" }],
            "macos": [{ "kind": "uri", "target": "shop-app", "background": true }],
            "linux": [{ "kind": "dbus", "target": "com.company.Shop", "background": true }],
            "android": [{ "kind": "android-intent", "target": "com.company.shop/dev.appmcp.WakeReceiver", "background": true }],
            "ios": [{ "kind": "uri", "target": "shop-app" }]
        },
        "tools": [{
            "name": "orders.search",
            "title": "搜索订单",
            "description": "按关键词搜索历史订单",
            "inputSchema": { "type": "object", "properties": { "keyword": { "type": "string" } } },
            "risk": "read",
            "activation": "headless"
        }],
        "resources": [{ "name": "cart.state", "description": "当前购物车内容与总价" }]
    })
}

fn with(mut base: Value, pointer: &str, value: Value) -> Manifest {
    *base.pointer_mut(pointer).expect("pointer exists") = value;
    serde_json::from_value(base).expect("parses")
}

fn with_pages(pages: Value) -> Manifest {
    let mut base = example();
    base["pages"] = pages;
    serde_json::from_value(base).expect("parses")
}

fn page_tool(name: &str) -> Value {
    json!({ "name": name, "description": "d", "inputSchema": { "type": "object" }, "surface": "view" })
}

fn error_paths(v: &Validation) -> Vec<&str> {
    v.errors.iter().map(|e| e.path.as_str()).collect()
}

#[test]
fn pages_parse_roundtrip_and_lookup() {
    let m = with_pages(json!([
        { "name": "cart", "title": "购物车", "description": "购物车页面", "route": "/cart",
          "tools": [page_tool("cart.checkout")] },
        { "name": "orders.detail", "params": { "type": "object", "properties": { "id": { "type": "string" } } },
          "navigable": false, "activation": "foreground" }
    ]));
    let v = m.validate();
    assert!(v.is_ok(), "{:?}", v.errors);
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    assert!(m.pages[0].navigable, "navigable 缺省 true");
    assert!(!m.pages[1].navigable);
    assert_eq!(m.pages[1].activation, Some(Activation::Foreground));
    let (p, t) = m.page_tool("cart.checkout").unwrap();
    assert_eq!((p.name.as_str(), t.surface), ("cart", app_mcp_protocol::ToolSurface::View));
    assert!(m.page_tool("orders.search").is_none(), "顶层工具不是页面工具");
    assert!(m.tool("cart.checkout").is_none(), "页面工具不是静态工具");
    assert_eq!(m.page("orders.detail").unwrap().route, None);
    let out = serde_json::to_value(&m).unwrap();
    assert!(out["pages"][0].get("navigable").is_none(), "缺省 true 不序列化");
    assert_eq!(out["pages"][1]["navigable"], json!(false));
    assert_eq!(serde_json::from_value::<Manifest>(out).unwrap(), m);
    // 没有 pages 时不序列化
    let plain: Manifest = serde_json::from_value(example()).unwrap();
    assert!(serde_json::to_value(&plain).unwrap().get("pages").is_none());
}

#[test]
fn page_name_rules() {
    let v = with_pages(json!([{ "name": "bad name" }, { "name": "a" }, { "name": "a" }])).validate();
    assert_eq!(error_paths(&v), vec!["pages[0].name", "pages[2].name"]);
    let v = with_pages(json!([{ "name": "a", "description": "" }])).validate();
    assert_eq!(error_paths(&v), vec!["pages[0].description"]);
}

#[test]
fn page_params_must_be_object_schema() {
    for bad in [json!([]), json!({ "type": "string" }), json!({})] {
        let v = with_pages(json!([{ "name": "a", "params": bad }])).validate();
        assert_eq!(error_paths(&v), vec!["pages[0].params"]);
    }
}

#[test]
fn page_tools_follow_tool_rules() {
    let v = with_pages(json!([{ "name": "a", "tools": [
        { "name": "x y", "description": "d", "inputSchema": { "type": "object" } },
        { "name": "t1", "description": "", "inputSchema": { "type": "string" } },
        { "name": "t2", "description": "d", "inputSchema": [] , "outputSchema": true }
    ] }]))
    .validate();
    assert_eq!(
        error_paths(&v),
        vec![
            "pages[0].tools[0].name",
            "pages[0].tools[1].description",
            "pages[0].tools[1].inputSchema.type",
            "pages[0].tools[2].inputSchema",
            "pages[0].tools[2].outputSchema"
        ]
    );
    let v = with_pages(json!([{ "name": "a", "tools": [page_tool("shop.info")] }])).validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings[0].path, "pages[0].tools[0].name");
}

#[test]
fn background_tool_rules() {
    let view_named = |name: &str, alt: &str| json!({ "name": name, "description": "d", "inputSchema": { "type": "object" },
        "surface": "view", "backgroundTool": alt });
    let view = |alt: &str| view_named("v", alt);
    // 指向顶层 app 工具：合法、无警告，往返不丢
    let m = with_pages(json!([{ "name": "a", "tools": [view("orders.search")] }]));
    let v = m.validate();
    assert!(v.is_ok() && v.warnings.is_empty(), "{:?} {:?}", v.errors, v.warnings);
    assert_eq!(m.page_tool("v").unwrap().1.background_tool.as_deref(), Some("orders.search"));
    assert_eq!(serde_json::to_value(&m).unwrap()["pages"][0]["tools"][0]["backgroundTool"], "orders.search");
    // 不合法的名称、指向自身、指向 view 工具：错误
    let v = with_pages(json!([{ "name": "a", "tools": [view("x y"), view_named("v2", "v2")] }])).validate();
    assert_eq!(error_paths(&v), vec!["pages[0].tools[0].backgroundTool", "pages[0].tools[1].backgroundTool"]);
    let mut other = page_tool("w");
    other["surface"] = json!("view");
    let v = with_pages(json!([{ "name": "a", "tools": [view("w"), other] }])).validate();
    assert_eq!(error_paths(&v), vec!["pages[0].tools[0].backgroundTool"]);
    // 未在清单中声明：警告；app 工具上声明：警告
    let v = with_pages(json!([{ "name": "a", "tools": [view("runtime.only")] }])).validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings[0].path, "pages[0].tools[0].backgroundTool");
    let mut m = with_pages(json!([]));
    m.tools[0].background_tool = Some("x".into());
    let v = m.validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings.iter().filter(|w| w.path == "tools[0].backgroundTool").count(), 2, "{:?}", v.warnings);
}

/// 工具 `implements`（spec/manifest.md 第 3 节）：每条规则一个用例，顶层与页面内工具相同。
#[test]
fn implements_rules() {
    let send_schema = json!({ "type": "object", "properties": { "to": { "type": "array", "items": { "type": "string" } },
        "text": { "type": "string" } } });
    let top = |implements: Value, schema: Value| {
        let mut m = example();
        m["tools"][0]["implements"] = implements;
        m["tools"][0]["inputSchema"] = schema;
        serde_json::from_value::<Manifest>(m).expect("parses")
    };
    let warning_paths = |v: &Validation| v.warnings.iter().map(|w| w.path.clone()).collect::<Vec<_>>();
    // 合法且兼容：无错误、无警告，往返不丢；未声明时不序列化
    let m = top(json!(["message.send@1"]), send_schema.clone());
    let v = m.validate();
    assert!(v.is_ok() && v.warnings.is_empty(), "{:?} {:?}", v.errors, v.warnings);
    assert_eq!(serde_json::to_value(&m).unwrap()["tools"][0]["implements"], json!(["message.send@1"]));
    assert!(serde_json::to_value(serde_json::from_value::<Manifest>(example()).unwrap()).unwrap()["tools"][0].get("implements").is_none());
    // 格式不合法：错误（逐项路径）
    let v = top(json!(["message.send", "ok.verb@1", "a.b@0"]), send_schema.clone()).validate();
    assert_eq!(error_paths(&v), vec!["tools[0].implements[0]", "tools[0].implements[2]"]);
    // 重复：错误
    let v = top(json!(["x.y@1", "x.y@1"]), send_schema.clone()).validate();
    assert_eq!(error_paths(&v), vec!["tools[0].implements[1]"]);
    // 超过 4 项：错误（整个字段）
    let v = top(json!(["a.b@1", "a.b@2", "a.b@3", "a.b@4", "a.b@5"]), send_schema.clone()).validate();
    assert_eq!(error_paths(&v), vec!["tools[0].implements"]);
    // 未知动词 / 未知版本：警告
    let v = top(json!(["x.y@1", "message.send@2"]), send_schema.clone()).validate();
    assert!(v.is_ok());
    assert_eq!(warning_paths(&v), vec!["tools[0].implements[0]", "tools[0].implements[1]"]);
    assert!(v.warnings[0].message.contains("known: false"));
    // 不兼容（缺必填参数 / 类型不一致）：警告，带原因
    let v = top(json!(["message.send@1"]), json!({ "type": "object", "properties": { "to": { "type": "string" } } })).validate();
    assert!(v.is_ok());
    assert_eq!(warning_paths(&v), vec!["tools[0].implements[0]"]);
    assert!(v.warnings[0].message.contains("缺少必填参数 text"), "{}", v.warnings[0].message);
    assert!(v.warnings[0].message.contains("参数 to 的类型应为 array"), "{}", v.warnings[0].message);
    // 格式错误的项不再给出词表警告
    let v = top(json!(["bad"]), json!({ "type": "object" })).validate();
    assert_eq!((error_paths(&v), v.warnings.len()), (vec!["tools[0].implements[0]"], 0));
    // 页面内工具：同一规则
    let mut t = page_tool("p");
    t["implements"] = json!(["link.open@1", "link.open@1"]);
    let v = with_pages(json!([{ "name": "a", "tools": [t] }])).validate();
    assert_eq!(error_paths(&v), vec!["pages[0].tools[0].implements[1]"]);
    assert_eq!(warning_paths(&v), vec!["pages[0].tools[0].implements[0]"], "inputSchema 没有 url：不兼容");
}

#[test]
fn tool_names_unique_across_pages() {
    // 与顶层工具重名、与其他页面的工具重名
    let v = with_pages(json!([
        { "name": "a", "tools": [page_tool("orders.search"), page_tool("t")] },
        { "name": "b", "tools": [page_tool("t")] }
    ]))
    .validate();
    assert_eq!(error_paths(&v), vec!["pages[0].tools[0].name", "pages[1].tools[0].name"]);
}

#[test]
fn tool_page_field_consistency() {
    let mut t = page_tool("t");
    t["page"] = json!("other");
    let v = with_pages(json!([{ "name": "a", "tools": [t] }])).validate();
    assert_eq!(error_paths(&v), vec!["pages[0].tools[0].page"]);
    let mut t = page_tool("t");
    t["page"] = json!("a");
    assert!(with_pages(json!([{ "name": "a", "tools": [t] }])).validate().is_ok());
    // 顶层工具引用未声明的页面：警告
    let mut m = with_pages(json!([{ "name": "a" }]));
    m.tools[0].page = Some("missing".into());
    let v = m.validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings[0].path, "tools[0].page");
    m.tools[0].page = Some("a".into());
    assert!(m.validate().warnings.is_empty());
}

#[test]
fn tool_name_with_app_id_prefix_warns() {
    let m = with(example(), "/tools/0/name", json!("shop.info"));
    let v = m.validate();
    assert!(v.is_ok(), "协议上仍合法：{:?}", v.errors);
    assert_eq!(v.warnings.len(), 1, "{:?}", v.warnings);
    assert_eq!(v.warnings[0].path, "tools[0].name");
    assert!(v.warnings[0].message.contains("\"info\""));
}

#[test]
fn parses_spec_example() {
    let m = parse(&example().to_string()).unwrap();
    assert_eq!(m.app_id, "shop");
    assert_eq!(m.tools.len(), 1);
    assert_eq!(m.tools[0].risk, app_mcp_protocol::Risk::Read);
    assert_eq!(m.web_url(), Some("http://localhost:5173/"));
    assert_eq!(m.launch.windows.len(), 3);
    assert!(matches!(
        m.launch.linux[1],
        LaunchEntry::Known(KnownLaunch::Desktop { .. })
    ));
    let v = m.validate();
    assert!(v.is_ok(), "{:?}", v.errors);
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
}

#[test]
fn overview_parsing_and_validation() {
    let m = parse(&example().to_string()).unwrap();
    let ov = m.overview.as_ref().unwrap();
    assert!(ov.summary.starts_with("演示用购物商城"));
    assert_eq!(ov.locale.as_deref(), Some("zh-CN"));
    assert_eq!(
        serde_json::to_value(&m).unwrap()["overview"]["body"],
        json!("## 能力范围\n- 待办的增删改查")
    );

    // summary 为空 → 错误
    let v = with(example(), "/overview/summary", json!("  ")).validate();
    assert_eq!(v.errors.len(), 1);
    assert_eq!(v.errors[0].path, "overview.summary");

    // 超长 → 警告（按字符计数，不是字节）
    let v = with(example(), "/overview/summary", json!("商".repeat(101))).validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings.len(), 1);
    assert!(
        with(example(), "/overview/summary", json!("商".repeat(100)))
            .validate()
            .warnings
            .is_empty()
    );
    let v = with(example(), "/overview/body", json!("x".repeat(2001))).validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings[0].path, "overview.body");

    // 缺 summary → 结构错误
    let mut base = example();
    base["overview"] = json!({ "body": "x" });
    assert!(parse(&base.to_string()).is_err());

    // 可省略
    let mut base = example();
    base.as_object_mut().unwrap().remove("overview");
    let m: Manifest = serde_json::from_value(base).unwrap();
    assert!(m.overview.is_none());
    assert!(m.validate().is_ok());
}

#[test]
fn roundtrip_keeps_fields() {
    let m = parse(&example().to_string()).unwrap();
    let back: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert_eq!(back, m);
    assert_eq!(serde_json::to_value(&m).unwrap()["appId"], json!("shop"));
}

#[test]
fn minimal_manifest() {
    let m = parse(r#"{"manifestVersion":1,"appId":"x","name":"X"}"#).unwrap();
    assert!(m.validate().is_ok());
    assert!(m.launch.is_empty());
    assert_eq!(m.web_url(), None);
}

#[test]
fn unknown_launch_type_is_preserved_with_warning() {
    let entry = json!({ "type": "flatpak", "ref": "com.company.Shop", "extra": [1, 2] });
    let m = with(example(), "/launch/linux/0", entry.clone());
    assert_eq!(m.launch.linux[0], LaunchEntry::Other(entry.clone()));
    let v = m.validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings.len(), 1);
    assert!(v.warnings[0].path.starts_with("launch.linux[0]"));
    // 原样透传
    let out = serde_json::to_value(&m).unwrap();
    assert_eq!(out["launch"]["linux"][0], entry);
}

#[test]
fn unknown_platform_is_preserved_with_warning() {
    let mut base = example();
    base["launch"]["android"] = json!([{ "type": "intent", "action": "x" }]);
    let m: Manifest = serde_json::from_value(base).unwrap();
    assert!(m.launch.other.contains_key("android"));
    let v = m.validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings.len(), 1);
    assert_eq!(
        serde_json::to_value(&m).unwrap()["launch"]["android"][0]["type"],
        json!("intent")
    );
}

#[test]
fn known_launch_type_missing_field_is_error() {
    let m = with(example(), "/launch/web/0", json!({ "type": "url" }));
    assert!(!m.validate().is_ok());
    let m = with(example(), "/launch/web/0", json!({ "href": "http://x" }));
    assert!(!m.validate().is_ok());
}

#[test]
fn launch_platform_mismatch_warns() {
    let m = with(
        example(),
        "/launch/macos/0",
        json!({ "type": "aumid", "id": "x" }),
    );
    let v = m.validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings.len(), 1);
}

#[test]
fn wake_parses_and_roundtrips() {
    let m = parse(&example().to_string()).unwrap();
    assert_eq!(m.wake.windows.len(), 2);
    let d = m.wake.descriptors("android").next().unwrap();
    assert_eq!(d.kind, WakeKind::AndroidIntent);
    assert!(d.background);
    let d = m.wake.descriptors("windows").next().unwrap();
    assert_eq!((d.kind, d.background), (WakeKind::Aumid, false));
    assert_eq!(
        m.wake_descriptor("web").unwrap().target.as_deref(),
        Some("http://localhost:5173/")
    );
    assert!(m.wake_descriptor("unknown").is_none());
    // background: false 序列化时省略
    let out = serde_json::to_value(&m).unwrap();
    assert_eq!(
        out["wake"]["windows"][1],
        json!({ "kind": "uri", "target": "shop-app" })
    );
    assert_eq!(out["wake"]["macos"][0]["background"], json!(true));
    let back: Manifest = serde_json::from_value(out).unwrap();
    assert_eq!(back, m);
    // 缺省
    let m = parse(r#"{"manifestVersion":1,"appId":"x","name":"X"}"#).unwrap();
    assert!(m.wake.is_empty());
    assert!(
        !serde_json::to_value(&m)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("wake")
    );
}

#[test]
fn web_wake_falls_back_to_launch_url() {
    let mut base = example();
    base.as_object_mut().unwrap().remove("wake");
    let m: Manifest = serde_json::from_value(base).unwrap();
    let d = m.wake_descriptor("web").unwrap();
    assert_eq!(d.kind, WakeKind::WebUrl);
    assert_eq!(d.target.as_deref(), Some("http://localhost:5173/"));
    assert!(m.wake_descriptor("windows").is_none());
    // 显式声明 none 时不回退
    let m = with(example(), "/wake/web", json!([{ "kind": "none" }]));
    assert_eq!(m.wake_descriptor("web").unwrap().kind, WakeKind::None);
    assert!(m.validate().is_ok());
}

#[test]
fn wake_target_rules() {
    let err = |pointer: &str, value: Value| {
        let v = with(example(), pointer, value).validate();
        assert!(!v.is_ok(), "{pointer}");
        v.errors[0].path.clone()
    };
    assert_eq!(
        err("/wake/web/0", json!({ "kind": "web-url" })),
        "wake.web[0].target"
    );
    assert_eq!(
        err("/wake/web/0", json!({ "kind": "web-url", "target": " " })),
        "wake.web[0].target"
    );
    err(
        "/wake/web/0",
        json!({ "kind": "web-url", "target": "shop-app://x" }),
    );
    err(
        "/wake/windows/1",
        json!({ "kind": "uri", "target": "shop app" }),
    );
    err(
        "/wake/windows/1",
        json!({ "kind": "uri", "target": "https" }),
    );
    err(
        "/wake/android/0",
        json!({ "kind": "android-intent", "target": "com.company.shop" }),
    );
    // 字段类型错误 → 保留为 Other，报错
    let m = with(
        example(),
        "/wake/macos/0",
        json!({ "kind": "uri", "target": 1 }),
    );
    assert!(matches!(m.wake.macos[0], WakeEntry::Other(_)));
    assert!(!m.validate().is_ok());
    err(
        "/wake/macos/0",
        json!({ "kind": "uri", "target": "x", "background": "yes" }),
    );
    err("/wake/macos/0", json!({ "target": "x" }));
    err("/wake/macos/0", json!("shop-app"));
}

#[test]
fn wake_warnings() {
    let warn = |pointer: &str, value: Value| {
        let v = with(example(), pointer, value).validate();
        assert!(v.is_ok(), "{pointer}: {:?}", v.errors);
        assert_eq!(v.warnings.len(), 1, "{pointer}: {:?}", v.warnings);
        v.warnings[0].message.clone()
    };
    // 平台不匹配
    assert!(
        warn("/wake/macos/0", json!({ "kind": "aumid", "target": "x" })).contains("不适用")
    );
    // 网页不能后台唤醒
    assert!(
        warn(
            "/wake/web/0",
            json!({ "kind": "web-url", "target": "https://a/", "background": true })
        )
        .contains("background")
    );
    warn(
        "/wake/web/0",
        json!({ "kind": "web-url", "target": "https://a/#x" }),
    );
    warn("/wake/linux/0", json!({ "kind": "dbus", "target": "shop" }));
    warn("/wake/ios/0", json!({ "kind": "none", "target": "x" }));
    // 未知 kind 原样保留
    let entry = json!({ "kind": "flatpak", "ref": "com.company.Shop" });
    let m = with(example(), "/wake/linux/0", entry.clone());
    assert_eq!(m.wake.linux[0], WakeEntry::Other(entry.clone()));
    assert_eq!(serde_json::to_value(&m).unwrap()["wake"]["linux"][0], entry);
    assert_eq!(m.validate().warnings.len(), 1);
    assert_eq!(m.wake.descriptors("linux").count(), 0);
    // 未知平台原样保留
    let mut base = example();
    base["wake"]["harmony"] = json!([{ "kind": "uri", "target": "shop" }]);
    let m: Manifest = serde_json::from_value(base).unwrap();
    assert!(m.wake.other.contains_key("harmony"));
    let v = m.validate();
    assert!(v.is_ok());
    assert_eq!(v.warnings[0].path, "wake.harmony");
}

#[test]
fn wrong_version() {
    let v = with(example(), "/manifestVersion", json!(2)).validate();
    assert_eq!(v.errors.len(), 1);
    assert_eq!(v.errors[0].path, "manifestVersion");
}

#[test]
fn bad_app_id() {
    assert!(!with(example(), "/appId", json!("Shop")).validate().is_ok());
    assert!(
        !with(example(), "/appId", json!("my.app"))
            .validate()
            .is_ok()
    );
    assert!(!with(example(), "/appId", json!("")).validate().is_ok());
}

#[test]
fn reserved_app_ids() {
    for id in RESERVED_APP_IDS {
        let v = with(example(), "/appId", json!(id)).validate();
        assert_eq!(v.errors.len(), 1, "{id}");
        assert!(v.errors[0].message.contains("保留"));
    }
    assert!(with(example(), "/appId", json!("apps2")).validate().is_ok());
}

#[test]
fn bad_tool_name_and_duplicates() {
    assert!(
        !with(example(), "/tools/0/name", json!("has space"))
            .validate()
            .is_ok()
    );
    let mut base = example();
    let tool = base["tools"][0].clone();
    base["tools"].as_array_mut().unwrap().push(tool);
    let m: Manifest = serde_json::from_value(base).unwrap();
    let v = m.validate();
    assert_eq!(v.errors.len(), 1);
    assert!(v.errors[0].message.contains("重复"));
}

/// `resources[].realtime`（spec/manifest.md、spec/lifecycle.md 第 13 节 B3）：缺省 false，可声明 true。
#[test]
fn resource_realtime_flag() {
    let m: Manifest = serde_json::from_value(example()).unwrap();
    assert!(!m.resource("cart.state").unwrap().realtime);
    let mut base = example();
    base["resources"][0]["realtime"] = json!(true);
    let m: Manifest = serde_json::from_value(base).unwrap();
    assert!(m.validate().is_ok());
    assert!(m.resource("cart.state").unwrap().realtime);
}

#[test]
fn annotations_and_output_schema() {
    let mut base = example();
    base["tools"][0]["annotations"] = json!({"readOnlyHint": true, "openWorldHint": false});
    base["tools"][0]["outputSchema"] = json!({"type": "array", "items": {"type": "string"}});
    base["resources"][0]["annotations"] = json!({"audience": ["user"], "priority": 0.8});
    let m: Manifest = serde_json::from_value(base.clone()).unwrap();
    assert!(m.validate().is_ok(), "{:?}", m.validate().errors);
    let t = &m.tools[0];
    assert_eq!(t.annotations.as_ref().unwrap().open_world_hint, Some(false));
    assert_eq!(t.output_schema.as_ref().unwrap()["type"], "array");
    assert_eq!(m.resources[0].annotations.as_ref().unwrap().priority, Some(0.8));
    // 往返保持字段
    let back: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert_eq!(back, m);
    // outputSchema 不是对象 → 错误；注解字段类型不对 → 解析失败
    base["tools"][0]["outputSchema"] = json!("array");
    let m: Manifest = serde_json::from_value(base.clone()).unwrap();
    assert!(m.validate().errors.iter().any(|e| e.path == "tools[0].outputSchema"));
    base["tools"][0]["annotations"] = json!({"readOnlyHint": "yes"});
    assert!(serde_json::from_value::<Manifest>(base).is_err());
}

#[test]
fn bad_resource_name_and_duplicates() {
    assert!(
        !with(example(), "/resources/0/name", json!(""))
            .validate()
            .is_ok()
    );
    let mut base = example();
    let res = base["resources"][0].clone();
    base["resources"].as_array_mut().unwrap().push(res);
    let m: Manifest = serde_json::from_value(base).unwrap();
    assert!(!m.validate().is_ok());
}

#[test]
fn input_schema_must_be_object_type() {
    assert!(
        !with(
            example(),
            "/tools/0/inputSchema",
            json!({ "type": "array" })
        )
        .validate()
        .is_ok()
    );
    assert!(
        !with(example(), "/tools/0/inputSchema", json!({}))
            .validate()
            .is_ok()
    );
    assert!(
        !with(example(), "/tools/0/inputSchema", json!("object"))
            .validate()
            .is_ok()
    );
}

#[test]
fn empty_descriptions() {
    assert!(
        !with(example(), "/description", json!(""))
            .validate()
            .is_ok()
    );
    assert!(
        !with(example(), "/tools/0/description", json!(""))
            .validate()
            .is_ok()
    );
    assert!(
        !with(example(), "/resources/0/description", json!(""))
            .validate()
            .is_ok()
    );
}

#[test]
fn structural_errors() {
    assert!(matches!(
        parse("not json"),
        Err(ManifestError::Parse { .. })
    ));
    assert!(parse(r#"{"manifestVersion":1,"name":"x"}"#).is_err()); // 缺 appId
    assert!(parse(r#"{"manifestVersion":"1","appId":"x","name":"x"}"#).is_err());
    assert!(matches!(
        load_str(r#"{"manifestVersion":1,"appId":"apps","name":"x"}"#),
        Err(ManifestError::Invalid { .. })
    ));
}

#[test]
fn load_file_and_dir() {
    let dir =
        std::env::temp_dir().join(format!("app-mcp-manifest-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.json"), example().to_string()).unwrap();
    std::fs::write(dir.join("b.json"), "{ broken").unwrap();
    std::fs::write(
        dir.join("c.json"),
        r#"{"manifestVersion":2,"appId":"c","name":"C"}"#,
    )
    .unwrap();
    std::fs::write(dir.join("notes.txt"), "ignored").unwrap();

    let loaded = load_file(dir.join("a.json")).unwrap();
    assert_eq!(loaded.manifest.app_id, "shop");
    assert_eq!(loaded.path.as_deref(), Some(dir.join("a.json").as_path()));
    assert!(matches!(
        load_file(dir.join("missing.json")),
        Err(ManifestError::Io { .. })
    ));

    let all = load_dir(&dir).unwrap();
    assert_eq!(all.len(), 3);
    assert!(all[0].is_ok());
    assert!(matches!(
        &all[1],
        Err(ManifestError::Parse { path: Some(_), .. })
    ));
    match &all[2] {
        Err(e @ ManifestError::Invalid { path: Some(_), .. }) => {
            assert!(e.to_string().contains("manifestVersion"))
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(load_dir(dir.join("nope")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

fn with_events(events: Value) -> Manifest {
    let mut base = example();
    base["events"] = events;
    serde_json::from_value(base).expect("parses")
}

#[test]
fn events_parse_and_roundtrip() {
    let m = with_events(json!([
        { "name": "order.shipped", "description": "订单已发货", "payloadSchema": { "type": "object" } },
        { "name": "download.done", "description": "下载完成" }
    ]));
    assert!(m.validate().is_ok());
    assert_eq!(m.events.len(), 2);
    assert_eq!(m.events[0].payload_schema, Some(json!({ "type": "object" })));
    let back: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert_eq!(back, m);
    // 无事件时不序列化。
    let plain: Manifest = serde_json::from_value(example()).unwrap();
    assert!(plain.events.is_empty());
    assert!(serde_json::to_value(&plain).unwrap().get("events").is_none());
}

#[test]
fn event_rules() {
    let v = with_events(json!([{ "name": "bad name", "description": "d" }])).validate();
    assert_eq!(error_paths(&v), ["events[0].name"]);
    let v = with_events(json!([
        { "name": "a", "description": "d" },
        { "name": "a", "description": "d" }
    ]))
    .validate();
    assert_eq!(error_paths(&v), ["events[1].name"]);
    assert!(v.errors[0].message.contains("重复"));
    let v = with_events(json!([{ "name": "a", "description": " " }])).validate();
    assert_eq!(error_paths(&v), ["events[0].description"]);
    let v = with_events(json!([{ "name": "a", "description": "d", "payloadSchema": "x" }])).validate();
    assert_eq!(error_paths(&v), ["events[0].payloadSchema"]);
    // 事件与工具分属不同命名空间：与工具同名不算重复。
    let v = with_events(json!([{ "name": "orders.search", "description": "d" }])).validate();
    assert!(v.is_ok(), "{v:?}");
}

#[test]
fn event_name_with_app_id_prefix_warns() {
    let v = with_events(json!([{ "name": "shop.order.shipped", "description": "d" }])).validate();
    assert!(v.is_ok());
    assert!(v.warnings.iter().any(|w| w.path == "events[0].name"), "{v:?}");
}
