use std::time::Duration;

use app_mcp_protocol::{ErrorKind, Risk, ToolError};
use serde_json::Value;
use serde_json::json;

use super::*;

#[test]
fn approval_threshold() {
    let p = ApprovalPolicy {
        require_at_or_above: Some(Risk::Destructive),
        timeout: None,
    };
    assert!(!p.requires(Risk::Read));
    assert!(!p.requires(Risk::Write));
    assert!(p.requires(Risk::Destructive));
    assert!(p.requires(Risk::Payment));
    assert!(p.requires(Risk::OsSensitive));
    assert!(!ApprovalPolicy::default().requires(Risk::OsSensitive));
}

#[test]
fn json_shapes() {
    let e = HubEvent::AppConnected {
        app_id: "shop".into(),
        instance_id: "i1".into(),
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({"type": "appConnected", "appId": "shop", "instanceId": "i1"})
    );
    assert_eq!(
        serde_json::to_value(HubEvent::ToolsChanged).unwrap(),
        json!({"type": "toolsChanged"})
    );
    assert_eq!(
        serde_json::to_value(HubEvent::AppDormant { app_id: "a".into(), instance_id: "i".into() }).unwrap(),
        json!({"type": "appDormant", "appId": "a", "instanceId": "i"})
    );
    assert_eq!(
        serde_json::to_value(HubEvent::AppWaking { app_id: "a".into(), instance_id: None }).unwrap(),
        json!({"type": "appWaking", "appId": "a", "instanceId": null})
    );
    assert_eq!(serde_json::to_value(Availability::Dormant).unwrap(), json!("dormant"));
    let r: CallRequest =
        serde_json::from_value(json!({"name": "a.b", "timeout": 1500, "session": "s"}))
            .unwrap();
    assert_eq!(r.timeout, Some(Duration::from_millis(1500)));
    assert_eq!(r.arguments, Value::Null);
    let f: ToolFilter = serde_json::from_value(json!({"maxRisk": "write"})).unwrap();
    assert!(f.include_builtin);
    assert_eq!(f.max_risk, Some(Risk::Write));
    assert_eq!(f.session, None);
    let f: ToolFilter = serde_json::from_value(json!({"session": "s"})).unwrap();
    assert_eq!(f.session.as_deref(), Some("s"));
    assert_eq!(serde_json::to_value(ToolExposure::Progressive).unwrap(), json!("progressive"));
    assert_eq!(serde_json::to_value(McpProtocolMode::LegacyOnly).unwrap(), json!("legacyOnly"));
    assert_eq!(serde_json::from_value::<McpProtocolMode>(json!("auto")).unwrap(), McpProtocolMode::Auto);
    assert_eq!(McpProtocolMode::default(), McpProtocolMode::Auto);
    assert_eq!(serde_json::from_value::<ToolExposure>(json!("all")).unwrap(), ToolExposure::All);
    assert_eq!(ToolExposure::default(), ToolExposure::Auto);
    let o = CallOutcome {
        call_id: "c".into(),
        result: Err(ToolError::new(ErrorKind::UserRejected, "不")),
        state_hints: vec![],
        instance_id: None,
        overview: None,
        annotations: None,
        state_resource: None,
        status: Default::default(),
        summary: None,
        routed_to: None,
        duration_ms: 5,
        woke: true,
    };
    let v = serde_json::to_value(&o).unwrap();
    assert_eq!((v["durationMs"].clone(), v["woke"].clone()), (json!(5), json!(true)));
    assert_eq!(v["result"]["error"]["kind"], "USER_REJECTED");
    assert!(v.get("routedTo").is_none(), "未改调时不序列化");
    let back: CallOutcome = serde_json::from_value(v).unwrap();
    assert_eq!(back, o);
}
