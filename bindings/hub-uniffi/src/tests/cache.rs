//! 端到端单元测试（续）：只读结果缓存（spec/hub-api.md 3.20）经绑定层的配置、绕过、结果与状态。

use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// 只读工具：每次执行计数并返回计数。
struct CountingGet(Arc<AtomicU32>);

impl native::ToolHandler for CountingGet {
    fn invoke(&self, call: native::CallHandle) {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = call.complete(Some(&json!({ "n": n }).to_string()), vec![]);
    }
}

fn start_cache_hub(result_cache: Option<CacheLimitOverrides>) -> (Arc<AppMcpHub>, mpsc::Receiver<HubEvent>) {
    let hub = AppMcpHub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        enable_ipc: false,
        result_cache,
        ..Default::default()
    })
    .expect("启动 Hub");
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    (hub, rx)
}

/// 连上 Hub 的 App `kv`：声明 `cache` 的只读工具 `kv.get`。
fn start_kv_app(hub: &AppMcpHub, rx: &mpsc::Receiver<HubEvent>, count: Arc<AtomicU32>) -> native::NativeClient {
    let mut cfg = native::NativeConfig::new("kv", "KV");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let mut get = native::ToolSpec::new("kv.get", "读取");
    get.risk = native::Risk::Read;
    let options = native::ToolOptions {
        cache: Some(native::CachePolicy { ttl_ms: 60_000, scope: native::CacheScope::Private }),
        ..Default::default()
    };
    app.register_tool_with(get, options, Arc::new(CountingGet(count))).expect("注册");
    app.start();
    wait_for(rx, |e| matches!(e, HubEvent::AppConnected { app_id, .. } if app_id == "kv"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !hub.tools(ToolFilter::default()).iter().any(|t| t.name == "kv.kv.get") {
        assert!(Instant::now() < deadline, "等待 kv.get 注册超时");
        std::thread::sleep(Duration::from_millis(10));
    }
    app
}

fn get(hub: &AppMcpHub, bypass: bool) -> CallOutcome {
    let out = wait(hub.call_tool(CallRequest { cache_bypass: bypass, ..req("kv.kv.get", json!({})) })).expect("调用");
    assert!(out.error.is_none(), "{out:?}");
    out
}

/// 第二次调用命中（`cached_age_ms` 有值、App 只执行一次、内容相同）；`cache_bypass` 照常执行；`status().cache` 计数。
#[test]
fn cached_result_bypass_and_status() {
    let (hub, rx) = start_cache_hub(None);
    let count = Arc::new(AtomicU32::new(0));
    let app = start_kv_app(&hub, &rx, count.clone());

    let first = get(&hub, false);
    assert_eq!(first.cached_age_ms, None);
    let second = get(&hub, false);
    assert!(second.cached_age_ms.is_some(), "{second:?}");
    assert_eq!(second.data_json, first.data_json, "命中返回原结果");
    assert!(!second.woke);
    assert_eq!(count.load(Ordering::SeqCst), 1, "命中不转发给 App");

    let fresh = get(&hub, true);
    assert_eq!(fresh.cached_age_ms, None);
    assert_eq!(count.load(Ordering::SeqCst), 2, "绕过照常调用");
    let after = get(&hub, false);
    assert_eq!(after.data_json, fresh.data_json, "绕过的新结果覆盖缓存");

    let cache = hub.status().expect("status").cache.expect("cache 状态");
    assert_eq!((cache.entries, cache.hits, cache.misses), (1, 2, 1), "{cache:?}");
    assert!(cache.bytes > 0, "{cache:?}");
    app.stop();
    hub.shutdown();
}

/// `result_cache.max_entries: 0` 关闭缓存：每次都转发、不计命中。
#[test]
fn zero_max_entries_disables_cache() {
    let (hub, rx) = start_cache_hub(Some(CacheLimitOverrides { max_entries: Some(0), ..Default::default() }));
    let count = Arc::new(AtomicU32::new(0));
    let app = start_kv_app(&hub, &rx, count.clone());
    assert_eq!(get(&hub, false).cached_age_ms, None);
    assert_eq!(get(&hub, false).cached_age_ms, None);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let cache = hub.status().expect("status").cache.unwrap_or_default();
    assert_eq!((cache.entries, cache.hits), (0, 0), "{cache:?}");
    app.stop();
    hub.shutdown();
}

/// 上限在 Hub 启动时校验（spec/hub-api.md 3.20）：开启时单条大于总量 / 总量为 0 → 启动失败，信息指出字段；
/// 状态带生效上限（只给 `max_bytes` 时单条上限随之收窄，不报错）。
#[test]
fn invalid_limits_fail_start_and_status_reports_limits() {
    for bad in [
        CacheLimitOverrides { max_bytes: Some(100), max_entry_bytes: Some(200), ..Default::default() },
        CacheLimitOverrides { max_bytes: Some(0), ..Default::default() },
    ] {
        let r = AppMcpHub::start(HubConfig { enable_listen: false, enable_ipc: false, result_cache: Some(bad.clone()), ..Default::default() });
        match r {
            Err(e) => assert!(format!("{e:?}").contains("resultCache."), "{bad:?}：{e:?}"),
            Ok(_) => panic!("非法上限应启动失败：{bad:?}"),
        }
    }
    let (hub, _rx) = start_cache_hub(Some(CacheLimitOverrides { max_bytes: Some(1000), ..Default::default() }));
    let cache = hub.status().expect("status").cache.expect("cache 状态");
    assert_eq!((cache.max_entries, cache.max_bytes, cache.max_entry_bytes), (1024, 1000, 1000), "{cache:?}");
    hub.shutdown();
}
