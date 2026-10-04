//! 单元测试（续）：只读结果缓存（v24，spec/hub-api.md 3.20）：App 声明 cache 的只读工具经 am_hub_call 第二次命中
//! （cachedAgeMs、不再调用 handler）、cacheBypass 照常调用、配置 resultCache.maxEntries: 0 关闭缓存、status.cache 统计。

use app_mcp_native::{CachePolicy, CacheScope};

use super::*;

/// 计数的只读 handler：返回第几次被调用。
struct Counter(Arc<AtomicUsize>);

impl ToolHandler for Counter {
    fn invoke(&self, call: CallHandle) {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = call.complete(Some(&json!({ "n": n }).to_string()), Vec::new());
    }
}

/// 注册 `quote.get`（只读、cache 60 s shared）的 App；返回 App 与 handler 调用计数。
fn start_cache_app(hub: *mut AmHub) -> (App, Arc<AtomicUsize>) {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("quote", "报价");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("q1".into());
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let runs = Arc::new(AtomicUsize::new(0));
    let mut spec = ToolSpec::new("get", "查询报价");
    spec.risk = Risk::Read;
    let options = ToolOptions {
        cache: Some(CachePolicy { ttl_ms: 60_000, scope: CacheScope::Shared }),
        ..ToolOptions::default()
    };
    let tool = client.register_tool_with(spec, options, Arc::new(Counter(runs.clone()))).expect("注册");
    client.start();
    (App { client, _handles: vec![Box::new(tool)] }, runs)
}

/// 等到 App 工具出现在 am_hub_tools_json 中。
fn wait_tool(hub: *mut AmHub, name: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        // SAFETY: 有效参数。
        let tools = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
        if tools.as_array().is_some_and(|t| t.iter().any(|t| t["name"] == name)) {
            return;
        }
        assert!(Instant::now() < deadline, "等待 App 工具超时");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn quote(hub: *mut AmHub, bypass: Option<bool>, tx: &Sender<String>, rx: &Receiver<String>) -> Value {
    let mut req = json!({"name": "quote.get", "arguments": {"symbol": "ACME"}});
    if let Some(b) = bypass {
        req["cacheBypass"] = json!(b);
    }
    call(hub, req, tx);
    recv(rx)
}

fn cache_status(hub: *mut AmHub) -> Value {
    // SAFETY: 有效参数。
    query_json(|o| unsafe { am_hub_status_json(hub, o) })["cache"].clone()
}

#[test]
fn cached_result_bypass_and_status() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    let (_app, runs) = start_cache_app(hub);
    wait_tool(hub, "quote.get");
    let (tx, rx) = mpsc::channel::<String>();

    // 第一次：调用 App，无 cachedAgeMs
    let first = quote(hub, None, &tx, &rx);
    assert_eq!(first["result"]["ok"], json!({"n": 1}), "{first}");
    assert!(first.get("cachedAgeMs").is_none(), "{first}");
    // 第二次：命中，内容相同、带 cachedAgeMs，handler 未再调用
    let second = quote(hub, None, &tx, &rx);
    assert_eq!(second["result"]["ok"], json!({"n": 1}), "{second}");
    assert!(second["cachedAgeMs"].is_u64(), "{second}");
    assert_eq!(second["woke"], json!(false));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // cacheBypass：照常调用并以新结果覆盖；之后命中新值
    let fresh = quote(hub, Some(true), &tx, &rx);
    assert_eq!(fresh["result"]["ok"], json!({"n": 2}), "{fresh}");
    assert!(fresh.get("cachedAgeMs").is_none(), "{fresh}");
    let again = quote(hub, Some(false), &tx, &rx);
    assert_eq!(again["result"]["ok"], json!({"n": 2}), "{again}");
    assert!(again["cachedAgeMs"].is_u64(), "{again}");
    assert_eq!(runs.load(Ordering::SeqCst), 2);

    // status.cache：1 条、命中 2 次、未命中 1 次（绕过不计）
    let status = cache_status(hub);
    assert_eq!(status["entries"], json!(1), "{status}");
    assert_eq!(status["hits"], json!(2), "{status}");
    assert_eq!(status["misses"], json!(1), "{status}");
    assert!(status["bytes"].as_u64().is_some_and(|b| b > 0), "{status}");

    // cacheBypass 不是布尔：请求 JSON 解析失败
    let bad = c(r#"{"name":"quote.get","cacheBypass":"yes"}"#);
    // SAFETY: 有效参数；回调不会被调用。
    let st = unsafe { am_hub_call(hub, bad.as_ptr(), Some(on_result), ud(&tx), ptr::null_mut()) };
    assert_eq!(st, AmHubStatus::InvalidJson);
    // SAFETY: 测试结束。
    unsafe { am_hub_free(hub) };
}

#[test]
fn result_cache_max_entries_zero_disables_cache() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","resultCache":{"maxEntries":0}}"#);
    let (_app, runs) = start_cache_app(hub);
    wait_tool(hub, "quote.get");
    let (tx, rx) = mpsc::channel::<String>();
    for n in 1..=2 {
        let out = quote(hub, None, &tx, &rx);
        assert_eq!(out["result"]["ok"], json!({ "n": n }), "{out}");
        assert!(out.get("cachedAgeMs").is_none(), "{out}");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    let status = cache_status(hub);
    assert_eq!((status["entries"].as_u64(), status["hits"].as_u64()), (Some(0), Some(0)), "{status}");
    // SAFETY: 测试结束。
    unsafe { am_hub_free(hub) };

    // 未知字段 / 负数：INVALID_JSON
    for bad in [r#"{"resultCache":{"maxItems":1}}"#, r#"{"resultCache":{"maxBytes":-1}}"#] {
        let cfg = c(bad);
        let mut out = ptr::null_mut();
        // SAFETY: 有效参数。
        assert_eq!(unsafe { am_hub_start(cfg.as_ptr(), &mut out) }, AmHubStatus::InvalidJson, "{bad}");
        assert!(out.is_null());
    }
}
