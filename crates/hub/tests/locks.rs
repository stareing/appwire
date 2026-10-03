//! 第 16 项 N6 对象锁（spec/hub-api.md 3.6「对象锁」）：经 Hub API 的两个会话（各自一个 Agent 任务）验证——
//!
//! - App 锁拦截其他持有者的写调用（`LOCKED`，未转发、不计用量），只读工具与持有者自己的调用照常；
//! - 命名锁只与同名加锁冲突、不拦截调用；续期、解锁（不能释放他人的锁）、持有者任务结束即释放；
//! - 每持有者上限、`max_locks = 0` 关闭、未知 App；`/status` 的 `locks` 不含任务以外的凭据。
//!
//! 所有 TCP 监听都绑定端口 0。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app_mcp_hub::{CallOutcome, CallRequest, ErrorKind, Hub, HubConfig, ToolFilter};
use app_mcp_native::{CallHandle, LifecycleMode, NativeClient, NativeConfig, Risk, ToolHandler, ToolSpec};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

/// 记录被转发的调用次数。
struct Counter(Arc<AtomicUsize>);

impl ToolHandler for Counter {
    fn invoke(&self, call: CallHandle) {
        self.0.fetch_add(1, Ordering::SeqCst);
        let _ = call.complete(Some(r#"{"ok":true}"#), vec![]);
    }
}

fn config(max_locks: usize) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        max_locks,
        ..Default::default()
    }
}

/// 在线的文档 App：`doc.edit`（写）与 `doc.read`（只读），返回被转发的调用计数。
async fn start_docs(hub: &Hub) -> (NativeClient, Arc<AtomicUsize>) {
    let mut c = NativeConfig::new("docs", "文档");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.lifecycle.mode = LifecycleMode::Persistent;
    let app = NativeClient::new(c, None).expect("client");
    let count = Arc::new(AtomicUsize::new(0));
    let mut read = ToolSpec::new("doc.read", "读文档");
    read.risk = Risk::Read;
    app.register_tool(ToolSpec::new("doc.edit", "改文档"), Arc::new(Counter(count.clone()))).expect("tool");
    app.register_tool(read, Arc::new(Counter(count.clone()))).expect("tool");
    app.start();
    let deadline = std::time::Instant::now() + T;
    while !hub.status().apps.iter().any(|a| a.app_id == "docs" && a.tools.len() == 2) {
        assert!(std::time::Instant::now() < deadline, "App 未注册工具");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (app, count)
}

async fn call(hub: &Hub, session: &str, name: &str, args: Value) -> CallOutcome {
    let mut req = CallRequest::new(name, args);
    req.session = Some(session.into());
    hub.call_tool(req).await.unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

/// 调用结果：成功时为数据，失败时为 (类别, details)。
async fn outcome(hub: &Hub, session: &str, name: &str, args: Value) -> Result<Value, (ErrorKind, Value)> {
    call(hub, session, name, args).await.result.map_err(|e| (e.kind, e.details.unwrap_or_default()))
}

fn kind<T>(r: &Result<T, (ErrorKind, Value)>) -> Option<ErrorKind> {
    r.as_ref().err().map(|(k, _)| *k)
}

#[tokio::test(flavor = "multi_thread")]
async fn app_lock_blocks_other_writers_only() {
    let hub = Hub::start(config(16)).await.expect("hub");
    let (app, forwarded) = start_docs(&hub).await;

    let r = outcome(&hub, "a", "apps.lock", json!({"appId": "docs", "ttlMs": 30_000})).await.expect("加锁");
    assert_eq!((r["renewed"].as_bool(), r["ttlMs"].as_u64()), (Some(false), Some(30_000)), "{r}");

    // 其他持有者的写调用：LOCKED，未转发、不计用量
    let before = forwarded.load(Ordering::SeqCst);
    let (k, d) = outcome(&hub, "b", "docs.doc.edit", json!({})).await.expect_err("应被锁");
    assert_eq!(k, ErrorKind::Locked);
    assert_eq!(d["holder"], "api", "{d}");
    assert!(d["retryAfterMs"].as_u64().is_some_and(|ms| ms > 0 && ms <= 30_000), "{d}");
    assert_eq!(forwarded.load(Ordering::SeqCst), before, "LOCKED 的调用不转发");
    // 只读工具与持有者自己的写调用照常
    assert!(outcome(&hub, "b", "docs.doc.read", json!({})).await.is_ok());
    assert!(outcome(&hub, "a", "docs.doc.edit", json!({})).await.is_ok());
    // 他人加同一把锁 → LOCKED；解锁他人的锁 → released: false
    assert_eq!(kind(&outcome(&hub, "b", "apps.lock", json!({"appId": "docs"})).await), Some(ErrorKind::Locked));
    let r = outcome(&hub, "b", "apps.unlock", json!({"appId": "docs"})).await.expect("解锁");
    assert_eq!(r["released"], false, "{r}");

    let locks = hub.status().locks.expect("locks");
    assert_eq!(locks.len(), 1, "{locks:?}");
    assert_eq!((locks[0].app_id.as_str(), locks[0].caller.as_str(), locks[0].holder.as_str()), ("docs", "api:a", "api"));
    // 用量：b 被锁住的那次不计调用（只计 doc.read 一次）
    let usage = hub.status().usage.expect("usage");
    let api = usage.iter().find(|u| u.subject == "api").expect("api");
    assert_eq!(api.total.calls, 2, "a 的 doc.edit 与 b 的 doc.read：{api:?}");

    // 续期；解锁后他人可写
    let r = outcome(&hub, "a", "apps.lock", json!({"appId": "docs"})).await.expect("续期");
    assert_eq!(r["renewed"], true);
    let r = outcome(&hub, "a", "apps.unlock", json!({"appId": "docs"})).await.expect("解锁");
    assert_eq!(r["released"], true);
    assert!(outcome(&hub, "b", "docs.doc.edit", json!({})).await.is_ok());
    assert!(hub.status().locks.expect("locks").is_empty());
    app.stop();
    hub.shutdown().await;
}

/// 命名锁只与同名加锁冲突，不拦截调用；持有者的任务结束（`reset_session`）即释放其全部锁。
#[tokio::test(flavor = "multi_thread")]
async fn named_locks_and_release_on_task_end() {
    let hub = Hub::start(config(16)).await.expect("hub");
    let (app, _) = start_docs(&hub).await;

    outcome(&hub, "a", "apps.lock", json!({"appId": "docs", "key": "doc-1"})).await.expect("命名锁");
    assert!(outcome(&hub, "b", "docs.doc.edit", json!({})).await.is_ok(), "命名锁不拦截调用");
    let (k, d) = outcome(&hub, "b", "apps.lock", json!({"appId": "docs", "key": "doc-1"})).await.expect_err("同名冲突");
    assert_eq!((k, d["key"].as_str()), (ErrorKind::Locked, Some("doc-1")));
    assert!(outcome(&hub, "b", "apps.lock", json!({"appId": "docs", "key": "doc-2"})).await.is_ok(), "不同对象互不冲突");
    assert!(outcome(&hub, "b", "apps.lock", json!({"appId": "docs"})).await.is_ok(), "App 锁与命名锁互不冲突");
    assert_eq!(kind(&outcome(&hub, "a", "docs.doc.edit", json!({})).await), Some(ErrorKind::Locked));

    hub.reset_session(Some("b"));
    assert!(outcome(&hub, "a", "docs.doc.edit", json!({})).await.is_ok(), "持有者任务结束即释放 App 锁");
    assert!(outcome(&hub, "a", "apps.lock", json!({"appId": "docs", "key": "doc-2"})).await.is_ok(), "命名锁一并释放");
    app.stop();
    hub.shutdown().await;
}

/// 每持有者上限（`RATE_LIMITED`，续期不计新锁）、参数与 appId 校验；`max_locks = 0` 关闭（不列出、不可调用、不拦截）。
#[tokio::test(flavor = "multi_thread")]
async fn limits_validation_and_disabled() {
    let hub = Hub::start(config(1)).await.expect("hub");
    let (app, _) = start_docs(&hub).await;
    outcome(&hub, "a", "apps.lock", json!({"appId": "docs", "key": "x"})).await.expect("第一把");
    let (k, d) = outcome(&hub, "a", "apps.lock", json!({"appId": "docs", "key": "y"})).await.expect_err("超限");
    assert_eq!((k, d["limit"].as_u64()), (ErrorKind::RateLimited, Some(1)));
    assert!(outcome(&hub, "a", "apps.lock", json!({"appId": "docs", "key": "x"})).await.is_ok(), "续期不计新锁");
    for args in [json!({"appId": "nope"}), json!({"appId": "apps"})] {
        assert_eq!(kind(&outcome(&hub, "a", "apps.lock", args).await), Some(ErrorKind::ToolNotFound));
    }
    for args in [json!({"appId": "docs", "ttlMs": 10}), json!({"appId": "docs", "key": ""})] {
        assert_eq!(kind(&outcome(&hub, "a", "apps.lock", args).await), Some(ErrorKind::InvalidInput));
    }
    let names: Vec<String> = hub.tools(&ToolFilter::default()).into_iter().map(|t| t.name).collect();
    assert!(names.contains(&"apps.lock".to_owned()) && names.contains(&"apps.unlock".to_owned()), "{names:?}");
    app.stop();
    hub.shutdown().await;

    let off = Hub::start(config(0)).await.expect("hub");
    let (app, _) = start_docs(&off).await;
    let names: Vec<String> = off.tools(&ToolFilter::default()).into_iter().map(|t| t.name).collect();
    assert!(!names.iter().any(|n| n.starts_with("apps.lock") || n == "apps.unlock"), "{names:?}");
    assert_eq!(kind(&outcome(&off, "a", "apps.lock", json!({"appId": "docs"})).await), Some(ErrorKind::ToolNotFound));
    assert!(outcome(&off, "b", "docs.doc.edit", json!({})).await.is_ok());
    app.stop();
    off.shutdown().await;
}
