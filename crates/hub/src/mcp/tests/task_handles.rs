//! 第 12 项 S8 / 第 16 项 P1：任务句柄（spec/hub-api.md 3.6「任务句柄」）。

use super::*;

/// 一次 `tools/call`（可带请求 `_meta`），不断言成功。
async fn call_with(agent: &RunningService<RoleClient, ()>, name: &str, args: Value, meta: Option<Value>) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_owned());
    params.arguments = args.as_object().cloned();
    params.meta = meta.and_then(|m| m.as_object().cloned()).map(rmcp::model::RequestMetaObject::from);
    tokio::time::timeout(T, agent.peer().call_tool(params)).await.expect("MCP 调用超时").expect("call")
}

fn sc(r: &CallToolResult) -> Value {
    r.structured_content.clone().unwrap_or(Value::Null)
}

/// 工具错误的（kind, data.details.reason, message）。
fn tool_error(r: &CallToolResult) -> (Value, Value, String) {
    assert_eq!(r.is_error, Some(true), "应为工具错误：{r:?}");
    let e = sc(r)["error"].clone();
    (e["kind"].clone(), e["details"]["reason"].clone(), e["message"].as_str().unwrap_or_default().to_owned())
}

async fn begin_task(agent: &RunningService<RoleClient, ()>) -> String {
    let r = call(agent, "apps.task.begin", json!({})).await;
    let id = sc(&r)["taskId"].as_str().expect("taskId").to_owned();
    assert!(crate::task::is_task_id(&id), "{id}");
    id
}

fn handle_task(hub: &Hub, id: &str) -> Option<(Option<String>, usize)> {
    let tasks = hub.shared().agent_tasks();
    tasks
        .iter()
        .find(|(k, t)| k.is_task_handle() && t.id == id)
        .map(|(_, t)| (t.selected.get("shop").map(|s| s.instance_id.clone()), t.leases.len()))
}

/// S8 验收：一个无会话主体开两个任务句柄——各自的选择（参数通道与 `_meta` 通道等价）、租约互不影响，`apps.release` /
/// `apps.task.end` 一个不影响另一个与主体自己的任务；结束后的句柄得到可恢复的错误，再次结束是幂等的。
#[tokio::test(flavor = "multi_thread")]
async fn task_handles_isolate_selections_and_leases() {
    const OTHER: &str = "shop-2";
    let (hub, clients) = start_instances(config(Duration::ZERO, Duration::ZERO), &[INSTANCE, OTHER], false).await;
    let modern = connect_2026(&hub).await;
    let legacy = connect(&hub, false).await;

    // 列表：无会话请求列出 apps.task.* 与 taskId 参数；legacy 两者都没有（定义与句柄出现之前相同）
    let list: Value = serde_json::from_str(&tools_json(&modern).await).expect("json");
    let select_schema = |l: &Value| {
        l["tools"].as_array().into_iter().flatten().find(|t| t["name"] == json!("apps.select")).map(|t| t["inputSchema"].clone())
    };
    let names = tool_names(&list.to_string());
    assert!(names.contains(&"apps.task.begin".to_owned()) && names.contains(&"apps.task.end".to_owned()), "{names:?}");
    assert!(select_schema(&list).is_some_and(|s| s["properties"]["taskId"]["type"] == json!("string")));
    let llist: Value = serde_json::from_str(&tools_json(&legacy).await).expect("json");
    assert!(!tool_names(&llist.to_string()).iter().any(|n| n.starts_with("apps.task.")));
    assert!(select_schema(&llist).is_some_and(|s| s["properties"].get("taskId").is_none()), "{llist}");

    let a = begin_task(&modern).await;
    let b = begin_task(&modern).await;
    assert_ne!(a, b);
    // 参数通道选择 a → INSTANCE；_meta 通道选择 b → OTHER
    let r = call(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE, "taskId": a})).await;
    assert!(sc(&r)["message"].as_str().is_some_and(|m| m.contains(&a) && m.contains("工具列表不变")), "{r:?}");
    let r = call_with(&modern, "apps.select", json!({"appId": "shop", "instanceId": OTHER}), Some(json!({ crate::names::META_TASK_ID: b }))).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    assert_eq!(handle_task(&hub, &a), Some((Some(INSTANCE.to_owned()), 0)));
    assert_eq!(handle_task(&hub, &b), Some((Some(OTHER.to_owned()), 0)));
    // 主体自己的任务没有选择
    assert_eq!(principal_task(&hub).2, None);
    let selected = |r: CallToolResult| sc(&r)["apps"][0]["selectedInstanceId"].clone();
    assert_eq!(selected(call(&modern, "apps.list", json!({"taskId": a})).await), json!(INSTANCE));
    assert_eq!(selected(call(&modern, "apps.list", json!({"taskId": b})).await), json!(OTHER));
    assert_eq!(selected(call(&modern, "apps.list", json!({})).await), Value::Null);
    // App 工具只能经 _meta 出示句柄：按各自任务的选择路由；不带句柄按默认规则
    for (id, want) in [(&a, INSTANCE), (&b, OTHER)] {
        let r = call_with(&modern, "shop.cart.add", json!({}), Some(json!({ crate::names::META_TASK_ID: id }))).await;
        assert_eq!(instance_of(&r).as_deref(), Some(want), "{r:?}");
    }

    // 租约：两个任务各自持有（调用完成后的租约记在出示的任务上），主体任务没有
    assert_eq!(handle_task(&hub, &a).map(|t| t.1), Some(1));
    assert_eq!(handle_task(&hub, &b).map(|t| t.1), Some(1));
    let r = call(&modern, "apps.activate", json!({"appId": "shop", "taskId": a})).await;
    assert_eq!(sc(&r)["instanceId"], json!(INSTANCE));
    // a 释放：只收回 a 的租约
    let r = call(&modern, "apps.release", json!({"appId": "shop", "taskId": a})).await;
    assert_eq!(sc(&r)["released"], json!(1));
    assert_eq!(handle_task(&hub, &a), Some((Some(INSTANCE.to_owned()), 0)));
    assert_eq!(handle_task(&hub, &b), Some((Some(OTHER.to_owned()), 1)));
    // /status：句柄任务以 principal:local/<taskId> 出现，种类为 principal
    let st = hub.status().tasks.expect("tasks");
    assert!(st.iter().any(|t| t.id == b && t.caller == format!("principal:local/{b}") && t.kind == CallerKind::Principal), "{st:?}");

    // 结束 b：收回其租约、清除选择；a 不受影响
    let r = call(&modern, "apps.task.end", json!({"taskId": b})).await;
    assert_eq!((sc(&r)["ended"].clone(), sc(&r)["released"].clone()), (json!(true), json!(1)), "{r:?}");
    assert_eq!(handle_task(&hub, &b), None);
    assert_eq!(handle_task(&hub, &a), Some((Some(INSTANCE.to_owned()), 0)));
    // 结束后的句柄：可恢复的错误（INVALID_INPUT，reason task-expired，告诉 Agent 重新 apps.task.begin）——参数与 _meta 两条通道
    for r in [
        call_with(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE, "taskId": b}), None).await,
        call_with(&modern, "shop.cart.add", json!({}), Some(json!({ crate::names::META_TASK_ID: b }))).await,
    ] {
        let (kind, reason, msg) = tool_error(&r);
        assert_eq!((kind, reason), (json!("INVALID_INPUT"), json!("task-expired")));
        assert!(msg.contains("apps.task.begin") && msg.contains(&b), "{msg}");
    }
    assert_eq!(handle_task(&hub, &b), None, "错误路径不重建任务");
    // 再次结束：幂等
    let r = call(&modern, "apps.task.end", json!({"taskId": b})).await;
    assert_eq!(sc(&r)["ended"], json!(false));
    let _ = (modern.cancel().await, legacy.cancel().await);
    clients.iter().for_each(NativeClient::stop);
    hub.shutdown().await;
}

/// S8：句柄按 `task_idle_ttl` 空闲回收（回收后可恢复的错误、上限随之释放）；每主体句柄数上限（`RATE_LIMITED`）；
/// 参数与 `_meta` 冲突 / 格式不对 → `INVALID_INPUT`；legacy 会话出示句柄或调用 `apps.task.begin` → `INVALID_INPUT`
/// （`task-handle-unsupported`），其余行为不变；`max_task_handles = 0` 时不列出。
#[tokio::test(flavor = "multi_thread")]
async fn task_handle_expiry_cap_conflict_and_legacy() {
    let cfg = HubConfig { max_task_handles: 2, ..config(Duration::ZERO, Duration::from_millis(400)) };
    let (hub, client) = start(cfg).await;
    let modern = connect(&hub, true).await;
    let legacy = connect(&hub, false).await;
    let a = begin_task(&modern).await;
    let b = begin_task(&modern).await;
    let r = call_with(&modern, "apps.task.begin", json!({}), None).await;
    let (kind, _, msg) = tool_error(&r);
    assert_eq!(kind, json!("RATE_LIMITED"));
    assert!(msg.contains("2 个") && msg.contains("apps.task.end"), "{msg}");
    assert_eq!(sc(&r)["error"]["details"]["limit"], json!(2));

    // 冲突：参数与 _meta 不同 → INVALID_INPUT，不执行；相同 → 正常
    let meta_b = Some(json!({ crate::names::META_TASK_ID: b }));
    let r = call_with(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE, "taskId": a}), meta_b.clone()).await;
    assert_eq!(tool_error(&r).0, json!("INVALID_INPUT"));
    assert_eq!(handle_task(&hub, &a), Some((None, 0)), "冲突时不执行");
    let r = call_with(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE, "taskId": b}), meta_b.clone()).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    // 格式不对 / 类型不对
    for bad in [json!("task-123"), json!(7)] {
        let r = call_with(&modern, "apps.list", json!({"taskId": bad}), None).await;
        assert_eq!(tool_error(&r).0, json!("INVALID_INPUT"), "{bad}");
    }
    let r = call_with(&modern, "shop.cart.add", json!({}), Some(json!({ crate::names::META_TASK_ID: 7 }))).await;
    assert_eq!(tool_error(&r).0, json!("INVALID_INPUT"));

    // legacy：出示句柄（参数或 _meta）与 apps.task.begin 都被显式拒绝；不带句柄照旧
    for r in [
        call_with(&legacy, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE, "taskId": a}), None).await,
        call_with(&legacy, "shop.cart.add", json!({}), Some(json!({ crate::names::META_TASK_ID: a }))).await,
        call_with(&legacy, "apps.task.begin", json!({}), None).await,
    ] {
        let (kind, reason, _) = tool_error(&r);
        assert_eq!((kind, reason), (json!("INVALID_INPUT"), json!("task-handle-unsupported")));
    }
    call(&legacy, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
    assert_eq!(legacy_keys(&hub).len(), 1);

    // 空闲回收：两个句柄任务都被回收；之后出示 → 可恢复错误；上限释放，可再签发
    eventually("句柄任务空闲回收", || handle_task(&hub, &a).is_none() && handle_task(&hub, &b).is_none()).await;
    let (kind, reason, msg) = tool_error(&call_with(&modern, "apps.list", json!({"taskId": a}), None).await);
    assert_eq!((kind, reason), (json!("INVALID_INPUT"), json!("task-expired")));
    assert!(msg.contains("0.4 秒") && msg.contains("apps.task.begin"), "{msg}");
    let c = begin_task(&modern).await;
    assert!(handle_task(&hub, &c).is_some());
    let _ = (modern.cancel().await, legacy.cancel().await);
    client.stop();
    hub.shutdown().await;

    // max_task_handles = 0：不列出，签发与出示都被拒绝
    let (hub, client) = start(HubConfig { max_task_handles: 0, ..config(Duration::ZERO, Duration::ZERO) }).await;
    let modern = connect(&hub, true).await;
    assert!(!tool_list(&modern).await.iter().any(|n| n.starts_with("apps.task.")));
    let (kind, reason, _) = tool_error(&call_with(&modern, "apps.task.begin", json!({}), None).await);
    assert_eq!((kind, reason), (json!("INVALID_INPUT"), json!("task-handle-unsupported")));
    let (_, reason, _) = tool_error(&call_with(&modern, "apps.list", json!({"taskId": c}), None).await);
    assert_eq!(reason, json!("task-handle-unsupported"));
    let _ = modern.cancel().await;
    client.stop();
    hub.shutdown().await;
}
