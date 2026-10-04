//! 休眠记录持久化（spec/hub-api.md 3.5「持久化」）：Hub 重启后按休眠实例列出并可唤醒。

use super::*;

/// Host 退出时仍在线、声明了唤醒描述的实例：就绪后即写出记录（不等关闭时写），Hub 重启后按休眠实例列出并可唤醒。
/// 回归（Windows 实测 e2）：App 在 Host 退出时在线、之后自行转休眠，重启的 Host 不认识它（TOOL_NOT_FOUND）。
#[tokio::test(flavor = "multi_thread")]
async fn online_instance_survives_hub_restart() {
    let n: u64 = rand::random();
    let state = std::env::temp_dir().join(format!("app-mcp-hub-state-o-{}-{n:x}", std::process::id()));
    let hub = Hub::start(HubConfig { state_dir: Some(state.clone()), ..config(0) }).await.unwrap();
    let client = native_client_with(native_config(&hub, "calc-o", 60_000));
    client.start();
    wait_status(&client, StateStatus::Connected).await;
    let file = state.join("dormant").join("calc.json");
    eventually("在线实例就绪后写出记录", || {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("\"calc-o\""))
    })
    .await;
    // 退出前停止 App：重启后只能经读回的记录唤醒
    client.stop();
    eventually("断开后删除记录", || !file.exists()).await;
    let saved = {
        // 断开会删掉记录；重新连接一次，在线时就把文件复制出来，模拟 Host 异常退出时磁盘上的内容。
        let client = native_client_with(native_config(&hub, "calc-o", 60_000));
        client.start();
        wait_status(&client, StateStatus::Connected).await;
        eventually("再次写出", || std::fs::read_to_string(&file).is_ok_and(|t| t.contains("\"calc-o\""))).await;
        let saved = std::fs::read(&file).unwrap();
        client.stop();
        saved
    };
    hub.shutdown().await;
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, saved).unwrap();

    let hub = Hub::start(HubConfig { state_dir: Some(state.clone()), ..config(0) }).await.unwrap();
    assert_eq!(availability(&hub, "calc.math.add"), Some(Availability::Dormant));
    let waker = Arc::new(FakeWaker { fail: true, ..Default::default() });
    hub.set_waker(waker.clone());
    let out = hub.call_tool(CallRequest::new("calc.math.add", json!({"a": 1, "b": 2}))).await.unwrap();
    assert_eq!(out.result.unwrap_err().kind, ErrorKind::LaunchFailed, "经唤醒而非 TOOL_NOT_FOUND");
    let reqs = waker.requests.lock().unwrap().clone();
    assert_eq!((reqs.len(), reqs[0].instance_id.as_deref()), (1, Some("calc-o")));
    assert_eq!(reqs[0].descriptor.target.as_deref(), Some("calc-app"), "握手时上报的唤醒描述");
    hub.shutdown().await;
    let _ = std::fs::remove_dir_all(&state);
}

/// Hub 重启（同一 `state_dir`）后，重启前休眠的实例仍列出、可唤醒，且用读回的恢复令牌快速恢复。
/// 回归（Windows 实测）：重启后调用休眠 App 的工具返回 TOOL_NOT_FOUND。
#[tokio::test(flavor = "multi_thread")]
async fn dormant_records_survive_hub_restart() {
    let n: u64 = rand::random();
    let state = std::env::temp_dir().join(format!("app-mcp-hub-state-{}-{n:x}", std::process::id()));
    // App 经固定的本地 IPC 端点连接：重启后的 Hub 在同一端点监听（TCP 端口 0 每次不同）。
    #[cfg(unix)]
    let ipc = format!("unix:{}", std::env::temp_dir().join(format!("app-mcp-p-{n:x}")).join("hub.sock").display());
    #[cfg(windows)]
    let ipc = format!(r"pipe:\\.\pipe\app-mcp-p-{n:x}");
    let cfg = || HubConfig { state_dir: Some(state.clone()), ipc_endpoint: Some(ipc.clone()), ..config(0) };

    let hub = Hub::start(cfg()).await.unwrap();
    let mut rx = hub.events();
    let mut c = native_config(&hub, "calc-p", 150);
    c.host_url = ipc.clone();
    let client = native_client_with(c);
    client.start();
    next_event(&mut rx, |e| matches!(e, HubEvent::AppDormant { .. })).await;
    wait_status(&client, StateStatus::Dormant).await;
    let file = state.join("dormant").join("calc.json");
    eventually("写出休眠记录", || file.exists()).await;
    let st = hub.status().dormant_store.expect("配置了 state_dir");
    assert!(st.writes >= 1 && st.issues.is_empty(), "{st:?}");
    hub.shutdown().await;

    // 重启：读回，工具列为 Dormant；调用经唤醒带回同一实例
    let hub = Hub::start(cfg()).await.unwrap();
    assert_eq!(availability(&hub, "calc.math.add"), Some(Availability::Dormant));
    let app = hub.apps().into_iter().find(|a| a.app_id == "calc").expect("读回的 App");
    assert_eq!(app.dormant_instances.len(), 1);
    assert_eq!(hub.status().dormant_store.unwrap().loaded_instances, 1);
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    *waker.client.lock().unwrap() = Some(client.clone());
    let mut rx = hub.events();
    let out = hub.call_tool(CallRequest::new("calc.math.add", json!({"a": 40, "b": 2}))).await.unwrap();
    assert_eq!(out.result.unwrap()["sum"], 42);
    assert_eq!(out.instance_id.as_deref(), Some("calc-p"));
    let reqs = waker.requests.lock().unwrap().clone();
    assert_eq!((reqs.len(), reqs[0].instance_id.as_deref()), (1, Some("calc-p")));
    assert_eq!(reqs[0].descriptor.target.as_deref(), Some("calc-app"), "读回的唤醒描述");
    // 回连后按在线实例重写；再次休眠 → 按休眠记录重写
    next_event(&mut rx, |e| matches!(e, HubEvent::AppDormant { .. })).await;
    eventually("再次写出休眠记录", || file.exists()).await;

    client.stop();
    hub.shutdown().await;
    // 未配置 state_dir：不读写文件
    let hub = Hub::start(config(0)).await.unwrap();
    assert_eq!(availability(&hub, "calc.math.add"), None);
    assert!(hub.status().dormant_store.is_none());
    hub.shutdown().await;
    let _ = std::fs::remove_dir_all(&state);
    #[cfg(unix)]
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("app-mcp-p-{n:x}")));
}
