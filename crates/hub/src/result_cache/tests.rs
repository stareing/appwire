//! 结果缓存的单元测试：键规范化、范围、LRU 与字节上限、单条上限、惰性 TTL（可控时刻），以及 Hub 层每条失效规则（T-09）。
//! 端到端（真实 App 连接、休眠不唤醒、MCP 出口）见 `tests/it/result_cache.rs`。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_protocol::{CacheScope, ErrorKind, ResultStatus, ToolError, ToolsInvokeResult};
use rmcp::model::ResourceContents;
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::call::ToolRun;
use crate::hub::{HubConfig, HubShared};
use crate::mcp_convert::OutputShape;
use crate::task::CallerKey;

use super::key::canonical_json;
use super::{CacheKey, CacheLimits, CacheStatus, ResultCache};

const MS: Duration = Duration::from_millis(1);
const TTL: Duration = Duration::from_secs(5);

fn tool_key(app: &str, tool: &str, args: Value) -> CacheKey {
    CacheKey::tool(CacheScope::Shared, "local", app, tool, &args)
}

fn res_key(app: &str, name: &str) -> CacheKey {
    CacheKey::resource(CacheScope::Shared, "local", app, name)
}

fn cache(max_entries: usize, max_bytes: usize, max_entry_bytes: usize) -> ResultCache<&'static str> {
    ResultCache::new(CacheLimits { max_entries, max_bytes, max_entry_bytes })
}

fn any(_: &&'static str) -> bool {
    true
}

// ---------------------------------------------------------------------------
// 键
// ---------------------------------------------------------------------------

#[test]
fn canonical_json_sorts_keys_recursively() {
    let a = json!({"b": 1, "a": {"d": [1, {"z": 1, "y": "é\"\n"}], "c": null}});
    let b: Value = serde_json::from_str(r#"{"a":{"c":null,"d":[1,{"y":"é\"\n","z":1}]},"b":1}"#).unwrap();
    assert_eq!(canonical_json(&a), canonical_json(&b));
    assert_eq!(canonical_json(&a), r#"{"a":{"c":null,"d":[1,{"y":"é\"\n","z":1}]},"b":1}"#);
    // 数组顺序有意义；值不同即不同键
    assert_ne!(canonical_json(&json!([1, 2])), canonical_json(&json!([2, 1])));
    assert_ne!(canonical_json(&json!({"a": 1})), canonical_json(&json!({"a": "1"})));
    // 字节序：大写在小写之前，多字节字符在 ASCII 之后
    assert_eq!(canonical_json(&json!({"b": 0, "B": 0, "é": 0, "a": 0})), r#"{"B":0,"a":0,"b":0,"é":0}"#);
}

#[test]
fn keys_with_reordered_arguments_hit_same_entry() {
    let mut c = cache(8, 1 << 20, 1 << 16);
    let now = Instant::now();
    assert!(c.insert(tool_key("shop", "search", json!({"q": "x", "page": 1})), "r", 1, TTL, now));
    let hit = c.get(&tool_key("shop", "search", json!({"page": 1, "q": "x"})), now, any);
    assert_eq!(hit.map(|h| h.value), Some("r"));
    assert!(c.get(&tool_key("shop", "search", json!({"page": 2, "q": "x"})), now, any).is_none());
    assert!(c.get(&tool_key("shop", "list", json!({"page": 1, "q": "x"})), now, any).is_none(), "工具名在键中");
    assert!(c.get(&tool_key("mall", "search", json!({"page": 1, "q": "x"})), now, any).is_none(), "appId 在键中");
}

#[test]
fn private_scope_is_per_subject_and_shared_is_common() {
    let args = json!({});
    let alice = CacheKey::tool(CacheScope::Private, "agent:alice", "shop", "t", &args);
    let bob = CacheKey::tool(CacheScope::Private, "agent:bob", "shop", "t", &args);
    let shared_a = CacheKey::tool(CacheScope::Shared, "agent:alice", "shop", "t", &args);
    let shared_b = CacheKey::tool(CacheScope::Shared, "agent:bob", "shop", "t", &args);
    assert_ne!(alice, bob);
    assert_eq!(shared_a, shared_b);
    assert_ne!(alice, shared_a, "private 与 shared 不共用条目");
    let r_alice = CacheKey::resource(CacheScope::Private, "agent:alice", "shop", "cart");
    assert_ne!(r_alice, CacheKey::resource(CacheScope::Private, "local", "shop", "cart"));
}

// ---------------------------------------------------------------------------
// 存储：LRU、字节上限、单条上限、TTL、关闭
// ---------------------------------------------------------------------------

#[test]
fn lru_evicts_least_recently_used_entry() {
    let mut c = cache(2, 1 << 20, 1 << 16);
    let now = Instant::now();
    c.insert(tool_key("a", "t", json!(1)), "one", 1, TTL, now);
    c.insert(tool_key("a", "t", json!(2)), "two", 1, TTL, now);
    assert!(c.get(&tool_key("a", "t", json!(1)), now, any).is_some(), "访问 one，two 变为最久未用");
    c.insert(tool_key("a", "t", json!(3)), "three", 1, TTL, now);
    assert!(c.get(&tool_key("a", "t", json!(2)), now, any).is_none(), "two 被淘汰");
    assert!(c.get(&tool_key("a", "t", json!(1)), now, any).is_some());
    assert!(c.get(&tool_key("a", "t", json!(3)), now, any).is_some());
    assert_eq!(c.status().entries, 2);
    assert_eq!(c.status().evictions, 1);
}

#[test]
fn byte_limit_evicts_oldest_until_within() {
    let k = |i: i32| res_key("a", &format!("r{i}"));
    let key_bytes = k(1).bytes();
    let mut c = cache(100, 3 * (key_bytes + 10), 1 << 16);
    let now = Instant::now();
    for i in 1..=3 {
        assert!(c.insert(k(i), "v", 10, TTL, now));
    }
    assert_eq!(c.status().bytes as usize, 3 * (key_bytes + 10));
    assert!(c.insert(k(4), "v", 15, TTL, now), "超出字节上限：淘汰最久未用的直到放得下");
    assert!(c.get(&k(1), now, any).is_none());
    assert!(c.get(&k(2), now, any).is_none(), "淘汰一条仍超出时继续淘汰");
    assert!(c.get(&k(3), now, any).is_some() && c.get(&k(4), now, any).is_some());
    assert!(c.status().bytes as usize <= 3 * (key_bytes + 10));
    assert_eq!(c.status().evictions, 2);
}

#[test]
fn oversized_entry_is_not_stored_and_drops_old_value() {
    let key = res_key("a", "big");
    let limit = key.bytes() + 100;
    let mut c = cache(8, 1 << 20, limit);
    let now = Instant::now();
    assert!(c.insert(key.clone(), "small", 100, TTL, now), "恰好等于上限可存");
    assert!(!c.insert(key.clone(), "big", 101, TTL, now), "超过 max_entry_bytes 不存");
    assert!(c.get(&key, now, any).is_none(), "以新结果为准：旧值也不再命中");
    assert_eq!(c.status(), CacheStatus { entries: 0, bytes: 0, hits: 0, misses: 1, evictions: 0 });
}

#[test]
fn ttl_expires_lazily_on_access() {
    let mut c = cache(8, 1 << 20, 1 << 16);
    let t0 = Instant::now();
    let key = res_key("a", "r");
    c.insert(key.clone(), "v", 1, TTL, t0);
    let hit = c.get(&key, t0 + TTL - MS, any).expect("到期前命中");
    assert_eq!(hit.age, TTL - MS);
    assert_eq!(hit.remaining, MS);
    assert_eq!(c.status().entries, 1, "未访问时不清理（无定时器）");
    assert!(c.get(&key, t0 + TTL, any).is_none(), "到期即未命中");
    assert_eq!(c.status().entries, 0, "访问时移除过期条目");
    assert_eq!((c.status().hits, c.status().misses), (1, 1));
}

#[test]
fn zero_max_entries_disables_cache() {
    let mut c = cache(0, 1 << 20, 1 << 16);
    assert!(!c.enabled());
    assert!(!c.insert(res_key("a", "r"), "v", 1, TTL, Instant::now()));
    assert_eq!(c.status().entries, 0);
}

#[test]
fn rejected_hit_counts_as_miss() {
    let mut c = cache(8, 1 << 20, 1 << 16);
    let now = Instant::now();
    c.insert(res_key("a", "r"), "v", 1, TTL, now);
    assert!(c.get(&res_key("a", "r"), now, |v| *v == "other").is_none());
    assert_eq!((c.status().hits, c.status().misses, c.status().entries), (0, 1, 1));
}

#[test]
fn clear_app_and_clear_resource_are_targeted() {
    let mut c = cache(16, 1 << 20, 1 << 16);
    let now = Instant::now();
    let private = CacheKey::resource(CacheScope::Private, "local", "a", "cart");
    c.insert(res_key("a", "cart"), "v", 1, TTL, now);
    c.insert(private.clone(), "v", 1, TTL, now);
    c.insert(res_key("a", "orders"), "v", 1, TTL, now);
    c.insert(tool_key("a", "cart", json!({})), "v", 1, TTL, now);
    c.insert(res_key("b", "cart"), "v", 1, TTL, now);
    assert_eq!(c.clear_resource("a", "cart"), 2, "各范围的同名资源条目，不含同名工具");
    assert!(c.get(&private, now, any).is_none());
    assert!(c.get(&tool_key("a", "cart", json!({})), now, any).is_some());
    assert!(c.get(&res_key("b", "cart"), now, any).is_some(), "其他 App 不受影响");
    assert_eq!(c.clear_app("a"), 2);
    assert_eq!(c.status().entries, 1);
    assert_eq!(c.status().bytes as usize, res_key("b", "cart").bytes() + 1, "字节数随清除扣减");
    let e = c.epoch();
    c.bump_epoch();
    assert_ne!(c.epoch(), e);
}

// ---------------------------------------------------------------------------
// Hub 层：只读判断、存入条件、各失效规则（T-09）
// ---------------------------------------------------------------------------

fn manifest() -> app_mcp_manifest::Manifest {
    let tool = |name: &str, risk: &str, cache: Value| {
        json!({"name": name, "description": name, "inputSchema": {"type": "object"}, "risk": risk, "cache": cache})
    };
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "tools": [
                tool("list", "read", json!({"ttlMs": 60000})),
                tool("feed", "read", json!({"ttlMs": 60000, "scope": "shared"})),
                tool("add", "write", json!({"ttlMs": 60000})),
                tool("now", "read", Value::Null),
            ],
            "resources": [{"name": "cart", "description": "购物车", "cache": {"ttlMs": 60000}}]
        })
        .to_string(),
    )
    .unwrap()
}

fn shared() -> Arc<HubShared> {
    HubShared::new_for_test(HubConfig { listen: None, ipc_endpoint: None, manifests: vec![manifest()], ..HubConfig::default() })
}

fn ok_run(hints: &[&str], status: ResultStatus) -> ToolRun {
    let result = ToolsInvokeResult {
        data: json!({"n": 1}),
        state_hints: hints.iter().map(|s| (*s).to_owned()).collect(),
        status,
        ..ToolsInvokeResult::default()
    };
    ToolRun { result: Ok(result), instance_id: Some("i1".into()), output_shape: OutputShape::Undeclared, woke: false }
}

fn err_run() -> ToolRun {
    ToolRun {
        result: Err(ToolError::new(ErrorKind::Timeout, "超时")),
        instance_id: Some("i1".into()),
        output_shape: OutputShape::Undeclared,
        woke: false,
    }
}

fn caller() -> CallerKey {
    CallerKey::api(None)
}

/// 调用 `tool`（结果为 `run`）并结算缓存。
fn settle(s: &HubShared, tool: &str, run: &ToolRun) {
    s.settle_tool_cache(&caller(), "shop", tool, false, &json!({}), run, s.cache_epoch());
}

fn cached(s: &HubShared, tool: &str) -> bool {
    s.lookup_tool_cache(&caller(), "shop", tool, &json!({}), None).is_some()
}

fn cart_contents() -> ResourceContents {
    ResourceContents::text("{}", "app-mcp://shop/cart")
}

/// 存入一条 `list` 结果与一条 `cart` 资源。
fn seeded() -> Arc<HubShared> {
    let s = shared();
    settle(&s, "list", &ok_run(&[], ResultStatus::Done));
    let policy = s.resource_cache_policy("shop", "cart").expect("资源声明了 cache");
    s.store_resource_cache(&caller(), "shop", "cart", policy, &cart_contents(), s.cache_epoch());
    assert!(cached(&s, "list") && cart_cached(&s));
    s
}

fn cart_cached(s: &HubShared) -> bool {
    let policy = s.resource_cache_policy("shop", "cart").expect("资源声明了 cache");
    s.lookup_resource_cache(&caller(), "shop", "cart", policy).is_some()
}

#[test]
fn store_conditions() {
    let cases: [(&str, ToolRun, bool, &str); 7] = [
        ("list", ok_run(&[], ResultStatus::Done), true, "只读 + cache + done"),
        ("list", ok_run(&[], ResultStatus::Noop), true, "noop"),
        ("list", ok_run(&[], ResultStatus::Pending), false, "pending 不存"),
        ("list", ok_run(&[], ResultStatus::Partial), false, "partial 不存"),
        ("list", err_run(), false, "isError 不存"),
        ("add", ok_run(&[], ResultStatus::Done), false, "写工具上的 cache 被忽略"),
        ("now", ok_run(&[], ResultStatus::Done), false, "未声明 cache 不存"),
    ];
    for (tool, run, want, why) in cases {
        let s = shared();
        settle(&s, tool, &run);
        assert_eq!(cached(&s, tool), want, "{why}");
    }
}

#[test]
fn hit_rebuilds_result_with_original_instance_and_age() {
    let s = shared();
    settle(&s, "list", &ok_run(&["orders"], ResultStatus::Done));
    let hit = s.lookup_tool_cache(&caller(), "shop", "list", &json!({}), None).expect("命中");
    assert_eq!(hit.result.data, json!({"n": 1}));
    assert_eq!(hit.result.state_hints, vec!["orders".to_owned()], "内容与原结果相同");
    assert_eq!(hit.instance_id.as_deref(), Some("i1"));
    assert!(s.lookup_tool_cache(&caller(), "shop", "list", &json!({}), Some("i1")).is_some());
    assert!(s.lookup_tool_cache(&caller(), "shop", "list", &json!({}), Some("i2")).is_none(), "指定实例只命中该实例的结果");
    assert!(s.lookup_tool_cache(&caller(), "shop", "list", &json!(7), None).is_none(), "参数不通过 inputSchema 不查");
}

#[test]
fn private_and_shared_scopes_at_hub_level() {
    let s = shared();
    let other = CallerKey::api(Some("other-session"));
    for tool in ["list", "feed"] {
        s.settle_tool_cache(&caller(), "shop", tool, false, &json!({}), &ok_run(&[], ResultStatus::Done), s.cache_epoch());
    }
    assert!(s.lookup_tool_cache(&other, "shop", "list", &json!({}), None).is_some(), "Hub API 各会话的记账主体都是 api");
    #[cfg(feature = "mcp-server")]
    {
        let local = CallerKey::mcp_session(1, None);
        assert!(s.lookup_tool_cache(&local, "shop", "list", &json!({}), None).is_none(), "private：local 主体不共用 api 的条目");
        assert!(s.lookup_tool_cache(&local, "shop", "feed", &json!({}), None).is_some(), "shared：全体调用方共用");
    }
}

#[test]
fn routed_and_changed_epoch_results_are_not_stored() {
    let s = shared();
    s.settle_tool_cache(&caller(), "shop", "list", true, &json!({}), &ok_run(&[], ResultStatus::Done), s.cache_epoch());
    assert!(!cached(&s, "list"), "改调后台替代的结果不存");
    let epoch = s.cache_epoch();
    s.invalidate_resource_cache("shop", "cart");
    s.settle_tool_cache(&caller(), "shop", "list", false, &json!({}), &ok_run(&[], ResultStatus::Done), epoch);
    assert!(!cached(&s, "list"), "调用期间发生数据失效：不存");
}

// 失效规则表（spec/hub-api.md 3.20「失效」），每条一个用例。

#[test]
fn invalidate_on_write_call_success() {
    let s = seeded();
    settle(&s, "add", &ok_run(&[], ResultStatus::Done));
    assert!(!cached(&s, "list") && !cart_cached(&s), "写调用完成：清空该 App");
}

#[test]
fn invalidate_on_write_call_failure() {
    let s = seeded();
    settle(&s, "add", &err_run());
    assert!(!cached(&s, "list") && !cart_cached(&s), "写调用失败 / 超时 / 取消同样清空");
}

#[test]
fn invalidate_on_unknown_tool_call_conservatively() {
    let s = seeded();
    settle(&s, "ghost", &err_run());
    assert!(!cached(&s, "list"), "定义未知的工具按非只读处理");
}

#[test]
fn read_only_call_does_not_invalidate() {
    let s = seeded();
    settle(&s, "now", &ok_run(&[], ResultStatus::Done));
    settle(&s, "list", &err_run());
    assert!(cached(&s, "list") && cart_cached(&s));
}

#[test]
fn invalidate_on_state_hints() {
    let s = seeded();
    settle(&s, "now", &ok_run(&["cart"], ResultStatus::Done));
    assert!(!cart_cached(&s), "stateHints 点名的资源失效");
    assert!(cached(&s, "list"), "其他条目保留");
}

#[test]
fn state_hints_invalidate_before_own_store() {
    let s = seeded();
    settle(&s, "list", &ok_run(&["cart"], ResultStatus::Done));
    assert!(!cart_cached(&s));
    assert!(cached(&s, "list"), "自身结果的 stateHints 不妨碍自身存入");
}

#[test]
fn invalidate_on_declaration_change() {
    let s = seeded();
    s.invalidate_app_cache("shop");
    assert!(!cached(&s, "list") && !cart_cached(&s), "tools / resources 的 sync / changed：清空该 App");
}

#[test]
fn invalidate_on_resource_updated() {
    let s = seeded();
    s.resource_updated("shop", "cart");
    assert!(!cart_cached(&s), "resources/updated：清该资源");
    assert!(cached(&s, "list"));
}

#[test]
fn disabled_cache_neither_looks_up_nor_stores() {
    let s = HubShared::new_for_test(HubConfig {
        listen: None,
        ipc_endpoint: None,
        manifests: vec![manifest()],
        result_cache: CacheLimits { max_entries: 0, ..CacheLimits::default() },
        ..HubConfig::default()
    });
    settle(&s, "list", &ok_run(&[], ResultStatus::Done));
    assert!(!cached(&s, "list"));
    assert!(s.resource_cache_policy("shop", "cart").is_none());
    assert_eq!(s.cache_status(), CacheStatus::default(), "关闭时不计未命中");
}
