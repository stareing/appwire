use super::*;
use app_mcp_protocol::intents::VOCABULARY;

fn model(tools: Value) -> Model {
    let manifest = app_mcp_manifest::parse(
        &json!({ "manifestVersion": 1, "appId": "hub", "name": "Hub", "tools": tools }).to_string(),
    )
    .expect("清单");
    crate::schema::build(&manifest, None, None)
}

fn play(kind: Value) -> Value {
    let mut props = json!({ "query": { "type": "string" } });
    if !kind.is_null() {
        props["kind"] = kind;
    }
    json!({ "name": "player.play", "description": "播放", "implements": ["media.play@1"],
            "inputSchema": { "type": "object", "properties": props, "required": ["query"] } })
}

fn navigate(destination: Value, extra: Value, required: Value) -> Value {
    let mut props = json!({ "destination": destination });
    if let Value::Object(map) = extra {
        for (k, v) in map {
            props[k] = v;
        }
    }
    json!({ "name": "map.go", "description": "导航", "implements": ["navigation.start@1"],
            "inputSchema": { "type": "object", "properties": props, "required": required } })
}

fn dest(props: Value, required: Value) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

fn full_dest() -> Value {
    dest(
        json!({ "name": { "type": "string" }, "address": { "type": "string" },
                "lat": { "type": "number" }, "lng": { "type": "number" } }),
        json!([]),
    )
}

fn run(tools: Value) -> (Vec<(String, &'static str)>, Vec<String>) {
    let m = model(tools);
    let mut w = Vec::new();
    let found = plans(&m, &mut w)
        .iter()
        .map(|p| (p.tool.info.name.clone(), p.intent.name))
        .collect();
    (found, w.into_iter().map(|w| w.to_string()).collect())
}

fn generate_files(tools: Value, domain: &str, ability: &str) -> (Vec<GeneratedFile>, Vec<Warning>) {
    let m = model(tools);
    let mut w = Vec::new();
    (super::super::generate(&m, domain, ability, true, &mut w), w)
}

fn contents<'a>(files: &'a [GeneratedFile], suffix: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path.ends_with(suffix))
        .unwrap_or_else(|| panic!("缺少 {suffix}"))
        .contents
}

/// 映射表：词表每个动词版本在鸿蒙上的标准意图族。
#[test]
fn family_covers_vocabulary() {
    let mapped: Vec<(String, Option<Family>)> =
        VOCABULARY.iter().map(|d| (d.id(), family(d))).collect();
    assert_eq!(
        mapped,
        [
            ("message.send@1".to_string(), None),
            ("calendar.create@1".to_string(), None),
            ("media.play@1".to_string(), Some(Family::Media)),
            ("file.share@1".to_string(), None),
            ("link.open@1".to_string(), None),
            ("navigation.start@1".to_string(), Some(Family::Navigation)),
        ]
    );
}

/// 标准意图表与 SDK 6.0 / API 20 的 schema 文件一致（名称、版本、垂域、必填属性）。
#[test]
fn standard_intent_table_matches_sdk_schemas() {
    let table: Vec<(&str, &str, &str, Vec<&str>)> =
        [PLAY_VIDEO, PLAY_AUDIO, PLAY_MUSIC_LIST, START_NAVIGATE]
            .iter()
            .map(|i| {
                (
                    i.name,
                    i.version,
                    i.domain,
                    i.props
                        .iter()
                        .filter(|p| p.required)
                        .map(|p| p.name)
                        .collect(),
                )
            })
            .collect();
    assert_eq!(
        table,
        [
            ("PlayVideo", "1.0.2", "MediaDomain", vec!["entityId"]),
            ("PlayAudio", "1.0.1", "MediaDomain", vec!["entityId"]),
            ("PlayMusicList", "1.0.2", "MediaDomain", vec![]),
            ("StartNavigate", "1.0.1", "NavigationDomain", vec![]),
        ]
    );
    let kinds: Vec<Option<&str>> = MEDIA_INTENTS.iter().map(|i| i.media_kind).collect();
    assert_eq!(kinds, [Some("video"), Some("podcast"), Some("playlist")]);
}

#[test]
fn unmapped_verbs_warn_and_generate_nothing() {
    let (found, warnings) = run(json!([
        { "name": "m.send", "description": "x", "implements": ["message.send@1"], "inputSchema": { "type": "object",
          "properties": { "to": { "type": "array", "items": { "type": "string" } }, "text": { "type": "string" } } } },
        { "name": "l.open", "description": "x", "implements": ["link.open@1"],
          "inputSchema": { "type": "object", "properties": { "url": { "type": "string" } } } }
    ]));
    assert!(found.is_empty());
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains("在 HarmonyOS 上没有对应的系统意图")),
        "{warnings:?}"
    );
}

/// media.play：按 kind 枚举逐项选择；没有 kind 枚举或枚举不含可映射取值时警告。
#[test]
fn media_selection_by_kind() {
    let cases: [(Value, &[&str], Option<&str>); 6] = [
        (
            json!({ "type": "string", "enum": ["song", "playlist", "podcast", "video"] }),
            &["PlayVideo", "PlayAudio", "PlayMusicList"],
            None,
        ),
        (
            json!({ "type": "string", "enum": ["video"] }),
            &["PlayVideo"],
            None,
        ),
        (
            json!({ "type": "string", "enum": ["podcast"] }),
            &["PlayAudio"],
            None,
        ),
        (
            json!({ "type": "string", "enum": ["playlist"] }),
            &["PlayMusicList"],
            None,
        ),
        (
            json!({ "type": "string", "enum": ["song", "album"] }),
            &[],
            Some("不含 video / podcast / playlist"),
        ),
        (json!({ "type": "string" }), &[], Some("未声明 kind 枚举")),
    ];
    for (kind, expect, warning) in cases {
        let (found, warnings) = run(json!([play(kind.clone())]));
        let names: Vec<&str> = found.iter().map(|(_, n)| *n).collect();
        assert_eq!(names, expect, "{kind}");
        match warning {
            Some(text) => assert!(
                warnings.len() == 1 && warnings[0].contains(text),
                "{kind}: {warnings:?}"
            ),
            None => assert!(warnings.is_empty(), "{kind}: {warnings:?}"),
        }
    }
    let (found, warnings) = run(json!([play(Value::Null)]));
    assert!(
        found.is_empty() && warnings[0].contains("未声明 kind 枚举"),
        "{warnings:?}"
    );
}

#[test]
fn navigation_conversion_maps_destination_and_mode() {
    let m = model(json!([navigate(
        full_dest(),
        json!({ "mode": { "type": "string", "enum": ["drive", "walk"] } }),
        json!(["destination"])
    )]));
    let plan = navigation_conversion(&m, m.params(&m.tools[0])).expect("可转换");
    let fields: Vec<(&str, &str)> = plan
        .fields
        .iter()
        .map(|f| (f.source.target, f.source.source))
        .collect();
    assert_eq!(
        fields,
        // 按 destination 属性的声明顺序（模型中按名称排序）
        [
            ("address", "address"),
            ("lat", "latitude"),
            ("lng", "longitude"),
            ("name", "locationName")
        ]
    );
    let (_, table) = plan.mode.expect("mode");
    assert_eq!(table, [("Drive", "drive"), ("Walk", "walk")]);

    // mode 为普通字符串：全部交通方式
    let m = model(json!([navigate(
        full_dest(),
        json!({ "mode": { "type": "string" } }),
        json!([])
    )]));
    let plan = navigation_conversion(&m, m.params(&m.tools[0])).expect("可转换");
    assert_eq!(plan.mode.expect("mode").1, TRAFFIC_MODES.to_vec());

    // 类型不符的可选子字段不填（lat 为整数）；只有 name 也可转换
    let m = model(json!([navigate(
        dest(
            json!({ "name": { "type": "string" }, "lat": { "type": "integer" } }),
            json!([])
        ),
        json!({}),
        json!([])
    )]));
    let plan = navigation_conversion(&m, m.params(&m.tools[0])).expect("可转换");
    assert_eq!(plan.fields.len(), 1);
    assert!(plan.mode.is_none());
}

/// 每条导航跳过规则一个工具。
#[test]
fn navigation_skip_rules() {
    let cases = [
        (
            navigate(
                full_dest(),
                json!({ "vehicle": { "type": "string" } }),
                json!(["destination", "vehicle"]),
            ),
            "必填参数 `vehicle` 没有来源",
        ),
        (
            navigate(
                json!({ "type": "object", "additionalProperties": { "type": "string" } }),
                json!({}),
                json!([]),
            ),
            "destination 不是声明了属性的对象",
        ),
        (
            navigate(
                dest(
                    json!({ "name": { "type": "string" }, "zip": { "type": "string" } }),
                    json!(["zip"]),
                ),
                json!({}),
                json!([]),
            ),
            "destination.zip 必填",
        ),
        (
            navigate(
                dest(json!({ "lat": { "type": "integer" } }), json!(["lat"])),
                json!({}),
                json!([]),
            ),
            "destination.lat 必填",
        ),
        (
            navigate(
                dest(json!({ "lat": { "type": "number" } }), json!([])),
                json!({}),
                json!([]),
            ),
            "没有可填入的",
        ),
    ];
    for (tool, text) in cases {
        let (found, warnings) = run(json!([tool]));
        assert!(found.is_empty(), "{text}");
        assert!(
            warnings.len() == 1
                && warnings[0].contains(text)
                && warnings[0].contains("只生成自定义意图"),
            "{text}: {warnings:?}"
        );
    }
    let m = model(
        json!([{ "name": "map.go", "description": "x", "inputSchema": { "type": "object" } }]),
    );
    assert_eq!(
        navigation_conversion(&m, m.params(&m.tools[0]))
            .err()
            .as_deref(),
        Some("缺少 destination")
    );
}

/// 生成：执行器路径、schema 与版本、--ability 生效而 --intent-domain 不生效、insight_intent.json 追加条目。
#[test]
fn generates_executors_with_schema_and_ability() {
    let (files, warnings) = generate_files(
        json!([
            play(json!({ "type": "string", "enum": ["video", "playlist"] })),
            navigate(full_dest(), json!({}), json!(["destination"]))
        ]),
        "ShoppingPlatformsDomain",
        "MainAbility",
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    let video = contents(&files, "standard/HubPlayVideoStandardIntent.ets");
    for line in [
        "intentName: \"PlayVideo\",",
        "domain: \"MediaDomain\",",
        "intentVersion: \"1.0.2\",",
        "schema: \"PlayVideo\",",
        "abilityName: \"MainAbility\",",
        "return HubIntentRuntime.missing(\"entityId\");",
        ".then((resolver: HubMediaEntityResolver) => resolver.playVideo(request))",
        ".then((params: PlayerPlayParams) => handlers.playerPlay(params)));",
    ] {
        assert!(video.contains(line), "缺少 {line}\n{video}");
    }
    assert!(!video.contains("ShoppingPlatformsDomain"));
    let list = contents(&files, "standard/HubPlayMusicListStandardIntent.ets");
    assert!(!list.contains("missing("), "PlayMusicList 没有必填属性");
    let resolver = contents(&files, "appmcp/HubStandardIntents.ets");
    assert!(resolver.contains(
        "playVideo(request: HubPlayVideoRequest): PlayerPlayParams | Promise<PlayerPlayParams>;"
    ));
    assert!(resolver.contains("playMusicList(request: HubPlayMusicListRequest)"));
    assert!(!resolver.contains("playAudio"));
    let nav = contents(&files, "standard/HubStartNavigateStandardIntent.ets");
    assert!(nav.contains("public dstLocation?: HubStandardNavigateLocation;"));
    assert!(nav.contains(
        "export class HubStandardNavigateLocation implements insightIntent.IntentEntity {"
    ));
    assert!(nav.contains("const latValue = wgs84Coordinate(dst.locationSystem, dst.latitude);"));
    assert!(
        !nav.contains("TRAFFIC_MODES"),
        "工具没有 mode 时不生成交通方式表"
    );
    let json = contents(&files, "insight_intent.json");
    assert!(
        json.contains("./ets/insightintents/HubPlayerPlayIntent.ets"),
        "自定义意图照常生成"
    );
    for name in [
        "HubPlayVideoStandardIntent",
        "HubPlayMusicListStandardIntent",
        "HubStartNavigateStandardIntent",
    ] {
        assert!(
            json.contains(&format!("./ets/insightintents/standard/{name}.ets")),
            "{name}"
        );
    }
}

/// 只有导航意图时不生成媒体解析文件；destination 的必填子字段生成缺参检查。
#[test]
fn navigation_only_and_required_destination_field() {
    let (files, _) = generate_files(
        json!([navigate(
            dest(
                json!({ "name": { "type": "string" }, "address": { "type": "string" } }),
                json!(["name"])
            ),
            json!({ "mode": { "type": "string", "enum": ["transit"] } }),
            json!(["destination"])
        )]),
        DEFAULT_DOMAIN,
        DEFAULT_ABILITY,
    );
    assert!(
        !files
            .iter()
            .any(|f| f.path.ends_with("HubStandardIntents.ets"))
    );
    let nav = contents(&files, "HubStartNavigateStandardIntent.ets");
    assert!(nav.contains("return HubIntentRuntime.missing(\"dstLocation.locationName\");"));
    assert!(nav.contains("if (nameValue === undefined && addressValue === undefined) {"));
    assert!(
        !nav.contains("wgs84Coordinate"),
        "没有坐标字段时不生成坐标转换"
    );
    assert!(nav.contains("const TRAFFIC_MODES: Record<string, MapGoMode> = {\n  \"Bus\": \"transit\",\n  \"Subway\": \"transit\",\n};"));
    assert!(nav.contains(
        "mode: this.trafficType === undefined ? undefined : TRAFFIC_MODES[this.trafficType],"
    ));
}
