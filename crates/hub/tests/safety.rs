//! 第 14 项（如实传递与资源保护）与第 19 项 R1–R3（结果契约）的集成测试：tokio-tungstenite 客户端扮演 App 端 SDK。
//!
//! 覆盖：工具注解 / outputSchema 原样到达 Hub API 与 MCP 出口、结果状态与摘要、无返回值、内容标注、
//! 限流（RATE_LIMITED + retryAfterMs）、参数 / 结果 / 资源大小上限（PAYLOAD_TOO_LARGE）、outputSchema 校验策略、
//! `/status` 的工具声明与拒绝计数。

use std::time::Duration;

use app_mcp_hub::{
    CallRequest, ErrorKind, Hub, HubConfig, LimitPolicy, OutputValidation, RateLimit, ResultStatus, ToolFilter,
};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;

const T: Duration = Duration::from_secs(5);

/// 假 App：注册下列工具，`tools/invoke` 按工具名回复固定结果；资源 `big` 返回 100 字节以上的内容。
async fn connect_app(hub: &Hub) {
    let url = format!("ws://{}/app", hub.listen_addr().expect("listen addr"));
    let (ws, _) = tokio_tungstenite::connect_async(url).await.expect("ws");
    let (mut sink, mut stream) = ws.split();
    let send = |v: Value| Ws::text(v.to_string());
    sink.send(send(json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "shop", "appName": "商城", "protocolVersion": "1", "sdkVersion": "0", "clientKind": "native",
        "instanceId": "i1"
    }})))
    .await
    .unwrap();
    loop {
        let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.expect("hello") else { panic!("握手失败") };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        if v["id"] == 1 {
            assert_eq!(v["result"]["status"], "paired");
            break;
        }
    }
    let schema = json!({"type": "object"});
    let tools = json!([
        {"name": "order.submit", "description": "下单", "inputSchema": schema, "risk": "payment",
         "annotations": {"idempotentHint": false, "openWorldHint": true},
         "outputSchema": {"type": "object", "properties": {"orderId": {"type": "string"}}, "required": ["orderId"]}},
        {"name": "cart.clear", "description": "清空", "inputSchema": schema},
        {"name": "list", "description": "列表", "inputSchema": schema, "risk": "read", "outputSchema": {"type": "array"}},
        {"name": "bad", "description": "结果不符合声明", "inputSchema": schema, "outputSchema": {"type": "object", "required": ["x"]}},
        {"name": "huge", "description": "大结果", "inputSchema": schema},
    ]);
    for m in [
        json!({"jsonrpc": "2.0", "method": "tools/sync", "params": {"tools": tools}}),
        json!({"jsonrpc": "2.0", "method": "resources/sync", "params": {"resources": [
            {"name": "big", "description": "大资源", "annotations": {"audience": ["user"], "priority": 0.2}}
        ]}}),
        json!({"jsonrpc": "2.0", "method": "app/ready", "params": {}}),
    ] {
        sink.send(send(m)).await.unwrap();
    }
    tokio::spawn(async move {
        while let Some(Ok(Ws::Text(t))) = stream.next().await {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            let (Some(id), Some(method)) = (v.get("id").cloned(), v["method"].as_str()) else { continue };
            let result = match (method, v["params"]["name"].as_str().unwrap_or_default()) {
                ("tools/invoke", "order.submit") => json!({
                    "data": {"orderId": "o1"}, "status": "pending", "stateResource": "order.state",
                    "summary": "已提交，等待用户在 App 内付款", "annotations": {"audience": ["assistant"]}
                }),
                ("tools/invoke", "cart.clear") => json!({"data": null}),
                ("tools/invoke", "list") => json!({"data": ["a", "b"]}),
                ("tools/invoke", "bad") => json!({"data": {"y": 1}}),
                ("tools/invoke", "huge") => json!({"data": "x".repeat(200)}),
                ("resources/read", _) => json!({"contents": {"text": "y".repeat(200)}}),
                _ => json!({}),
            };
            let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
            if sink.send(send(reply)).await.is_err() {
                break;
            }
        }
    });
    let filter = ToolFilter { apps: Some(vec!["shop".into()]), include_builtin: false, ..Default::default() };
    timeout(T, async {
        while hub.tools(&filter).len() < 5 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("工具没有出现");
}

fn config(limits: LimitPolicy, output_validation: OutputValidation) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        response_timeout: Duration::from_secs(3),
        limits,
        output_validation,
        ..Default::default()
    }
}

async fn call(hub: &Hub, name: &str, args: Value) -> app_mcp_hub::CallOutcome {
    hub.call_tool(CallRequest::new(name, args)).await.expect("call")
}

#[tokio::test]
async fn annotations_and_result_contract() {
    let hub = Hub::start(config(LimitPolicy::default(), OutputValidation::Log)).await.unwrap();
    connect_app(&hub).await;

    // S1：声明的注解逐字段优先，缺少的按 risk 推导；outputSchema 原样
    let tools = hub.tools(&ToolFilter { apps: Some(vec!["shop".into()]), ..Default::default() });
    let submit = tools.iter().find(|t| t.name == "shop.order.submit").unwrap();
    assert_eq!(
        serde_json::to_value(&submit.annotations).unwrap(),
        json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true})
    );
    assert_eq!(submit.output_schema.as_ref().unwrap()["required"], json!(["orderId"]));
    let clear = tools.iter().find(|t| t.name == "shop.cart.clear").unwrap();
    assert_eq!(serde_json::to_value(&clear.annotations).unwrap(), json!({"readOnlyHint": false}), "旧写法得到与之前相同的注解");

    // R1 + S2：状态、状态资源、摘要、内容标注
    let o = call(&hub, "shop.order.submit", json!({})).await;
    assert_eq!(o.result.unwrap(), json!({"orderId": "o1"}));
    assert_eq!(o.status, ResultStatus::Pending);
    assert_eq!(o.state_resource.as_deref(), Some("app-mcp://shop/order.state"));
    assert_eq!(o.summary.as_deref(), Some("已提交，等待用户在 App 内付款"));
    assert!(o.annotations.unwrap().audience.is_some());
    // 普通结果：done，无附加字段
    let o = call(&hub, "shop.list", json!({})).await;
    assert_eq!((o.status, o.summary), (ResultStatus::Done, None));
    assert_eq!(o.result.unwrap(), json!(["a", "b"]));

    // S2：资源标注出现在 Hub API 的资源列表
    let res = hub.resources();
    assert_eq!(res[0].annotations.as_ref().unwrap().priority, Some(0.2));

    // S5：/status 列出每个工具的声明
    let st = hub.status();
    let shop = st.apps.iter().find(|a| a.app_id == "shop").unwrap();
    let d = shop.tools.iter().find(|d| d.name == "order.submit").unwrap();
    assert_eq!(d.risk, app_mcp_hub::Risk::Payment);
    assert_eq!(d.annotations.as_ref().unwrap().destructive_hint, None, "声明原样");
    assert_eq!(d.effective.destructive_hint, Some(true));
    assert!(d.output_schema);
    assert_eq!(st.output_validation, Some(OutputValidation::Log));
    assert_eq!(st.limits.unwrap().tool_rate_per_minute, Some(120));
}

#[cfg(feature = "mcp-server")]
#[tokio::test]
async fn mcp_egress_shapes_results() {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;

    let hub = Hub::start(config(LimitPolicy::default(), OutputValidation::Log)).await.unwrap();
    connect_app(&hub).await;
    let (c, s) = tokio::io::duplex(1 << 20);
    let session = hub.mcp_session();
    tokio::spawn(async move {
        if let Ok(svc) = session.serve(s).await {
            let _ = svc.waiting().await;
        }
    });
    let client = ().serve(c).await.expect("mcp");
    let listed = client.list_all_tools().await.unwrap();
    let list = listed.iter().find(|t| t.name == "shop.list").unwrap();
    assert_eq!(
        Value::Object((*list.output_schema.clone().unwrap()).clone()),
        json!({"type": "object", "properties": {"result": {"type": "array"}}, "required": ["result"]}),
        "非对象 outputSchema 按 MCP 要求包装"
    );
    let submit = listed.iter().find(|t| t.name == "shop.order.submit").unwrap();
    assert_eq!(submit.annotations.as_ref().unwrap().open_world_hint, Some(true));

    let texts = |r: &rmcp::model::CallToolResult| -> Vec<String> {
        r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect()
    };
    let r = client.call_tool(CallToolRequestParams::new("shop.list")).await.unwrap();
    assert_eq!(r.structured_content, Some(json!({"result": ["a", "b"]})));
    // 无返回值：「已完成」，不填 structuredContent
    let r = client.call_tool(CallToolRequestParams::new("shop.cart.clear")).await.unwrap();
    assert_eq!(r.structured_content, None);
    assert!(texts(&r).last().is_some_and(|t| t == "已完成"), "{:?}", texts(&r));
    // pending：说明在 App 内容之前，_meta 带状态
    let r = client.call_tool(CallToolRequestParams::new("shop.order.submit")).await.unwrap();
    let t = texts(&r);
    let note = t.iter().position(|s| s.contains("尚未完成")).expect("状态说明");
    let summary = t.iter().position(|s| s == "已提交，等待用户在 App 内付款").expect("摘要");
    assert!(note < summary, "{t:?}");
    assert_eq!(r.meta.as_ref().unwrap().get("app-mcp/status"), Some(&json!("pending")));
    let _ = client.cancel().await;
}

#[tokio::test]
async fn rate_limits_per_tool_and_app() {
    let limits = LimitPolicy {
        tool_rate: RateLimit { per_minute: 60, burst: 1 },
        app_rate: RateLimit { per_minute: 60, burst: 2 },
        ..LimitPolicy::default()
    };
    let hub = Hub::start(config(limits, OutputValidation::Log)).await.unwrap();
    connect_app(&hub).await;
    assert!(call(&hub, "shop.list", json!({})).await.result.is_ok());
    // 同一工具第二次：工具级超限
    let e = call(&hub, "shop.list", json!({})).await.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::RateLimited);
    let d = e.details.unwrap();
    assert_eq!(d["scope"], "tool");
    assert!(d["retryAfterMs"].as_u64().is_some_and(|ms| ms > 0 && ms <= 1000), "{d}");
    // 另一工具：工具级有余量，App 级还剩 1 个
    assert!(call(&hub, "shop.cart.clear", json!({})).await.result.is_ok());
    let e = call(&hub, "shop.huge", json!({})).await.result.unwrap_err();
    assert_eq!((e.kind, e.details.unwrap()["scope"].as_str()), (ErrorKind::RateLimited, Some("app")));
    // 内置工具不限流
    assert!(call(&hub, "apps.list", json!({})).await.result.is_ok());
    let shop = hub.status().apps.into_iter().find(|a| a.app_id == "shop").unwrap();
    assert_eq!((shop.rate_limited, shop.too_large), (2, 0));
}

#[tokio::test]
async fn size_limits_are_explicit() {
    let limits = LimitPolicy {
        max_arguments_bytes: 50,
        max_result_bytes: 100,
        max_resource_bytes: 100,
        ..LimitPolicy::default()
    };
    let hub = Hub::start(config(limits, OutputValidation::Log)).await.unwrap();
    connect_app(&hub).await;
    let e = call(&hub, "shop.list", json!({"text": "z".repeat(100)})).await.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PayloadTooLarge);
    assert_eq!(e.details.unwrap()["part"], "arguments");
    let e = call(&hub, "shop.huge", json!({})).await.result.unwrap_err();
    assert_eq!((e.kind, e.details.unwrap()["part"].as_str()), (ErrorKind::PayloadTooLarge, Some("result")));
    // 小结果不受影响
    assert!(call(&hub, "shop.list", json!({})).await.result.is_ok());
    let e = hub.read_resource("app-mcp://shop/big").await.unwrap_err();
    assert_eq!(e.kind(), ErrorKind::PayloadTooLarge);
    let shop = hub.status().apps.into_iter().find(|a| a.app_id == "shop").unwrap();
    assert_eq!(shop.too_large, 3);
}

#[tokio::test]
async fn output_validation_modes() {
    // 默认 log：不符也照常返回
    let hub = Hub::start(config(LimitPolicy::default(), OutputValidation::Log)).await.unwrap();
    connect_app(&hub).await;
    assert_eq!(call(&hub, "shop.bad", json!({})).await.result.unwrap(), json!({"y": 1}));
    drop(hub);
    let hub = Hub::start(config(LimitPolicy::default(), OutputValidation::Reject)).await.unwrap();
    connect_app(&hub).await;
    let r = call(&hub, "shop.bad", json!({})).await.result;
    #[cfg(feature = "schema-validation")]
    assert_eq!(r.unwrap_err().kind, ErrorKind::HandlerError);
    #[cfg(not(feature = "schema-validation"))]
    assert!(r.is_ok(), "未启用 schema 校验时不校验");
    // 符合声明的结果不受影响
    assert!(call(&hub, "shop.order.submit", json!({})).await.result.is_ok());
}

#[tokio::test]
async fn invalid_limits_rejected_at_start() {
    let limits = LimitPolicy { tool_rate: RateLimit { per_minute: 10, burst: 0 }, ..LimitPolicy::default() };
    let e = Hub::start(config(limits, OutputValidation::Log)).await.err().expect("应拒绝");
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
}
