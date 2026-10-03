//! 运行中 Host 状态的检查：App 实例、唤醒、休眠记录、租约、工具声明、资源保护与 SDK 上报。

use std::path::Path;

use app_mcp_hub::call_objects::CallState;
use app_mcp_hub::{AppState, AwakeReason, HubStatus, InstancePower, LeaseStatus, ToolAnnotations};
use app_mcp_protocol::{ConnectionErrorCode, ErrorKind};
use serde_json::{Value, json};

use super::{Check, Level};

/// 调用方计数（`status` 一行摘要与 doctor 共用）：MCP 会话数，以及 `subscriptions/listen` 流数、Agent 任务数、持有中的对象锁
/// 与进行中的调用（spec/hub-api.md 3.6；旧 Host 不报告时省略，没有时不提；调用最多列 5 个）。
pub(crate) fn callers_text(st: &HubStatus) -> String {
    let listen = st.mcp_listen_streams.map(|n| format!("、listen 流 {n} 个")).unwrap_or_default();
    let tasks = st.tasks.as_ref().map(|t| format!("、Agent 任务 {} 个", t.len())).unwrap_or_default();
    let locks = match st.locks.as_deref() {
        Some([]) | None => String::new(),
        Some(l) => {
            let mut held: Vec<String> = l.iter().map(|k| format!("{}（{}）", k.app_id, k.holder)).collect();
            held.dedup();
            format!("、对象锁 {} 把：{}", l.len(), held.join("、"))
        }
    };
    let calls = match st.calls.as_deref() {
        Some([]) | None => String::new(),
        Some(c) => {
            let shown: Vec<String> = c
                .iter()
                .take(5)
                .map(|k| format!("{}（{}，{}，{:.1} s）", k.name, call_state_text(k.state), k.subject, k.elapsed_ms as f64 / 1000.0))
                .collect();
            let more = if c.len() > shown.len() { "等".to_owned() } else { String::new() };
            format!("、进行中调用 {} 个：{}{more}", c.len(), shown.join("、"))
        }
    };
    format!("MCP 会话 {} 个{listen}{tasks}{locks}{calls}{}", st.mcp_sessions, events_text(st))
}

/// 事件订阅（第 16 项 N3）：订阅数、各信箱积压合计、丢弃合计；没有订阅且没有丢弃时不提。
fn events_text(st: &HubStatus) -> String {
    let Some(ev) = st.events.as_ref() else { return String::new() };
    let mut inboxes: Vec<(&str, usize)> = ev.subscriptions.iter().map(|s| (s.subscriber.as_str(), s.pending)).collect();
    inboxes.sort_unstable();
    inboxes.dedup();
    let pending: usize = inboxes.iter().map(|(_, n)| n).sum();
    let dropped = ev.dropped_invalid + ev.subscriptions.iter().map(|s| s.dropped).sum::<u64>();
    if ev.subscriptions.is_empty() && dropped == 0 {
        return String::new();
    }
    let dropped = if dropped > 0 { format!("，丢弃 {dropped} 条") } else { String::new() };
    format!("、事件订阅 {} 个（信箱积压 {pending} 条{dropped}）", ev.subscriptions.len())
}

fn call_state_text(s: CallState) -> &'static str {
    match s {
        CallState::Created => "已受理",
        CallState::Approving => "等待确认",
        CallState::Activating => "唤醒中",
        CallState::Running => "执行中",
    }
}

pub(super) fn apps_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "App 实例";
    let st = match status {
        None => return Check::new("apps", T, Level::Skip, "Host 未运行"),
        Some(Err(e)) => {
            return Check::new("apps", T, Level::Warn, format!("无法读取运行中 Host 的 /status：{e}"))
                .hint("经本地 IPC 或令牌访问 /status；Host 版本过旧时请升级");
        }
        Some(Ok(st)) => st,
    };
    let details = serde_json::to_value(&st.apps).unwrap_or(Value::Null);
    if st.apps.is_empty() {
        return Check::new("apps", T, Level::Info, format!("没有已知 App（{}）", callers_text(st)))
            .hint("启动接入了 SDK 的 App，或用 --manifest 加载静态清单")
            .details(details);
    }
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for a in &st.apps {
        let state = match a.state {
            AppState::Connected => "在线",
            AppState::Waking => "唤醒中",
            AppState::Dormant => "休眠",
            AppState::Disconnected => "未连接",
        };
        let cids: Vec<&str> = a.instances.iter().filter_map(|i| i.info.connection_id.as_deref()).collect();
        let cid = if cids.is_empty() { String::new() } else { format!("，连接 {}", cids.join("/")) };
        let wakes = if a.wakes > 0 { format!("，唤醒 {} 次", a.wakes) } else { String::new() };
        lines.push(format!("{}：{state}（实例 {}{cid}{wakes}）", a.app_id, a.instances.len()));
        for i in &a.instances {
            if let Some(p) = &i.power {
                lines.push(format!("{}/{} {}", a.app_id, i.info.instance_id, power_text(p)));
            }
        }
        if let Some(e) = &a.last_error {
            errors.push(format!("{}：[{}] {}", a.app_id, e.code.as_deref().unwrap_or("-"), e.message));
        }
    }
    let summary = format!("{}；{}", lines.join("；"), callers_text(st));
    if errors.is_empty() {
        Check::new("apps", T, Level::Ok, summary).details(details)
    } else {
        Check::new("apps", T, Level::Warn, format!("{summary}。最近错误：{}", errors.join("；")))
            .hint("按错误码处理（spec/protocol.md 10.1）；唤醒失败时检查清单的 wake 配置与 App 是否已安装")
            .details(details)
    }
}

/// 视为"唤醒未完成"的最近错误类别：激活命令失败（含单实例转交超时后第二实例非 0 退出）或唤醒后未回连。
pub(super) const WAKE_FAILURE_KINDS: [ErrorKind; 2] = [ErrorKind::LaunchFailed, ErrorKind::AppNotResponding];

pub(super) const WAKE_FAILURE_HINT: &str = "App 未运行时检查清单的 wake 配置与 App 是否已安装；App 进程在运行却唤醒失败时，\
多半是它没能及时处理激活：检查该进程的优先级是否被设为「低」（IDLE）或处于 Windows 效率模式（任务管理器 → 详细信息 / 进程右键），\
以及系统 CPU 是否满载；负载下降后通常自行恢复。本库不调整进程调度，需要时由用户或 App 调整";

/// 唤醒失败排查（TASKS 4f G12）：最近错误为 `LAUNCH_FAILED` / `APP_NOT_RESPONDING` 的 App 给出修复提示。
/// 唤醒速率超限（`WAKE_RATE_LIMITED`）不在此列，见 App 实例检查。
pub(super) fn wake_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "唤醒";
    let Some(Ok(st)) = status else {
        return Check::new("wake", T, Level::Skip, "无法读取运行中 Host 的状态");
    };
    let failed: Vec<String> = st
        .apps
        .iter()
        .filter_map(|a| {
            let e = a.last_error.as_ref()?;
            let code = e.code.as_deref()?;
            WAKE_FAILURE_KINDS
                .iter()
                .any(|k| k.as_str() == code)
                .then(|| format!("{}：[{code}] {}", a.app_id, e.message))
        })
        .collect();
    if failed.is_empty() {
        return Check::new("wake", T, Level::Ok, "最近没有唤醒失败");
    }
    Check::new("wake", T, Level::Warn, format!("最近唤醒失败：{}", failed.join("；")))
        .hint(WAKE_FAILURE_HINT)
        .details(json!({ "apps": failed }))
}

/// 休眠记录持久化（spec/hub-api.md 3.5「持久化」）：离线检查 `<home>/state/dormant/` 中的文件（Host 未运行也可用），
/// 并附上运行中 Host 的写入状态。被跳过的文件（损坏、版本未知、超出上限）给出警告。
pub(super) fn dormant_store_check(state_dir: &Path, status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "休眠记录";
    let ttl = app_mcp_hub::HubConfig::default().dormant_ttl;
    let running = match status {
        Some(Ok(st)) => st.dormant_store.as_ref(),
        _ => None,
    };
    let dir = state_dir.join(app_mcp_hub::dormant_store::DORMANT_DIR);
    let files = match app_mcp_hub::dormant_store::inspect(state_dir, std::time::SystemTime::now(), ttl) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Check::new("dormant_store", T, Level::Info, format!("{} 不存在（还没有 App 休眠过）", dir.display()));
        }
        Err(e) => {
            return Check::new("dormant_store", T, Level::Warn, format!("{} 无法读取：{e}", dir.display()))
                .hint("检查目录权限；Host 读不到时重启后不能列出重启前休眠的 App（App 回连后恢复）");
        }
    };
    let details = json!({ "dir": dir.display().to_string(), "files": files, "running": running });
    let instances: u64 = files.iter().map(|f| f.instances).sum();
    let bad: Vec<String> =
        files.iter().filter_map(|f| f.problem.as_ref().map(|p| format!("{}：{p}", f.file))).collect();
    let mut summary = format!("{}：{} 个 App、{instances} 个休眠实例", dir.display(), files.len() - bad.len());
    if let Some(e) = running.and_then(|r| r.last_error.as_deref()) {
        summary.push_str(&format!("；最近写入失败：{e}"));
    }
    if bad.is_empty() && running.and_then(|r| r.last_error.as_ref()).is_none() {
        return Check::new("dormant_store", T, Level::Ok, summary).details(details);
    }
    if !bad.is_empty() {
        summary.push_str(&format!("；跳过的文件：{}", bad.join("；")));
    }
    Check::new("dormant_store", T, Level::Warn, summary)
        .hint("被跳过的文件不会读回（不影响启动，对应 App 回连后恢复）；版本未知的文件可能由更新的 Host 写入，确认无用后可删除")
        .details(details)
}

/// 租约策略与统计（spec/lifecycle.md 第 13 节 B2）。
pub(super) fn lease_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "租约";
    let Some(Ok(st)) = status else {
        return Check::new("lease", T, Level::Skip, "无法读取 Host 状态");
    };
    let Some(l) = &st.lease else {
        return Check::new("lease", T, Level::Skip, "运行中的 Host 版本不提供租约统计");
    };
    let details = serde_json::to_value(l).unwrap_or(Value::Null);
    Check::new("lease", T, Level::Info, lease_text(l)).details(details)
}

/// 各工具的声明（docs/plans/14-safety.md S5）：`risk` 与 Agent 实际看到的 MCP 注解，便于用户核对 Agent 的放行规则。
pub(super) fn tools_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "工具声明（risk 与 MCP 注解）";
    let Some(Ok(st)) = status else {
        return Check::new("tools", T, Level::Skip, "无法读取 Host 状态");
    };
    let details: serde_json::Map<String, Value> = st
        .apps
        .iter()
        .filter(|a| !a.tools.is_empty())
        .map(|a| (a.app_id.clone(), serde_json::to_value(&a.tools).unwrap_or(Value::Null)))
        .collect();
    let lines: Vec<String> = st
        .apps
        .iter()
        .flat_map(|a| {
            a.tools.iter().map(move |d| {
                let source = if d.annotations.is_some() { "已声明注解" } else { "注解按 risk 推导" };
                let output = if d.output_schema { "，有 outputSchema" } else { "" };
                let risk = serde_json::to_value(d.risk).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                format!("{}.{}：risk {risk}，{}（{source}{output}）", a.app_id, d.name, annotation_text(&d.effective))
            })
        })
        .collect();
    if lines.is_empty() {
        return Check::new("tools", T, Level::Info, "没有已知工具（或运行中的 Host 版本不提供工具声明）");
    }
    Check::new("tools", T, Level::Info, format!("{} 个工具：\n    {}", lines.len(), lines.join("\n    ")))
        .hint("本库只如实传递 App 的声明，要不要确认由 Agent 决定；可据此配置 Agent 的放行规则（按工具全名）")
        .details(Value::Object(details))
}

/// MCP 工具注解的一行文本（未声明的提示为 `-`）。
pub(super) fn annotation_text(a: &ToolAnnotations) -> String {
    let b = |v: Option<bool>| v.map_or("-".to_owned(), |v| v.to_string());
    let mut text = format!(
        "readOnlyHint={} destructiveHint={} idempotentHint={} openWorldHint={}",
        b(a.read_only_hint),
        b(a.destructive_hint),
        b(a.idempotent_hint),
        b(a.open_world_hint)
    );
    if let Some(t) = &a.title {
        text.push_str(&format!(" title=「{t}」"));
    }
    text
}

/// 资源保护（spec/hub-api.md 3.11）：限流与大小上限的策略，以及各 App 启动以来被拒绝的次数。
pub(super) fn limits_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "资源保护（限流与大小上限）";
    let Some(Ok(st)) = status else {
        return Check::new("limits", T, Level::Skip, "无法读取 Host 状态");
    };
    let Some(l) = &st.limits else {
        return Check::new("limits", T, Level::Skip, "运行中的 Host 版本不提供资源保护信息");
    };
    let n = |v: Option<u64>| match v {
        Some(0) => "不限".to_owned(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    let rate = |per_minute: Option<u32>, burst: Option<u32>| match per_minute {
        Some(0) => "不限".to_owned(),
        _ => format!("每分钟 {} 次、突发 {} 次", n(per_minute.map(u64::from)), n(burst.map(u64::from))),
    };
    let policy = format!(
        "每工具 {}；每 App {}；参数 {} 字节、结果 {} 字节、资源 {} 字节；outputSchema 不符时 {}",
        rate(l.tool_rate_per_minute, l.tool_rate_burst),
        rate(l.app_rate_per_minute, l.app_rate_burst),
        n(l.max_arguments_bytes),
        n(l.max_result_bytes),
        n(l.max_resource_bytes),
        st.output_validation
            .and_then(|v| serde_json::to_value(v).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "-".to_owned()),
    );
    let rejected: Vec<String> = st
        .apps
        .iter()
        .filter(|a| a.rate_limited > 0 || a.too_large > 0)
        .map(|a| format!("{}：限流 {} 次、超大 {} 次", a.app_id, a.rate_limited, a.too_large))
        .collect();
    let details = json!({
        "limits": l,
        "outputValidation": st.output_validation,
        "rejected": st.apps.iter().map(|a| json!({"appId": a.app_id, "rateLimited": a.rate_limited, "tooLarge": a.too_large})).collect::<Vec<_>>(),
    });
    if rejected.is_empty() {
        return Check::new("limits", T, Level::Ok, format!("{policy}；没有被拒绝的调用")).details(details);
    }
    Check::new("limits", T, Level::Warn, format!("{policy}。启动以来被拒绝：{}", rejected.join("；")))
        .hint("RATE_LIMITED / PAYLOAD_TOO_LARGE 见 spec/protocol.md 第 4 节；确属正常用量时调大 --tool-rate-limit / --app-rate-limit / --max-*-bytes")
        .details(details)
}

pub(super) fn lease_text(l: &LeaseStatus) -> String {
    let policy = match l.mode.as_str() {
        "off" => return "租约已关闭（--lease-ms 0）".to_owned(),
        "fixed" => format!("固定 {} ms（自适应已关闭）", l.default_ms),
        _ => {
            let idle = if l.idle_revoke_ms == 0 {
                "不因空闲收回".to_owned()
            } else {
                format!("会话空闲 {} ms 收回默认租约", l.idle_revoke_ms)
            };
            format!(
                "自适应：最近 {} 个间隔 p90 + {} ms，范围 [{}, {}] ms，无历史 {} ms，{idle}",
                l.window, l.margin_ms, l.min_ms, l.max_ms, l.default_ms
            )
        }
    };
    let mut text = format!(
        "{policy}；已发出 自适应 {} / 默认 {} 次，收回 会话结束 {} / 空闲 {} 次",
        l.adaptive_grants, l.default_grants, l.revoked_session_end, l.revoked_idle
    );
    let pairs: Vec<String> = l
        .pairs
        .iter()
        .map(|p| {
            let src = if p.adaptive { "统计" } else { "默认" };
            format!("{}→{} {} ms（{src}，{} 个样本）", p.session, p.app_id, p.next_ttl_ms, p.samples)
        })
        .collect();
    if !pairs.is_empty() {
        text.push_str(&format!("；当前：{}", pairs.join("、")));
    }
    text
}

/// 每实例功耗观测的一行摘要（spec/lifecycle.md 第 12 节）。
pub(super) fn power_text(p: &InstancePower) -> String {
    let heartbeat = match p.heartbeat_ms {
        None => "双向（旧 SDK）".to_owned(),
        Some(0) => "无（靠连接断开）".to_owned(),
        Some(ms) => format!("SDK 每 {ms} ms"),
    };
    let mode = p.lifecycle_mode.map_or("未声明", |m| match m {
        app_mcp_protocol::LifecycleMode::Persistent => "persistent",
        app_mcp_protocol::LifecycleMode::Idle => "idle",
        app_mcp_protocol::LifecycleMode::OnDemand => "on-demand",
    });
    let mut text = format!(
        "回连 {} 次，唤醒 {} 次，在线 {} 秒，心跳 {} 次（{heartbeat}），模式 {mode}",
        p.reconnects, p.wakes, p.online_secs, p.heartbeats
    );
    if !p.awake_reasons.is_empty() {
        let reasons: Vec<&str> = p
            .awake_reasons
            .iter()
            .map(|r| match r {
                AwakeReason::Persistent => "persistent 模式",
                AwakeReason::Call => "调用进行中",
                AwakeReason::Lease => "租约",
                AwakeReason::Subscription => "资源订阅",
                AwakeReason::WakePending => "待派发的唤醒",
            })
            .collect();
        text.push_str(&format!("，未休眠原因：{}", reasons.join("、")));
    }
    text
}

pub(super) fn reports_check(status: Option<&Result<HubStatus, String>>) -> Check {
    const T: &str = "SDK 上报（浏览器拦截等）";
    const LIMIT: &str = "被拦截期间页面无法连接 Host，只有连接恢复后才会上报；从未连上的页面请看页面自身的 state.code";
    let Some(Ok(st)) = status else {
        return Check::new("reports", T, Level::Skip, "无法读取 Host 状态");
    };
    let details = serde_json::to_value(&st.reports).unwrap_or(Value::Null);
    if st.reports.is_empty() {
        return Check::new("reports", T, Level::Ok, format!("没有收到上报（{LIMIT}）")).details(details);
    }
    let mut hints = Vec::new();
    let lines: Vec<String> = st
        .reports
        .iter()
        .map(|r| {
            if let Some(c) = ConnectionErrorCode::parse(&r.code)
                && !hints.contains(&c.hint())
            {
                hints.push(c.hint());
            }
            format!("{}（{}，连接 {}）：[{}] {} ×{}", r.app_id, r.instance_id, r.connection_id, r.code, r.message, r.count)
        })
        .collect();
    Check::new("reports", T, Level::Warn, format!("最近 {} 条：{}（{LIMIT}）", lines.len(), lines.join("；")))
        .hint(hints.join("；"))
        .details(details)
}
