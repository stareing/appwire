//! 第 12 项 S7：MCP 2026-07-28 协商、subscriptions/listen、resultType、回退开关。

use super::*;

const BOARD_STATE: &str = "app-mcp://board/state";

struct Board;
impl app_mcp_native::ResourceReader for Board {
    fn read(&self, read: app_mcp_native::ReadHandle) {
        let _ = match read.resource_name().as_str() {
            "broken" => read.fail(ErrorKind::HandlerError, "读取失败"),
            _ => read.complete(&json!({ "items": [1, 2] }).to_string()),
        };
    }
}

/// 另一个 App（`board`：工具 `board.note`，资源 `state` 与读取总是失败的 `broken`），用于 App 上下线与资源变化。
async fn start_board(hub: &Hub) -> (NativeClient, app_mcp_native::ResourceHandle) {
    let mut c = NativeConfig::new("board", "看板");
    c.host_url = format!("ws://{}/app", hub.listen_addr().expect("listen"));
    c.instance_id = Some("board-1".into());
    c.lifecycle.mode = LifecycleMode::Persistent;
    let client = NativeClient::new(c, None).expect("client");
    client.register_tool(ToolSpec::new("note", "记一笔"), std::sync::Arc::new(Echo)).expect("tool");
    let spec = |name: &str| app_mcp_native::ResourceSpec { name: name.into(), description: name.into(), mime_type: None };
    let state = client.register_resource(spec("state"), std::sync::Arc::new(Board)).expect("resource");
    client.register_resource(spec("broken"), std::sync::Arc::new(Board)).expect("resource");
    client.start();
    eventually("board 注册工具与资源", || {
        hub.status().apps.iter().any(|a| a.app_id == "board" && a.instances.iter().any(|i| i.state == InstanceState::Connected))
            && hub.resources().len() == 2
    })
    .await;
    (client, state)
}

/// 等下一条通知（带超时）；同时断言它带本订阅的 `subscriptionId`。
async fn next_notification(sub: &mut Subscription) -> Option<(String, Value)> {
    let n = tokio::time::timeout(T, sub.next()).await.expect("等待 listen 通知超时").expect("listen 流出错")?;
    assert_eq!(n.get_meta().subscription_id().as_ref(), Some(sub.id()), "通知带 subscriptionId");
    let v = serde_json::to_value(&n).expect("json");
    Some((v["method"].as_str().unwrap_or_default().to_owned(), v))
}

/// 读到指定方法的通知为止（中间的其他通知跳过）。
async fn wait_notification(sub: &mut Subscription, method: &str) -> Value {
    loop {
        match next_notification(sub).await {
            Some((m, v)) if m == method => return v,
            Some(_) => continue,
            None => panic!("listen 流在收到 {method} 前结束：{:?}", sub.end()),
        }
    }
}

fn listen_streams(hub: &Hub) -> Option<usize> {
    hub.status().mcp_listen_streams
}

/// 默认（`Auto`）：2026-07-28 客户端经 `server/discover` 协商成功，`supportedVersions` 含两代；`initialize` 仍协商 2025-11-25
/// 且结果不带 `resultType`。modern 的列表 / 调用 / 资源读取结果都带 `resultType: complete`；modern 的 JSON-RPC 错误不用 AppWire 的
/// -32000…-32019 码（资源不存在 -32602、App 读取失败 -32603，类别在 `data.kind`），legacy 不变。
#[tokio::test(flavor = "multi_thread")]
async fn modern_2026_negotiation_results_and_error_codes() {
    let (hub, shop) = start(config(Duration::ZERO, Duration::ZERO)).await;
    let (board, _state) = start_board(&hub).await;
    let modern = connect_2026(&hub).await;
    let legacy = connect(&hub, false).await;
    assert_eq!(legacy.peer().peer_info().map(|i| i.protocol_version.clone()), Some(ProtocolVersion::V_2025_11_25));
    let discovered = modern.peer().discover(rmcp::model::RequestMetaObject::with_client_context(
        ProtocolVersion::V_2026_07_28,
        rmcp::model::Implementation::new("t", "1"),
        rmcp::model::ClientCapabilities::default(),
    ));
    let supported = discovered.await.expect("server/discover").supported_versions;
    assert!(supported.contains(&ProtocolVersion::V_2026_07_28) && supported.contains(&ProtocolVersion::V_2025_06_18), "{supported:?}");

    // resultType：modern 每种结果都带 complete；legacy 不带
    let list: Value = serde_json::from_str(&tools_json(&modern).await).expect("json");
    assert_eq!(list["resultType"], json!("complete"), "{list}");
    assert!(tool_names(&list.to_string()).contains(&"board.note".to_owned()));
    let r = serde_json::to_value(call(&modern, "board.note", json!({})).await).expect("json");
    assert_eq!(r["resultType"], json!("complete"), "{r}");
    let read = modern.peer().read_resource(ReadResourceRequestParams::new(BOARD_STATE)).await.expect("resources/read");
    assert_eq!(serde_json::to_value(&read).expect("json")["resultType"], json!("complete"));
    let res = serde_json::to_value(modern.peer().list_resources(None).await.expect("resources/list")).expect("json");
    assert_eq!(res["resultType"], json!("complete"));
    let l = tools_json(&legacy).await;
    assert!(!l.contains("resultType"), "{l}");
    let lr = serde_json::to_value(call(&legacy, "board.note", json!({})).await).expect("json");
    assert!(lr.get("resultType").is_none(), "{lr}");

    // 错误码：modern 用规范码 + data.kind，legacy 保持原码
    let code_of = |e: ServiceError| match e {
        ServiceError::McpError(e) => (e.code, e.data.and_then(|d| d.get("kind").cloned())),
        other => panic!("不是 JSON-RPC 错误：{other:?}"),
    };
    let read_err = |agent: &RunningService<RoleClient, ()>, uri: &'static str| {
        let peer = agent.peer().clone();
        async move { code_of(peer.read_resource(ReadResourceRequestParams::new(uri)).await.expect_err(uri)) }
    };
    assert_eq!(read_err(&modern, "app-mcp://nope/x").await, (ErrorCode::INVALID_PARAMS, Some(json!("RESOURCE_NOT_FOUND"))));
    assert_eq!(read_err(&legacy, "app-mcp://nope/x").await, (ErrorCode::RESOURCE_NOT_FOUND, Some(json!("RESOURCE_NOT_FOUND"))));
    assert_eq!(read_err(&modern, "app-mcp://board/broken").await, (ErrorCode::INTERNAL_ERROR, Some(json!("HANDLER_ERROR"))));
    let handler_code = ErrorCode(ErrorKind::HandlerError.code() as i32);
    assert_eq!(read_err(&legacy, "app-mcp://board/broken").await, (handler_code, Some(json!("HANDLER_ERROR"))));

    // 无会话：不登记会话
    assert_eq!(hub.status().mcp_sessions, 1, "只有 legacy 会话");
    let _ = (modern.cancel().await, legacy.cancel().await);
    board.stop();
    shop.stop();
    hub.shutdown().await;
}

/// `subscriptions/listen`：确认只含接受的类别与可订阅的资源；App 上线 / 资源变化 / App 下线依次在流上收到
/// `tools/list_changed`、`resources/updated`（带 `subscriptionId`），列表随之变化；客户端关闭流后订阅方与资源订阅移除；
/// Hub 停止时流以最终结果正常结束。legacy 会话照旧经会话收 `list_changed`。
#[tokio::test(flavor = "multi_thread")]
async fn modern_listen_delivers_list_changes_and_resource_updates() {
    let (hub, shop) = start(config(Duration::ZERO, Duration::ZERO)).await;
    let modern = connect_2026(&hub).await;
    let filter = SubscriptionFilter::builder()
        .tools_list_changed()
        .resources_list_changed()
        .resource_subscriptions([BOARD_STATE, "https://example.com/x", "app-mcp://"])
        .build();
    let mut sub = modern.peer().listen(filter).await.expect("subscriptions/listen");
    let ack = sub.acknowledged().clone();
    assert_eq!((ack.tools_list_changed, ack.resources_list_changed), (Some(true), Some(true)));
    assert_eq!(ack.resource_subscriptions, Some(vec![BOARD_STATE.to_owned()]), "只接受本 Hub 的资源 URI");
    assert_eq!(listen_streams(&hub), Some(1));
    assert_eq!(hub.status().mcp_sessions, 0, "listen 流不是会话");
    assert!(!tool_list(&modern).await.contains(&"board.note".to_owned()));

    // App 上线：工具列表变化经 listen 流送达，之后的 tools/list 含新工具
    let (board, state) = start_board(&hub).await;
    wait_notification(&mut sub, "notifications/tools/list_changed").await;
    assert!(tool_list(&modern).await.contains(&"board.note".to_owned()));
    // 资源变化：Hub 已向 App 订阅（listen 流接受的 URI），变化经流送达
    eventually("Hub 向 App 订阅了资源", || {
        hub.shared().registry().resource_holders("board", "state").iter().any(|(_, s)| *s)
    })
    .await;
    state.notify_changed().expect("notify");
    let updated = wait_notification(&mut sub, "notifications/resources/updated").await;
    assert_eq!(updated["params"]["uri"], json!(BOARD_STATE), "{updated}");
    // App 下线：再收到列表变化，列表中不再有其工具
    board.stop();
    wait_notification(&mut sub, "notifications/tools/list_changed").await;
    eventually("board 工具移除", || hub.status().apps.iter().all(|a| a.app_id != "board" || a.tools.is_empty())).await;
    assert!(!tool_list(&modern).await.contains(&"board.note".to_owned()));

    // 客户端关闭流：订阅方与资源订阅移除
    sub.cancel().await.expect("cancel");
    eventually("listen 流移除", || listen_streams(&hub) == Some(0)).await;
    eventually("资源订阅移除", || hub.shared().has_no_resource_subscriptions()).await;

    // Hub 停止：流以最终结果（SubscriptionsListenResult）结束
    let mut sub = modern.peer().listen(SubscriptionFilter::builder().tools_list_changed().build()).await.expect("listen");
    assert_eq!(listen_streams(&hub), Some(1));
    shop.stop();
    hub.shutdown().await;
    assert!(next_notification(&mut sub).await.is_none());
    assert!(matches!(sub.end(), Some(SubscriptionEnd::Graceful(_))), "{:?}", sub.end());
    let _ = modern.cancel().await;
}

/// 每个主体的 listen 流数有上限（B-07）：超出的 listen 在确认后以 `RATE_LIMITED`（modern 码 -32603）错误结束，关掉一个后可再开；
/// `max_listen_streams = 0` 时不提供 listen（method not found）。legacy 会话没有 listen。
#[tokio::test(flavor = "multi_thread")]
async fn listen_streams_are_bounded_per_principal() {
    let cfg = HubConfig { max_listen_streams: 2, ..config(Duration::ZERO, Duration::ZERO) };
    let (hub, shop) = start(cfg).await;
    let a = connect_2026(&hub).await;
    let b = connect_2026(&hub).await;
    let tools = || SubscriptionFilter::builder().tools_list_changed().build();
    let mut first = a.peer().listen(tools()).await.expect("第 1 个");
    let _second = b.peer().listen(tools()).await.expect("第 2 个");
    // 确认在 Hub 的 listen 处理之前由 rmcp 发出，因此超限表现为：确认之后流以 JSON-RPC 错误结束
    let mut third = a.peer().listen(tools()).await.expect("确认");
    let refused = match tokio::time::timeout(T, third.next()).await.expect("超时") {
        Err(ServiceError::McpError(e)) => e,
        other => panic!("第 3 个应被拒绝：{other:?}"),
    };
    assert_eq!(refused.code, ErrorCode::INTERNAL_ERROR, "{refused:?}");
    assert_eq!(refused.data.as_ref().and_then(|d| d.get("kind")), Some(&json!("RATE_LIMITED")));
    assert_eq!(listen_streams(&hub), Some(2));
    first.cancel().await.expect("cancel");
    eventually("关闭后计数下降", || listen_streams(&hub) == Some(1)).await;
    let _third = a.peer().listen(tools()).await.expect("关闭一个后可再开");

    // legacy 会话：listen 不可用（rmcp 按 legacy 请求返回 method not found）
    let legacy = connect(&hub, false).await;
    match legacy.peer().listen(tools()).await {
        Err(ServiceError::McpError(e)) => assert_eq!(e.code, ErrorCode::METHOD_NOT_FOUND),
        other => panic!("legacy 会话不应能 listen：{other:?}"),
    }
    let _ = (a.cancel().await, b.cancel().await, legacy.cancel().await);
    shop.stop();
    hub.shutdown().await;

    let (hub, shop) = start(HubConfig { max_listen_streams: 0, ..config(Duration::ZERO, Duration::ZERO) }).await;
    let c = connect_2026(&hub).await;
    match c.peer().listen(tools()).await {
        Err(ServiceError::McpError(e)) => assert_eq!(e.code, ErrorCode::METHOD_NOT_FOUND),
        other => panic!("max_listen_streams = 0 不提供 listen：{other:?}"),
    }
    let _ = c.cancel().await;
    shop.stop();
    hub.shutdown().await;
}

/// 回退开关 `LegacyOnly`：只声明到 2025-11-25（2026-07-28 客户端协商失败，`supported` 不含 2026-07-28），`initialize` 照旧，
/// 以旧版本经 `server/discover` 的无会话请求照旧可用但没有 listen（S7 之前的行为）。
#[tokio::test(flavor = "multi_thread")]
async fn legacy_only_switch_restores_legacy_negotiation() {
    let cfg = HubConfig { mcp_protocol_mode: McpProtocolMode::LegacyOnly, ..config(Duration::ZERO, Duration::ZERO) };
    let (hub, shop) = start(cfg).await;
    match connect_with(&hub, ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] }).await.map_err(|e| *e) {
        Err(ClientInitializeError::NoCompatibleProtocolVersion { server_supported, .. }) => {
            assert_eq!(server_supported, ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25).to_vec());
        }
        Err(other) => panic!("应以 -32022 拒绝 2026-07-28：{other:?}"),
        Ok(_) => panic!("回退开关下不应协商出 2026-07-28"),
    }
    let legacy = connect(&hub, false).await;
    assert_eq!(legacy.peer().peer_info().map(|i| i.protocol_version.clone()), Some(ProtocolVersion::V_2025_11_25));
    call(&legacy, "shop.cart.add", json!({})).await;
    let old_discover = connect(&hub, true).await;
    call(&old_discover, "shop.cart.add", json!({})).await;
    match old_discover.peer().listen(SubscriptionFilter::builder().tools_list_changed().build()).await {
        Err(ServiceError::McpError(e)) => assert_eq!(e.code, ErrorCode::METHOD_NOT_FOUND),
        other => panic!("回退开关下没有 listen：{other:?}"),
    }
    let _ = (legacy.cancel().await, old_discover.cancel().await);
    shop.stop();
    hub.shutdown().await;
}
