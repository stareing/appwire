//! v20 事件（spec/protocol.md 3.5）：参数校验、错误码映射，以及经 C 接口与 fake_host 往返。

use super::*;
use crate::events::parse_payload_schema;

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn config_for(id: &CString, name: &CString, url: &CString) -> AmClientConfig {
    AmClientConfig {
        app_id: id.as_ptr(),
        app_name: name.as_ptr(),
        instance_id: ptr::null(),
        host_url: url.as_ptr(),
        app_version: ptr::null(),
        instance_title: ptr::null(),
        token: ptr::null(),
        launch_token: ptr::null(),
        client_kind: 0,
        max_concurrent_calls: 0,
        overview_summary: ptr::null(),
        overview_body: ptr::null(),
        overview_locale: ptr::null(),
    }
}

#[test]
fn payload_schema_parsing() {
    assert_eq!(parse_payload_schema(None).ok(), Some(None));
    assert_eq!(
        parse_payload_schema(Some(r#"{"type":"object"}"#)).ok(),
        Some(Some(serde_json::json!({ "type": "object" })))
    );
    for bad in ["{", "[1]", "1", "null"] {
        assert_eq!(parse_payload_schema(Some(bad)).err().map(|e| e.status), Some(AmStatus::InvalidSchema), "{bad}");
    }
}

#[test]
fn event_null_pointers() {
    let n = cstr("a.b");
    let mut flag = true;
    unsafe {
        assert_eq!(am_client_declare_event(ptr::null_mut(), n.as_ptr(), n.as_ptr(), ptr::null()), AmStatus::InvalidArgument);
        assert_eq!(am_client_remove_event(ptr::null_mut(), n.as_ptr(), &mut flag), AmStatus::InvalidArgument);
        assert_eq!(am_client_emit_event(ptr::null_mut(), n.as_ptr(), ptr::null(), &mut flag), AmStatus::InvalidArgument);
    }
    assert!(!flag, "出错时 *sent 为 false");
}

/// 未连接（不启动）时：声明 / 撤销 / 发出的返回值与错误码。
#[test]
fn events_without_host() {
    let (id, name, url) = (cstr("c-abi-events"), cstr("C ABI Events"), cstr("ws://127.0.0.1:1"));
    let cfg = config_for(&id, &name, &url);
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(unsafe { am_client_new(&cfg, ptr::null(), &mut client) }, AmStatus::Ok);

    let ev = cstr("order.shipped");
    let desc = cstr("订单已发货");
    let schema = cstr(r#"{"type":"object"}"#);
    let bad_name = cstr("bad name");
    let not_object = cstr("[1]");
    let payload = cstr(r#"{"orderId":"o1"}"#);
    let invalid_utf8 = [0xffu8, 0];
    let mut flag = true;
    unsafe {
        // 声明：名称 / schema / 必填参数
        assert_eq!(am_client_declare_event(client, bad_name.as_ptr(), desc.as_ptr(), ptr::null()), AmStatus::InvalidName);
        assert_eq!(am_client_declare_event(client, ev.as_ptr(), desc.as_ptr(), not_object.as_ptr()), AmStatus::InvalidSchema);
        assert_eq!(am_client_declare_event(client, ev.as_ptr(), ptr::null(), ptr::null()), AmStatus::InvalidArgument);
        assert_eq!(am_client_declare_event(client, ptr::null(), desc.as_ptr(), ptr::null()), AmStatus::InvalidArgument);

        // 未声明 → INVALID_NAME，*sent 为 false
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), ptr::null(), &mut flag), AmStatus::InvalidName);
        assert!(!flag);
        assert!(last_error().contains("order.shipped"), "{}", last_error());

        assert_eq!(am_client_declare_event(client, ev.as_ptr(), desc.as_ptr(), schema.as_ptr()), AmStatus::Ok);
        // 未连接：丢弃，返回 AM_OK 与 false；sent 可为 NULL
        flag = true;
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), payload.as_ptr(), &mut flag), AmStatus::Ok);
        assert!(!flag);
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), ptr::null(), ptr::null_mut()), AmStatus::Ok);
        // 载荷：不是对象 / 不是 JSON / 非法 UTF-8 → INVALID_JSON
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), not_object.as_ptr(), &mut flag), AmStatus::InvalidJson);
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), schema.as_ptr().add(1), &mut flag), AmStatus::InvalidJson);
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), invalid_utf8.as_ptr().cast(), &mut flag), AmStatus::InvalidJson);
        let big = cstr(&format!(r#"{{"x":"{}"}}"#, "a".repeat(9000)));
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), big.as_ptr(), &mut flag), AmStatus::InvalidJson);
        assert_eq!(am_client_emit_event(client, ptr::null(), ptr::null(), &mut flag), AmStatus::InvalidArgument);

        // 撤销：第一次 true，第二次 false；撤销后发出 → INVALID_NAME
        flag = false;
        assert_eq!(am_client_remove_event(client, ev.as_ptr(), &mut flag), AmStatus::Ok);
        assert!(flag);
        assert_eq!(am_client_remove_event(client, ev.as_ptr(), &mut flag), AmStatus::Ok);
        assert!(!flag);
        assert_eq!(am_client_remove_event(client, ev.as_ptr(), ptr::null_mut()), AmStatus::Ok);
        assert_eq!(am_client_emit_event(client, ev.as_ptr(), ptr::null(), &mut flag), AmStatus::InvalidName);

        am_client_free(client);
    }
}

/// `ship`：已连接时发出事件（带载荷 / 无载荷），以 `{"sent":[…]}` 完成。user_data 为 AmClient。
unsafe extern "C" fn emitting_tool(ud: *mut c_void, call: *mut AmCall) {
    let client = ud.cast::<AmClient>();
    let ev = cstr("order.shipped");
    let payload = cstr(r#"{"orderId":"o1"}"#);
    let mut first = false;
    let mut second = false;
    let s1 = unsafe { am_client_emit_event(client, ev.as_ptr(), payload.as_ptr(), &mut first) };
    let s2 = unsafe { am_client_emit_event(client, ev.as_ptr(), ptr::null(), &mut second) };
    let data = serde_json::json!({ "sent": [s1 == AmStatus::Ok && first, s2 == AmStatus::Ok && second] });
    let data = cstr(&data.to_string());
    let _ = unsafe { am_call_complete(call, data.as_ptr(), ptr::null(), 0) };
}

/// 端到端：握手后 events/sync（全量），调用中 emit → events/emit（eventId 由 SDK 生成，无载荷时省略 payload）。
#[test]
fn events_reach_host_through_c_abi() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let bin = app_mcp_native::test_support::fake_host_path().unwrap_or_else(|e| panic!("{e}"));
    let Ok(mut child) = Command::new(bin)
        .args(["--invoke", "ship", "--timeout-ms", "8000"])
        .stdout(Stdio::piped())
        .spawn()
    else {
        panic!("无法启动 fake_host");
    };
    let Some(stdout) = child.stdout.take() else { panic!("fake_host 没有 stdout") };
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next().and_then(Result::ok).unwrap_or_default();
    let addr = first.strip_prefix("LISTENING ").unwrap_or_default().to_owned();
    assert!(!addr.is_empty(), "LISTENING 行：{first}");

    let (id, name, url) = (cstr("c-abi-events-e2e"), cstr("C ABI Events"), cstr(&format!("ws://{addr}")));
    let cfg = config_for(&id, &name, &url);
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(unsafe { am_client_new(&cfg, ptr::null(), &mut client) }, AmStatus::Ok);
    let mut root: *mut AmScope = ptr::null_mut();
    assert_eq!(unsafe { am_client_root_scope(client, &mut root) }, AmStatus::Ok);
    let (ev, desc, schema) = (cstr("order.shipped"), cstr("订单已发货"), cstr(r#"{"type":"object"}"#));
    assert_eq!(unsafe { am_client_declare_event(client, ev.as_ptr(), desc.as_ptr(), schema.as_ptr()) }, AmStatus::Ok);

    let (tname, tdesc) = (cstr("ship"), cstr("发货"));
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: tdesc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 1,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let mut tool: *mut AmTool = ptr::null_mut();
    assert_eq!(
        unsafe { am_tool_register(root, &spec, Some(emitting_tool), client.cast(), None, &mut tool) },
        AmStatus::Ok
    );
    assert_eq!(unsafe { am_client_start(client) }, AmStatus::Ok);

    let out: Vec<serde_json::Value> =
        lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    let ok = child.wait().is_ok_and(|s| s.success());
    unsafe {
        am_tool_free(tool);
        am_scope_free(root);
        am_client_free(client);
    }
    assert!(ok, "fake_host 退出码非 0：{out:?}");
    let sync = out.iter().find(|l| l["type"] == "events").unwrap_or_else(|| panic!("没有 events/sync：{out:?}"));
    assert_eq!(
        sync["events"],
        serde_json::json!([{ "name": "order.shipped", "description": "订单已发货", "payloadSchema": { "type": "object" } }])
    );
    let emitted: Vec<&serde_json::Value> = out.iter().filter(|l| l["type"] == "event").collect();
    assert_eq!(emitted.len(), 2, "{out:?}");
    assert_eq!(emitted[0]["name"], "order.shipped");
    assert_eq!(emitted[0]["payload"], serde_json::json!({ "orderId": "o1" }));
    assert!(emitted[0]["eventId"].is_string());
    assert!(emitted[1].get("payload").is_none(), "{:?}", emitted[1]);
    assert_ne!(emitted[0]["eventId"], emitted[1]["eventId"]);
    let result = out.iter().find(|l| l["type"] == "invoke").unwrap_or_else(|| panic!("没有调用结果：{out:?}"));
    assert_eq!(result["result"], serde_json::json!({ "data": { "sent": [true, true] } }));
}
