//! 核心热路径基准（TASKS 第 10 项、第 4f 项 l 的核心部分）：sans-IO，虚拟时钟，输入固定，可重复。
//!
//! 运行：`cargo bench -p app-mcp-core --bench hot_path`（bench profile = release + lto）。
//! 可选参数：`--quick`（迭代次数减为 1/5）。
//!
//! 测量项：
//! - `tools/invoke` 入站（文本 → [`Event::InvokeTool`]）与出站（[`Client::complete_call`] → 回复文本）的耗时、
//!   分配次数、分配字节；小参数与 1 MiB 级参数（对象数组 / 长字符串两种形态）。
//! - 驱动层把参数转回 JSON 文本（native 的 `arguments.to_string()`）的代价：参数原样透传可省掉的部分。
//! - 单次调用峰值堆增量、调用后保留的堆（去重表）。
//! - `toolsHash` 计算；休眠 → 唤醒重建（快速恢复 vs 完整同步）；冷启动（新建 + 注册 + 完整同步）。
//! - 取消延迟（`tools/cancel` → [`Event::CancelTool`]）；结果后回到可休眠状态的虚拟时长；空闲常驻堆。
//!
//! @why 不用 criterion：Cargo.lock 中没有，且需要分配计数（自定义全局分配器），手写计时足够且无新依赖。

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use app_mcp_core::*;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// 计数分配器
// ---------------------------------------------------------------------------

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn on_alloc(size: usize) {
    ALLOCS.fetch_add(1, Relaxed);
    ALLOC_BYTES.fetch_add(size, Relaxed);
    let cur = CURRENT.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(cur, Relaxed);
}

// @security 只转发到系统分配器并计数，不改变布局与对齐。
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        on_alloc(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        on_alloc(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // realloc 计为一次分配（新大小），旧块按释放处理。
        CURRENT.fetch_sub(layout.size(), Relaxed);
        on_alloc(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy, Default)]
struct Mem {
    allocs: usize,
    bytes: usize,
    /// 相对开始时的峰值堆增量。
    peak: usize,
}

fn measure<R>(f: impl FnOnce() -> R) -> (R, Duration, Mem) {
    let a0 = ALLOCS.load(Relaxed);
    let b0 = ALLOC_BYTES.load(Relaxed);
    let c0 = CURRENT.load(Relaxed);
    PEAK.store(c0, Relaxed);
    let t = Instant::now();
    let r = f();
    let d = t.elapsed();
    let m = Mem {
        allocs: ALLOCS.load(Relaxed) - a0,
        bytes: ALLOC_BYTES.load(Relaxed) - b0,
        peak: PEAK.load(Relaxed).saturating_sub(c0),
    };
    (r, d, m)
}

fn heap_now() -> usize {
    CURRENT.load(Relaxed)
}

// ---------------------------------------------------------------------------
// 统计与输出
// ---------------------------------------------------------------------------

struct Stats {
    samples: Vec<Duration>,
    mem: Mem,
}

impl Stats {
    fn new() -> Self {
        Self { samples: Vec::new(), mem: Mem::default() }
    }
    fn push(&mut self, d: Duration, m: Mem) {
        self.samples.push(d);
        // 分配数与峰值取最后一次（每次相同输入，稳定）。
        self.mem = m;
    }
    fn pct(&self, p: f64) -> Duration {
        let mut s = self.samples.clone();
        s.sort();
        let idx = ((s.len() as f64 - 1.0) * p).round() as usize;
        s.get(idx).copied().unwrap_or_default()
    }
}

fn fmt_dur(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns >= 1_000_000 { format!("{:.2} ms", ns as f64 / 1e6) } else { format!("{:.1} µs", ns as f64 / 1e3) }
}

fn fmt_bytes(b: usize) -> String {
    if b >= 1 << 20 {
        format!("{:.2} MiB", b as f64 / (1u64 << 20) as f64)
    } else if b >= 1 << 10 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn row(name: &str, s: &Stats) {
    println!(
        "| {name} | {} | {} | {} | {} | {} |",
        fmt_dur(s.pct(0.5)),
        fmt_dur(s.pct(0.95)),
        s.mem.allocs,
        fmt_bytes(s.mem.bytes),
        fmt_bytes(s.mem.peak)
    );
}

fn header(title: &str) {
    println!("\n### {title}\n");
    println!("| 项 | P50 | P95 | 分配次数 | 分配字节 | 峰值堆增量 |");
    println!("|---|---|---|---|---|---|");
}

// ---------------------------------------------------------------------------
// 固定输入
// ---------------------------------------------------------------------------

/// 对象数组形态（典型的列表 / 表格数据），约 `target` 字节。
fn payload_objects(target: usize) -> Value {
    let mut items = Vec::new();
    let mut len = 0;
    let mut i = 0u64;
    while len < target {
        let item = json!({
            "id": i,
            "sku": format!("SKU-{i:08}"),
            "name": format!("商品 {i} 名称"),
            "price": (i % 1000) as f64 + 0.5,
            "qty": i % 7,
            "tags": ["a", "bb", "ccc"],
            "inStock": i % 2 == 0,
        });
        len += item.to_string().len() + 1;
        items.push(item);
        i += 1;
    }
    json!({ "items": items })
}

/// 长字符串形态（base64 文件内容等），约 `target` 字节。
fn payload_blob(target: usize) -> Value {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let s: String = (0..target).map(|i| ALPHA[(i * 7 + i / 64) % 64] as char).collect();
    json!({ "name": "photo.jpg", "data": s })
}

fn tool_def(i: usize) -> ToolDef {
    ToolDef {
        name: format!("tool.n{i:03}"),
        description: format!("第 {i} 个工具：根据条件查询并返回订单、商品与库存信息"),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "关键词"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                "sort": {"type": "string", "enum": ["price", "date", "name"]},
                "filters": {"type": "object", "properties": {"inStock": {"type": "boolean"}}}
            },
            "required": ["query"]
        }),
        risk: if i % 3 == 0 { Risk::Write } else { Risk::Read },
        activation: None,
        title: Some(format!("工具 {i}")),
        enabled: true,
        scope: None,
        annotations: None,
        output_schema: None,
        surface: ToolSurface::App,
        page: None,
        background_tool: None,
    }
}

fn resource_def(i: usize) -> ResourceDef {
    ResourceDef {
        name: format!("res.n{i:02}"),
        description: format!("资源 {i}"),
        mime_type: Some("application/json".into()),
        scope: None,
        realtime: false,
        annotations: None,
    }
}

// ---------------------------------------------------------------------------
// 驱动
// ---------------------------------------------------------------------------

struct H {
    c: Client,
    now: Millis,
    next_id: i64,
}

fn drain(c: &mut Client) -> Vec<Event> {
    let mut out = Vec::new();
    while let Some(e) = c.poll_event() {
        out.push(e);
    }
    out
}

fn sent(events: &[Event]) -> impl Iterator<Item = &String> {
    events.iter().filter_map(|e| match e {
        Event::Send(s) => Some(s),
        _ => None,
    })
}

fn hello_id(events: &[Event]) -> Value {
    sent(events)
        .filter_map(|s| serde_json::from_str::<Value>(s).ok())
        .find(|m| m["method"] == "app/hello")
        .map(|m| m["id"].clone())
        .unwrap_or(Value::Null)
}

fn paired_text(id: &Value, tools_current: bool) -> String {
    let mut r = json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "bench"});
    if tools_current {
        r["toolsCurrent"] = json!(true);
    }
    json!({"jsonrpc": "2.0", "id": id, "result": r}).to_string()
}

fn config(mode: LifecycleMode) -> ClientConfig {
    let mut c = ClientConfig::new("bench", "基准", "inst-1", ClientKind::Native);
    c.lifecycle.mode = mode;
    c.transport = TransportKind::Ipc;
    c
}

impl H {
    fn new(mode: LifecycleMode, tools: usize, resources: usize) -> Self {
        let mut c = Client::new(config(mode));
        for i in 0..tools {
            c.register_tool(tool_def(i)).expect("register tool");
        }
        for i in 0..resources {
            c.register_resource(resource_def(i)).expect("register resource");
        }
        Self { c, now: 1_000, next_id: 0 }
    }

    fn connect(&mut self) {
        self.c.start(self.now);
        drain(&mut self.c);
        self.c.handle_connected(self.now);
        let ev = drain(&mut self.c);
        let id = hello_id(&ev);
        self.c.handle_message(&paired_text(&id, false), self.now);
        drain(&mut self.c);
        assert_eq!(self.c.state(), &ConnectionState::Connected);
    }

    fn invoke_text(&mut self, args: &str) -> (String, String) {
        self.next_id += 1;
        let call_id = format!("call-{}", self.next_id);
        let text = format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"tools/invoke","params":{{"callId":"{call_id}","name":"tool.n000","arguments":{args}}}}}"#,
            self.next_id
        );
        (call_id, text)
    }

    /// 推进虚拟时间直到发出 `app/sleep`，返回所用虚拟毫秒。
    fn until_sleep(&mut self, limit: Millis) -> Option<(Millis, Value)> {
        let start = self.now;
        while self.now < start + limit {
            let next = self.c.poll_timeout().unwrap_or(start + limit).min(start + limit).max(self.now + 1);
            self.now = next;
            self.c.handle_timeout(self.now);
            let ev = drain(&mut self.c);
            for s in sent(&ev) {
                let m: Value = serde_json::from_str(s).unwrap_or(Value::Null);
                if m["method"] == "app/sleep" {
                    return Some((self.now - start, m));
                }
            }
        }
        None
    }

    fn accept_sleep(&mut self, sleep: &Value) {
        let text = json!({"jsonrpc": "2.0", "id": sleep["id"], "result": {"accepted": true, "resumeToken": "resume-1"}})
            .to_string();
        self.c.handle_message(&text, self.now);
        drain(&mut self.c);
        assert_eq!(self.c.state(), &ConnectionState::Dormant);
    }
}

// ---------------------------------------------------------------------------
// 场景
// ---------------------------------------------------------------------------

struct Iters {
    small: usize,
    large: usize,
    misc: usize,
}

/// 参考（未实施，需驱动层配合）：参数以原始 JSON 文本透传给 handler 时，入站只需扫描信封并复制参数文本。
#[derive(Deserialize)]
struct RawEnvelope<'a> {
    #[serde(borrow)]
    params: &'a RawValue,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct RawInvokeParams<'a> {
    call_id: String,
    name: String,
    #[serde(borrow)]
    arguments: &'a RawValue,
}

fn raw_passthrough(text: &str) -> Option<String> {
    let env: RawEnvelope<'_> = serde_json::from_str(text).ok()?;
    let p: RawInvokeParams<'_> = serde_json::from_str(env.params.get()).ok()?;
    Some(p.arguments.get().to_owned())
}

fn bench_invoke(label: &str, args: &Value, result: &Value, n: usize) {
    let args_text = args.to_string();
    println!("\n#### {label}：参数 {} / 结果 {}", fmt_bytes(args_text.len()), fmt_bytes(result.to_string().len()));
    header(&format!("tools/invoke：{label}"));
    // 去重关闭：测纯解析 / 派发 / 回复；去重保留另测（bench_dedup_retention）。
    let mut cfg = config(LifecycleMode::Persistent);
    cfg.call_dedup = CallDedupPolicy::OFF;
    let mut h = H { c: Client::new(cfg), now: 1_000, next_id: 0 };
    for i in 0..16 {
        h.c.register_tool(tool_def(i)).expect("register");
    }
    h.connect();
    let mut inbound = Stats::new();
    let mut to_text = Stats::new();
    let mut outbound = Stats::new();
    let mut whole = Stats::new();
    let mut raw = Stats::new();
    let mut reply_len = 0;
    for i in 0..n + 2 {
        let (call_id, text) = h.invoke_text(&args_text);
        let now = h.now;
        let c = &mut h.c;
        let out = CallOutput { data: result.clone(), ..Default::default() };
        if i % 2 == 1 {
            // 整次调用（入站 + 驱动层转文本 + 出站），单独一轮以免嵌套测量干扰峰值。
            let (len, d, m) = measure(|| {
                c.handle_message(&text, now);
                let mut len = 0;
                while let Some(e) = c.poll_event() {
                    if let Event::InvokeTool { arguments, .. } = e {
                        len += black_box(arguments.to_string()).len();
                    }
                }
                c.complete_call(&call_id, Ok(out), now).expect("complete");
                while let Some(e) = c.poll_event() {
                    black_box(e);
                }
                len
            });
            assert_eq!(len, args_text.len());
            whole.push(d, m);
            continue;
        }
        let (arguments, d, m) = measure(|| {
            c.handle_message(&text, now);
            let mut found = None;
            while let Some(e) = c.poll_event() {
                if let Event::InvokeTool { arguments, .. } = e {
                    found = Some(arguments);
                }
            }
            found
        });
        inbound.push(d, m);
        let (txt, d, m) = measure(|| raw_passthrough(&text));
        assert_eq!(txt.map(|t| t.len()), Some(args_text.len()));
        raw.push(d, m);
        let arguments = arguments.expect("InvokeTool");
        let (txt, d, m) = measure(|| arguments.to_string());
        assert_eq!(txt.len(), args_text.len());
        to_text.push(d, m);
        drop((txt, arguments));
        let (len, d, m) = measure(|| {
            c.complete_call(&call_id, Ok(out), now).expect("complete");
            let mut len = 0;
            while let Some(e) = c.poll_event() {
                if let Event::Send(s) = &e {
                    len += s.len();
                }
                black_box(e);
            }
            len
        });
        reply_len = len;
        outbound.push(d, m);
    }
    // 丢弃第一次（预热）
    for s in [&mut inbound, &mut to_text, &mut outbound, &mut whole, &mut raw] {
        s.samples.remove(0);
    }
    row("入站：解析 + 派发（核心）", &inbound);
    row("驱动层：arguments → JSON 文本", &to_text);
    row(&format!("出站：complete_call → 回复文本 {}（核心）", fmt_bytes(reply_len)), &outbound);
    row("整次调用（入站 + 转文本 + 出站）", &whole);
    row("参考：参数原始文本透传（入站 + 转文本的替代，未实施）", &raw);
}

fn bench_dedup_retention(label: &str, result: &Value) {
    let mut h = H::new(LifecycleMode::Persistent, 16, 0);
    h.connect();
    let before = heap_now();
    for _ in 0..CallDedupPolicy::DEFAULT_MAX_ENTRIES {
        let (call_id, text) = h.invoke_text("{}");
        h.c.handle_message(&text, h.now);
        drain(&mut h.c);
        h.c.complete_call(&call_id, Ok(CallOutput { data: result.clone(), ..Default::default() }), h.now)
            .expect("complete");
        drain(&mut h.c);
    }
    let retained = heap_now().saturating_sub(before);
    println!(
        "| {label} | {} | {} | {} |",
        fmt_bytes(result.to_string().len()),
        fmt_bytes(retained),
        fmt_bytes(retained / CallDedupPolicy::DEFAULT_MAX_ENTRIES)
    );
}

fn bench_tools_hash(n: usize, iters: usize) {
    let h = H::new(LifecycleMode::Persistent, n, 4);
    let mut s = Stats::new();
    for _ in 0..iters {
        let (r, d, m) = measure(|| h.c.tools_hash());
        black_box(r);
        s.push(d, m);
    }
    row(&format!("toolsHash（{n} 工具 / 4 资源）"), &s);
}

/// 休眠 → 唤醒 → 握手完成（`app/ready` 已发出）。`fast` = Host 回 `toolsCurrent: true`。
fn bench_wake(n: usize, fast: bool, iters: usize) {
    let mut s = Stats::new();
    let mut sent_bytes = 0;
    for _ in 0..iters {
        let mut h = H::new(LifecycleMode::Idle, n, 4);
        h.connect();
        let (_, sleep) = h.until_sleep(10 * 60_000).expect("app/sleep");
        h.accept_sleep(&sleep);
        let now = h.now;
        let c = &mut h.c;
        let (bytes, d, m) = measure(|| {
            let mut bytes = 0;
            c.wake(now);
            drain(c);
            c.handle_connected(now);
            let ev = drain(c);
            bytes += sent(&ev).map(String::len).sum::<usize>();
            let id = hello_id(&ev);
            c.handle_message(&paired_text(&id, fast), now);
            let ev = drain(c);
            bytes += sent(&ev).map(String::len).sum::<usize>();
            bytes
        });
        assert_eq!(h.c.state(), &ConnectionState::Connected);
        sent_bytes = bytes;
        s.push(d, m);
    }
    let kind = if fast { "快速恢复" } else { "完整同步" };
    row(&format!("唤醒重建：{kind}（{n} 工具，发出 {}）", fmt_bytes(sent_bytes)), &s);
}

/// 冷启动：新建 Client + 注册 + 连接 + 完整同步。
fn bench_cold(n: usize, iters: usize) {
    let mut s = Stats::new();
    for _ in 0..iters {
        let (c, d, m) = measure(|| {
            let mut h = H::new(LifecycleMode::Idle, n, 4);
            h.connect();
            h
        });
        black_box(c);
        s.push(d, m);
    }
    row(&format!("冷启动：新建 + 注册 + 完整同步（{n} 工具）"), &s);
}

fn bench_cancel(iters: usize) {
    let mut h = H::new(LifecycleMode::Persistent, 16, 0);
    h.connect();
    let mut s = Stats::new();
    for _ in 0..iters {
        let (call_id, text) = h.invoke_text(r#"{"a":1}"#);
        h.c.handle_message(&text, h.now);
        drain(&mut h.c);
        let cancel = format!(r#"{{"jsonrpc":"2.0","method":"tools/cancel","params":{{"callId":"{call_id}"}}}}"#);
        let now = h.now;
        let c = &mut h.c;
        let (found, d, m) = measure(|| {
            c.handle_message(&cancel, now);
            let mut found = false;
            while let Some(e) = c.poll_event() {
                found |= matches!(e, Event::CancelTool { .. });
            }
            found
        });
        assert!(found);
        s.push(d, m);
    }
    row("取消：tools/cancel → CancelTool + 回复（核心）", &s);
}

/// 结果后多久（虚拟时间）发出 `app/sleep`：由策略决定（合并窗口 / 租约），不是 CPU 时间。
fn release_delay() {
    for (label, lease) in [("无租约", None), ("Host 租约 5 s", Some(5_000u64))] {
        let mut h = H::new(LifecycleMode::Idle, 16, 0);
        h.connect();
        if let Some(ttl) = lease {
            let text = json!({"jsonrpc": "2.0", "method": "app/lease", "params": {"ttlMs": ttl}}).to_string();
            h.c.handle_message(&text, h.now);
            drain(&mut h.c);
        }
        let (call_id, text) = h.invoke_text("{}");
        h.c.handle_message(&text, h.now);
        drain(&mut h.c);
        h.c.complete_call(&call_id, Ok(CallOutput::default()), h.now).expect("complete");
        drain(&mut h.c);
        let at = h.until_sleep(10 * 60_000).map(|(ms, _)| ms);
        println!("| idle 模式，{label} | {} |", at.map_or("未休眠".into(), |ms| format!("{ms} ms")));
    }
}

fn idle_heap(n: usize) {
    let before = heap_now();
    let mut h = H::new(LifecycleMode::Idle, n, 4);
    let registered = heap_now() - before;
    h.connect();
    let connected = heap_now() - before;
    let (_, sleep) = h.until_sleep(10 * 60_000).expect("sleep");
    h.accept_sleep(&sleep);
    let dormant = heap_now() - before;
    println!(
        "| {n} 工具 / 4 资源 | {} | {} | {} |",
        fmt_bytes(registered),
        fmt_bytes(connected),
        fmt_bytes(dormant)
    );
    black_box(h);
}

fn main() {
    let quick = std::env::args().any(|a| a == "--quick");
    let it = if quick { Iters { small: 400, large: 6, misc: 40 } } else { Iters { small: 2000, large: 30, misc: 200 } };
    println!("# app-mcp-core 热路径基准");
    println!(
        "\nprofile = {}；迭代：小 {} / 大 {} / 其他 {}",
        if cfg!(debug_assertions) { "debug" } else { "release" },
        it.small,
        it.large,
        it.misc
    );

    let small_args = json!({"a": 20, "b": 22, "note": "hello"});
    let small_result = json!({"sum": 42});
    let obj = payload_objects(1 << 20);
    let blob = payload_blob(1 << 20);
    let half = payload_objects(1 << 19);

    bench_invoke("小参数", &small_args, &small_result, it.small);
    bench_invoke("0.5 MiB 对象数组（参数与结果相同）", &half, &half, it.large);
    bench_invoke("1 MiB 对象数组（参数与结果相同）", &obj, &obj, it.large);
    bench_invoke("1 MiB 长字符串（参数与结果相同）", &blob, &blob, it.large);

    println!("\n### 去重表保留（默认 64 条 / 5 分钟，64 次调用后）\n");
    println!("| 结果 | 单个结果 JSON | 保留堆 | 每条 |");
    println!("|---|---|---|---|");
    bench_dedup_retention("小结果", &small_result);
    bench_dedup_retention("1 MiB 对象数组", &obj);
    bench_dedup_retention("1 MiB 长字符串", &blob);

    header("toolsHash");
    bench_tools_hash(16, it.misc);
    bench_tools_hash(128, it.misc / 4);

    header("休眠 → 唤醒重建 / 冷启动（核心 CPU，不含 I/O）");
    for n in [16, 128] {
        bench_wake(n, true, it.misc / 4);
        bench_wake(n, false, it.misc / 4);
        bench_cold(n, it.misc / 4);
    }

    header("取消延迟（核心）");
    bench_cancel(it.misc);

    println!("\n### 结果后回到可休眠（虚拟时间，策略决定）\n");
    println!("| 场景 | 结果 → app/sleep |");
    println!("|---|---|");
    release_delay();

    println!("\n### 空闲常驻堆（核心 Client）\n");
    println!("| 规模 | 注册后 | 已连接 | 休眠 |");
    println!("|---|---|---|---|");
    idle_heap(16);
    idle_heap(128);
}
