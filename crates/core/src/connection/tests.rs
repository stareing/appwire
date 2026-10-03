use super::*;

/// `take_invoke_params` 与 serde 逐条对照（T-09）：接受时结果相同；不接受时 `params` 不变（交给 serde 报错或按其规则接受）。
#[test]
fn take_invoke_params_matches_serde() {
    let cases = [
        json!({"callId": "c", "name": "n", "arguments": {"a": [1, {"b": null}]}, "timeoutMs": 9, "idempotencyKey": "k"}),
        json!({"callId": "c", "name": "n"}),
        json!({"callId": "c", "name": "n", "arguments": null, "timeoutMs": null, "idempotencyKey": null, "x": [1]}),
        json!({"callId": "c", "name": "n", "timeoutMs": u64::MAX}),
        json!({"callId": "c", "name": "n", "timeoutMs": 1.0}),
        json!({"callId": "c", "name": "n", "timeoutMs": -1}),
        json!({"callId": "c", "name": "n", "timeoutMs": "5"}),
        json!({"callId": "c", "name": "n", "idempotencyKey": 1}),
        json!({"callId": "c", "name": 1}),
        json!({"callId": null, "name": "n"}),
        json!({"name": "n"}),
        json!({"callId": "c"}),
        json!(["c", "n"]),
        json!(null),
        json!("c"),
    ];
    for case in cases {
        let serde = serde_json::from_value::<ToolsInvokeParams>(case.clone()).ok();
        let mut v = case.clone();
        match take_invoke_params(&mut v) {
            Some(p) => assert_eq!(Some(p), serde, "{case}"),
            None => assert_eq!(v, case, "不接受时不修改 params：{case}"),
        }
        // 只有 serde 能接受而这里不接受的形态：数组（按字段顺序）。
        if serde.is_some() && !case.is_object() {
            assert!(take_invoke_params(&mut case.clone()).is_none());
        }
    }
}

/// 结果文本与经 `Value` 的序列化逐字节相同（各可省略字段分别出现 / 省略）。
#[test]
fn invoke_result_json_matches_value_serialization() {
    let base = CallOutput { data: json!({"z": 1, "a": [true, "é\n"]}), ..Default::default() };
    let variants = [
        base.clone(),
        CallOutput { state_hints: vec!["a".into(), "b\"".into()], ..base.clone() },
        CallOutput { state_resource: Some("job".into()), status: proto::ResultStatus::Pending, ..base.clone() },
        CallOutput { status: proto::ResultStatus::Noop, summary: Some("无变化".into()), ..base.clone() },
        CallOutput { annotations: Some(proto::ContentAnnotations { priority: Some(1.0), ..Default::default() }), ..base },
    ];
    for out in variants {
        let legacy = to_value(&proto::ToolsInvokeResult {
            data: out.data.clone(),
            state_hints: out.state_hints.clone(),
            annotations: out.annotations.clone(),
            status: out.status,
            state_resource: out.state_resource.clone(),
            summary: out.summary.clone(),
        })
        .to_string();
        assert_eq!(invoke_result_json(&out), legacy);
    }
    for mime in [None, Some("text/plain")] {
        let contents = json!({"b": 1, "a": "x"});
        let legacy = to_value(&proto::ResourcesReadResult { contents: contents.clone(), mime_type: mime.map(str::to_owned) })
            .to_string();
        assert_eq!(read_result_json(&contents, mime), legacy);
    }
    assert_eq!(
        result_message(&RequestId::from("a\"b"), "{}"),
        Message::result(RequestId::from("a\"b"), json!({})).to_json()
    );
}
