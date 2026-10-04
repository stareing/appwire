use serde_json::json;

use super::*;

#[test]
fn vocabulary_matches_spec_table() {
    let ids: Vec<String> = VOCABULARY.iter().map(IntentDef::id).collect();
    assert_eq!(
        ids,
        ["message.send@1", "calendar.create@1", "media.play@1", "file.share@1", "link.open@1", "navigation.start@1"]
    );
    let required = |verb: &str| -> Vec<(&str, &str)> {
        lookup(verb, 1).unwrap().required.iter().map(|p| (p.name, p.kind.json_type())).collect()
    };
    assert_eq!(required("message.send"), [("to", "array"), ("text", "string")]);
    assert_eq!(required("calendar.create"), [("title", "string"), ("start", "string")]);
    assert_eq!(required("media.play"), [("query", "string")]);
    assert_eq!(required("file.share"), [("files", "array")]);
    assert_eq!(required("link.open"), [("url", "string")]);
    assert_eq!(required("navigation.start"), [("destination", "object")]);
    for d in &VOCABULARY {
        assert!(is_valid_verb(d.verb) && !d.description.is_empty(), "{}", d.verb);
        assert_eq!(IntentRef::parse(&d.id()).unwrap().lookup(), Some(d));
    }
    assert!(lookup("message.send", 2).is_none());
    assert!(is_known_verb("link.open") && !is_known_verb("link.close"));
    assert_eq!(ParamKind::Boolean.json_type(), "boolean");
    assert_eq!((ParamKind::StringArray.item_type(), ParamKind::String.item_type()), (Some("string"), None));
}

#[test]
fn verb_and_version_format() {
    let ok = ["message.send@1", "a.b@1", "x-y.z_w@12", "Mail.Send2@4294967295"];
    for s in ok {
        assert!(IntentRef::parse(s).is_ok(), "{s}");
    }
    let bad = [
        "message.send",      // 缺版本
        "message.send@",     // 空版本
        "message.send@0",    // 主版本须为正
        "message.send@01",   // 前导零
        "message.send@1.0",  // 不是整数
        "message.send@-1",
        "message.send@4294967296", // 超出 u32
        "message@1",         // 只有一段
        "a.b.c@1",           // 三段
        ".send@1",
        "message.@1",
        "1message.send@1",   // 段须以字母开头
        "message.se nd@1",
        "message.send@1@2",
        "",
    ];
    for s in bad {
        assert_eq!(IntentRef::parse(s), Err(IntentError::Format(s.to_owned())), "{s}");
    }
    let long = format!("{}.b@1", "a".repeat(MAX_VERB_LEN - 2));
    assert!(IntentRef::parse(&long).is_ok());
    let too_long = format!("{}.b@1", "a".repeat(MAX_VERB_LEN - 1));
    assert!(IntentRef::parse(&too_long).is_err());
    let r = IntentRef::parse("media.play@3").unwrap();
    assert_eq!((r.verb, r.version, r.to_string()), ("media.play", 3, "media.play@3".to_owned()));
}

#[test]
fn verb_query_with_or_without_version() {
    assert_eq!(parse_verb_query("message.send"), Ok(("message.send", None)));
    assert_eq!(parse_verb_query("message.send@2"), Ok(("message.send", Some(2))));
    for s in ["message", "message.send@", "message.send@0", "a.b.c", "message.send@x"] {
        assert!(parse_verb_query(s).is_err(), "{s}");
    }
}

#[test]
fn implements_list_rules() {
    let list = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(validate_implements(&[]), Ok(()));
    assert_eq!(validate_implements(&list(&["message.send@1", "message.send@2", "x.y@1", "link.open@1"])), Ok(()));
    let five = list(&["a.b@1", "a.b@2", "a.b@3", "a.b@4", "a.b@5"]);
    assert_eq!(validate_implements(&five), Err(IntentError::TooMany(5)));
    let dup = list(&["a.b@1", "c.d@1", "a.b@1"]);
    assert_eq!(implements_errors(&dup), vec![(Some(2), IntentError::Duplicate("a.b@1".into()))]);
    let mixed = list(&["bad", "a.b@1", "a.b@1", "x"]);
    let errors = implements_errors(&mixed);
    assert_eq!(
        errors,
        vec![
            (Some(0), IntentError::Format("bad".into())),
            (Some(2), IntentError::Duplicate("a.b@1".into())),
            (Some(3), IntentError::Format("x".into())),
        ]
    );
    for (_, e) in &errors {
        assert!(!e.to_string().is_empty());
    }
    assert!(IntentError::TooMany(5).to_string().contains('4'));
}

#[test]
fn compatibility_rules() {
    let send = IntentRef::parse("message.send@1").unwrap();
    let full = json!({"type": "object", "properties": {
        "to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}, "extra": {"type": "integer"}
    }});
    assert_eq!(compatibility(send, &full), Compatibility::Compatible);
    // 未声明类型视为一致；type 为数组时包含即可
    let untyped = json!({"type": "object", "properties": {"to": {}, "text": {"type": ["string", "null"]}}});
    assert_eq!(compatibility(send, &untyped), Compatibility::Compatible);
    let cases = [
        (json!({"type": "object"}), "缺少必填参数 to；缺少必填参数 text"),
        (json!({"type": "object", "properties": {"to": {"type": "array"}}}), "缺少必填参数 text"),
        (
            json!({"type": "object", "properties": {"to": {"type": "string"}, "text": {"type": "string"}}}),
            "参数 to 的类型应为 array（声明为 \"string\"）",
        ),
        (
            json!({"type": "object", "properties": {"to": {"type": "array", "items": {"type": "integer"}}, "text": {}}}),
            "参数 to 的元素类型应为 string（声明为 \"integer\"）",
        ),
        (
            json!({"type": "object", "properties": {"to": {"type": ["null"]}, "text": {"type": 3}}}),
            "参数 to 的类型应为 array（声明为 [\"null\"]）；参数 text 的类型应为 string（声明为 3）",
        ),
    ];
    for (schema, reason) in cases {
        assert_eq!(compatibility(send, &schema), Compatibility::Incompatible(reason.to_owned()), "{schema}");
    }
    let nav = IntentRef::parse("navigation.start@1").unwrap();
    let obj = json!({"type": "object", "properties": {"destination": {"type": "object"}}});
    assert_eq!(compatibility(nav, &obj), Compatibility::Compatible);
    let wrong = json!({"type": "object", "properties": {"destination": {"type": "string"}}});
    assert!(matches!(compatibility(nav, &wrong), Compatibility::Incompatible(_)));
    // 不在词表中：无从检查
    let unknown = IntentRef::parse("message.send@2").unwrap();
    assert_eq!(compatibility(unknown, &json!({})), Compatibility::Unknown);
    assert_eq!(compatibility(IntentRef::parse("x.y@1").unwrap(), &full), Compatibility::Unknown);
}
