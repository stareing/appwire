//! 生命周期指标基准（TASKS 第 4f 项 l 的桌面部分、第 10 项）：进程内 Hub + 真实 `app-mcp-native` 客户端，
//! 经回环 WebSocket（不占用默认 IPC 端点）。默认忽略，手动运行：
//!
//! ```text
//! cargo test --release -p app-mcp-host --test lifecycle_bench -- --ignored --nocapture
//! ```
//!
//! 输出 Markdown 表格：热调用 P50 / P95、1 MiB 参数调用、热唤醒（休眠实例回连 + 快速恢复）、进程内冷唤醒（实例被回收后
//! 新建客户端 + 完整同步，不含 OS 启动进程）、取消延迟、结果后回到休眠的延迟、每个客户端的线程 / 常驻内存、
//! 休眠期间的上下文切换（唤醒次数）、单次调用 CPU、恢复率与重复执行率。
//!
//! @why 放在 host 的测试里：host 已把 hub 与 native 作为（开发）依赖，基准不给 core / native / hub 增加依赖。
//! 数值含 Hub 与客户端两侧（同一进程），只用于同一机器上的前后对比。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_mcp_hub::{CallRequest, ErrorKind, Hub, HubConfig, HubError, LimitPolicy, RateLimit, WakeRequest, Waker, async_trait};
use app_mcp_native::{
    CallHandle, CancelListener, CancelReason, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler,
    ToolSpec, WakeDescriptor, WakeKind,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);
/// 每轮唤醒循环数（每轮含合并窗口等待，约 0.3 s）。
const WAKE_ROUNDS: usize = 20;
const CALLS: usize = 300;
const CLIENTS: usize = 8;

// ---------------------------------------------------------------------------
// 进程计量：堆（计数分配器，跨平台）；线程、RSS、上下文切换（Linux /proc，其他平台输出"—"）
// ---------------------------------------------------------------------------

struct Counting;

static HEAP: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

// @security 只转发到系统分配器并计数，不改变布局与对齐。
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let cur = HEAP.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        HEAP_PEAK.fetch_max(cur, Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        HEAP.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        let cur = HEAP.fetch_add(new_size, Ordering::Relaxed) + new_size;
        HEAP_PEAK.fetch_max(cur, Ordering::Relaxed);
        HEAP.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn heap() -> usize {
    HEAP.load(Ordering::Relaxed)
}

fn kib(delta: i64) -> String {
    format!("{:.0} KiB", delta as f64 / 1024.0)
}

fn rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()
}

fn thread_count() -> Option<usize> {
    Some(std::fs::read_dir("/proc/self/task").ok()?.count())
}

/// 全部线程的上下文切换次数（自愿 + 非自愿）。
fn context_switches() -> Option<u64> {
    let mut total = 0;
    for task in std::fs::read_dir("/proc/self/task").ok()? {
        let path = task.ok()?.path().join("status");
        let Ok(s) = std::fs::read_to_string(path) else { continue };
        for l in s.lines() {
            if l.starts_with("voluntary_ctxt_switches:") || l.starts_with("nonvoluntary_ctxt_switches:") {
                total += l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            }
        }
    }
    Some(total)
}

/// 进程 CPU 时间（用户 + 系统）。
///
/// @compat 只在 Unix 统计（getrusage）；其他平台为 `None`，报告中显示「—」。
#[cfg(unix)]
fn cpu_time() -> Option<Duration> {
    // @security 只读当前进程的资源用量。
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } != 0 {
        return None;
    }
    let tv = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
    Some(tv(ru.ru_utime) + tv(ru.ru_stime))
}

#[cfg(not(unix))]
fn cpu_time() -> Option<Duration> {
    None
}

/// 两次 [`cpu_time`] 之差按次数平均。
fn cpu_per_call(before: Option<Duration>, after: Option<Duration>, calls: u32) -> Option<Duration> {
    after.zip(before).map(|(a, b)| a.saturating_sub(b) / calls)
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or("—".into(), |v| v.to_string())
}

// ---------------------------------------------------------------------------
// 统计
// ---------------------------------------------------------------------------

fn pct(samples: &[Duration], p: f64) -> Duration {
    let mut s = samples.to_vec();
    s.sort();
    s.get(((s.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or_default()
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1e3)
}

fn row(name: &str, samples: &[Duration]) {
    println!("| {name} | {} | {} | {} | {} |", samples.len(), ms(pct(samples, 0.5)), ms(pct(samples, 0.95)), ms(pct(samples, 1.0)));
}

// ---------------------------------------------------------------------------
// App 端
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Counters {
    invoked: AtomicUsize,
    cancelled_at: Mutex<Option<Instant>>,
    started: AtomicUsize,
}

/// `bench.echo`：原样返回参数（另起线程完成，模拟 App 主线程外的工作）。
struct Echo(Arc<Counters>);
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        self.0.invoked.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let args = call.arguments_json();
            let _ = call.complete(Some(&args), vec![]);
        });
    }
}

struct RecordCancel(Arc<Counters>, CallHandle);
impl CancelListener for RecordCancel {
    fn on_cancel(&self, _reason: CancelReason) {
        *self.0.cancelled_at.lock().unwrap() = Some(Instant::now());
        let _ = self.1.fail(ErrorKind::Cancelled, "取消");
    }
}

/// `bench.hang`：不完成，等取消。
struct Hang(Arc<Counters>);
impl ToolHandler for Hang {
    fn invoke(&self, call: CallHandle) {
        self.0.started.fetch_add(1, Ordering::SeqCst);
        call.set_cancel_listener(Arc::new(RecordCancel(self.0.clone(), call.clone())));
    }
}

fn config(hub: &Hub, instance: &str, mode: LifecycleMode) -> NativeConfig {
    let mut c = NativeConfig::new("bench", "基准");
    c.host_url = format!("ws://{}/app", hub.listen_addr().expect("listen addr"));
    c.instance_id = Some(instance.to_owned());
    c.launch_token = Some(String::new());
    c.lifecycle.mode = mode;
    c.lifecycle.idle_timeout_ms = 100;
    c.lifecycle.merge_window_ms = 100;
    c.lifecycle.wake = Some(WakeDescriptor { kind: WakeKind::Uri, target: Some("bench-app".into()), background: true });
    c
}

fn client(c: NativeConfig, counters: &Arc<Counters>) -> NativeClient {
    let client = NativeClient::new(c, None).expect("native client");
    let mut echo = ToolSpec::new("echo", "原样返回");
    echo.input_schema_json = Some(r#"{"type":"object"}"#.into());
    client.register_tool(echo, Arc::new(Echo(counters.clone()))).expect("register echo");
    let mut hang = ToolSpec::new("hang", "等待取消");
    hang.input_schema_json = Some(r#"{"type":"object"}"#.into());
    client.register_tool(hang, Arc::new(Hang(counters.clone()))).expect("register hang");
    for i in 0..14 {
        let mut spec = ToolSpec::new(format!("tool.n{i:02}"), format!("第 {i} 个工具"));
        spec.input_schema_json =
            Some(r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}}}"#.into());
        client.register_tool(spec, Arc::new(Echo(counters.clone()))).expect("register");
    }
    client
}

async fn wait_status(client: &NativeClient, status: StateStatus) -> Duration {
    let t = Instant::now();
    while client.state().status != status {
        assert!(t.elapsed() < T, "等待客户端进入 {status:?} 超时（当前 {:?}）", client.state().status);
        tokio::time::sleep(Duration::from_micros(500)).await;
    }
    t.elapsed()
}

/// 唤醒器：有客户端时交给它 `handle_wake`（热唤醒）；没有时新建客户端（进程内冷唤醒，带唤醒令牌）。
struct BenchWaker {
    hub_addr: String,
    counters: Arc<Counters>,
    client: Mutex<Option<NativeClient>>,
    wakes: AtomicUsize,
}

#[async_trait]
impl Waker for BenchWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        let mut slot = self.client.lock().unwrap();
        match slot.as_ref() {
            Some(c) => {
                c.handle_wake(&req.activation_arg);
            }
            None => {
                let mut cfg = NativeConfig::new("bench", "基准");
                cfg.host_url = self.hub_addr.clone();
                cfg.instance_id = req.instance_id.clone();
                cfg.launch_token = Some(req.token.clone());
                cfg.lifecycle.mode = LifecycleMode::Idle;
                cfg.lifecycle.idle_timeout_ms = 100;
                cfg.lifecycle.merge_window_ms = 100;
                cfg.lifecycle.wake =
                    Some(WakeDescriptor { kind: WakeKind::Uri, target: Some("bench-app".into()), background: true });
                let c = client(cfg, &self.counters);
                c.start();
                *slot = Some(c);
            }
        }
        Ok(())
    }
}

fn hub_config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        lease_ttl: Duration::ZERO,
        wake_timeout: Duration::from_secs(5),
        // 基准连续调用 / 唤醒：放宽唤醒上限、关闭调用限流（其余上限保持默认）。
        wake_rate_limit: 1000,
        limits: LimitPolicy { tool_rate: RateLimit::UNLIMITED, app_rate: RateLimit::UNLIMITED, ..Default::default() },
        ..Default::default()
    }
}

/// 约 `target` 字节的对象数组参数（与 core 热路径基准同一形态）。
fn payload_objects(target: usize) -> Value {
    let mut items = Vec::new();
    let mut len = 0;
    let mut i = 0u64;
    while len < target {
        let item = json!({"id": i, "sku": format!("SKU-{i:08}"), "name": format!("商品 {i} 名称"),
            "price": (i % 1000) as f64 + 0.5, "qty": i % 7, "tags": ["a", "bb", "ccc"], "inStock": i % 2 == 0});
        len += item.to_string().len() + 1;
        items.push(item);
        i += 1;
    }
    json!({ "items": items })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "基准：手动运行（--ignored --nocapture，建议 --release）"]
async fn lifecycle_metrics() {
    println!("\n# 生命周期指标（进程内 Hub + native 客户端，回环 WebSocket）\n");
    println!("profile = {}", if cfg!(debug_assertions) { "debug" } else { "release" });

    // ---- 热调用（persistent，已连接）----------------------------------------
    let hub = Arc::new(Hub::start(hub_config()).await.expect("hub"));
    let counters = Arc::new(Counters::default());

    // ---- 每个客户端的常驻开销 --------------------------------------------------
    // 最先测（之前没有大调用，RSS 不受分配器回收影响）。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (th0, heap0) = (thread_count(), heap());
    let clients: Vec<NativeClient> = (0..CLIENTS)
        .map(|i| {
            let c = client(config(&hub, &format!("multi-{i}"), LifecycleMode::Persistent), &counters);
            c.start();
            c
        })
        .collect();
    for c in &clients {
        wait_status(c, StateStatus::Connected).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (th1, heap1) = (thread_count(), heap());
    let cs_a = context_switches();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let connected_cs = cs_a.zip(context_switches()).map(|(a, b)| b.saturating_sub(a));
    for c in &clients {
        c.sleep();
    }
    for c in &clients {
        wait_status(c, StateStatus::Dormant).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (th2, heap2) = (thread_count(), heap());
    let per = |a: Option<usize>, b: Option<usize>| a.zip(b).map(|(a, b)| format!("{:.1}", (b as f64 - a as f64) / CLIENTS as f64));
    let per_heap = |a: usize, b: usize| kib((b as i64 - a as i64) / CLIENTS as i64);
    for c in &clients {
        c.stop();
    }
    drop(clients);

    let warm = client(config(&hub, "warm-1", LifecycleMode::Persistent), &counters);
    warm.start();
    wait_status(&warm, StateStatus::Connected).await;
    let call = |name: &str, args: Value| hub.call_tool(CallRequest::new(name, args));
    // 等工具出现
    let t = Instant::now();
    while call("bench.echo", json!({})).await.is_err() {
        assert!(t.elapsed() < T, "工具未出现");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    println!("\n| 项 | 次数 | P50 | P95 | 最大 |\n|---|---|---|---|---|");
    let small = json!({"a": 20, "b": 22, "note": "hello"});
    let mut lat = Vec::new();
    let cpu0 = cpu_time();
    for _ in 0..CALLS {
        let t = Instant::now();
        let out = call("bench.echo", small.clone()).await.expect("call");
        lat.push(t.elapsed());
        assert_eq!(out.result.expect("ok"), small);
    }
    let cpu_small = cpu_per_call(cpu0, cpu_time(), CALLS as u32);
    row("热调用：小参数（已连接）", &lat);

    // Hub 默认参数上限 1 MiB（spec/hub-api.md 3.11）：取略小于上限的大小。
    let big = payload_objects(1000 * 1024);
    let big_len = big.to_string().len();
    let mut lat = Vec::new();
    let cpu0 = cpu_time();
    let rss0 = rss_kib();
    let heap_before = heap();
    let mut peak = 0;
    for _ in 0..10 {
        HEAP_PEAK.store(heap(), Ordering::Relaxed);
        let base = heap();
        let t = Instant::now();
        let out = call("bench.echo", big.clone()).await.expect("call");
        lat.push(t.elapsed());
        assert_eq!(out.result.map(|v| v.to_string().len()), Ok(big_len));
        peak = peak.max(HEAP_PEAK.load(Ordering::Relaxed).saturating_sub(base));
    }
    let heap_retained = heap() as i64 - heap_before as i64;
    let cpu_big = cpu_per_call(cpu0, cpu_time(), 10);
    let rss_big = rss_kib().zip(rss0).map(|(a, b)| a.saturating_sub(b));
    row(&format!("热调用：{:.2} MiB 对象数组参数 + 同样大小结果", big_len as f64 / (1 << 20) as f64), &lat);

    // ---- 取消延迟 -------------------------------------------------------------
    let mut cancel_handler = Vec::new();
    let mut cancel_caller = Vec::new();
    for i in 0..50 {
        let before = counters.started.load(Ordering::SeqCst);
        *counters.cancelled_at.lock().unwrap() = None;
        let id = format!("cancel-{i}");
        let mut req = CallRequest::new("bench.hang", json!({}));
        req.call_id = Some(id.clone());
        let h = hub.clone();
        let task = tokio::spawn(async move { h.call_tool(req).await.map(|o| o.result.is_ok()) });
        let t = Instant::now();
        while counters.started.load(Ordering::SeqCst) == before {
            assert!(t.elapsed() < T, "hang 未开始");
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
        let t0 = Instant::now();
        hub.cancel_call(&id);
        let res = tokio::time::timeout(T, task).await.expect("取消后调用应返回").expect("join");
        cancel_caller.push(t0.elapsed());
        assert_ne!(res.ok(), Some(true), "被取消的调用不应成功");
        let t = Instant::now();
        let at = loop {
            if let Some(at) = *counters.cancelled_at.lock().unwrap() {
                break at;
            }
            assert!(t.elapsed() < T, "handler 未收到取消");
            tokio::time::sleep(Duration::from_micros(200)).await;
        };
        cancel_handler.push(at.saturating_duration_since(t0));
    }
    row("取消：Hub cancel_call → handler 收到取消", &cancel_handler);
    row("取消：Hub cancel_call → 调用方返回", &cancel_caller);
    warm.stop();
    drop(warm);

    // ---- 热唤醒 / 释放（idle，合并窗口 100 ms，租约 0）-------------------------
    let waker = Arc::new(BenchWaker {
        hub_addr: format!("ws://{}/app", hub.listen_addr().expect("addr")),
        counters: counters.clone(),
        client: Mutex::new(None),
        wakes: AtomicUsize::new(0),
    });
    hub.set_waker(waker.clone());
    let idle = client(config(&hub, "idle-1", LifecycleMode::Idle), &counters);
    *waker.client.lock().unwrap() = Some(idle.clone());
    idle.start();
    wait_status(&idle, StateStatus::Connected).await;
    wait_status(&idle, StateStatus::Dormant).await;
    let mut hot = Vec::new();
    let mut release = Vec::new();
    let invoked0 = counters.invoked.load(Ordering::SeqCst);
    for _ in 0..WAKE_ROUNDS {
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(idle.state().status, StateStatus::Dormant);
        let t = Instant::now();
        let out = call("bench.echo", small.clone()).await.expect("热唤醒调用");
        hot.push(t.elapsed());
        assert_eq!(out.result.expect("ok"), small);
        release.push(wait_status(&idle, StateStatus::Dormant).await);
    }
    row("热唤醒：休眠实例 → 回连 + 快速恢复 + 调用返回", &hot);
    row("释放：调用返回 → 客户端休眠（合并窗口 100 ms）", &release);
    let hot_invoked = counters.invoked.load(Ordering::SeqCst) - invoked0;

    // 休眠期间的空闲唤醒（上下文切换）：Hub + 1 个休眠客户端
    let cs0 = context_switches();
    let cpu0 = cpu_time();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let idle_cs = cs0.zip(context_switches()).map(|(a, b)| b.saturating_sub(a));
    let idle_cpu = cpu_per_call(cpu0, cpu_time(), 1);

    // ---- 进程内冷唤醒：休眠后实例被回收（客户端停止并丢弃），新客户端带令牌回连 ------
    let mut cold = Vec::new();
    let mut cold_ok = 0;
    let invoked1 = counters.invoked.load(Ordering::SeqCst);
    for _ in 0..WAKE_ROUNDS {
        let old = waker.client.lock().unwrap().take();
        if let Some(c) = old {
            wait_status(&c, StateStatus::Dormant).await;
            c.stop();
        }
        let t = Instant::now();
        let res = call("bench.echo", small.clone()).await;
        cold.push(t.elapsed());
        if res.is_ok_and(|o| o.result.as_ref().is_ok_and(|v| v == &small)) {
            cold_ok += 1;
        }
    }
    row("冷唤醒（进程内）：实例已回收 → 新客户端 + 完整同步 + 调用返回", &cold);
    let cold_invoked = counters.invoked.load(Ordering::SeqCst) - invoked1;
    if let Some(c) = waker.client.lock().unwrap().take() {
        c.stop();
    }


    println!("\n| 资源 | 值 |\n|---|---|");
    println!("| 单次小调用 CPU（Hub + 客户端，进程合计） | {} |", cpu_small.map_or("—".into(), ms));
    println!("| 单次 1 MiB 调用 CPU（Hub + 客户端，进程合计） | {} |", cpu_big.map_or("—".into(), ms));
    println!("| 单次 1 MiB 调用峰值堆增量（Hub + 客户端） | {} |", kib(peak as i64));
    println!("| 10 次 1 MiB 调用后保留的堆（去重表等） | {} |", kib(heap_retained));
    println!("| 10 次 1 MiB 调用后 RSS 增量（含分配器保留） | {} |", opt(rss_big.map(|k| format!("{k} KiB"))));
    println!("| 每个已连接客户端：线程 | {} |", opt(per(th0, th1)));
    println!("| 每个已连接客户端：堆（含 Hub 侧连接） | {} |", per_heap(heap0, heap1));
    println!("| 每个休眠客户端：线程 | {} |", opt(per(th0, th2)));
    println!("| 每个休眠客户端：堆（含 Hub 侧休眠快照） | {} |", per_heap(heap0, heap2));
    println!("| {CLIENTS} 个已连接客户端（本地回环、无心跳）3 s 上下文切换 | {} |", opt(connected_cs));
    println!("| Hub + 1 个休眠客户端 3 s 上下文切换 / CPU | {} / {} |", opt(idle_cs), idle_cpu.map_or("—".into(), ms));
    println!("| 唤醒器调用次数（热 {WAKE_ROUNDS} 轮 + 冷 {WAKE_ROUNDS} 轮） | {} |", waker.wakes.load(Ordering::SeqCst));
    println!("| 冷唤醒恢复率 | {cold_ok}/{WAKE_ROUNDS} |");
    println!(
        "| 重复执行（handler 执行次数 − 调用次数） | 热 {} / 冷 {} |",
        hot_invoked as i64 - WAKE_ROUNDS as i64,
        cold_invoked as i64 - WAKE_ROUNDS as i64
    );

    if let Ok(hub) = Arc::try_unwrap(hub) {
        hub.shutdown().await;
    }
}
