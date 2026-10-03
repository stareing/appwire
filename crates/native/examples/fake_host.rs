//! 假 Host：供各语言绑定做集成测试。
//!
//! ```text
//! cargo run -p app-mcp-native --example fake_host -- [--addr 127.0.0.1:0 | --ipc <端点>] \
//!     [--invoke <tool> --args '<json>']... [--read <resource>]... \
//!     [--await-sleep] [--wake] ... [--lease-ms <ms>] [--reject-sleep <retryAfterMs>] [--timeout-ms 10000]
//! ```
//!
//! - 绑定后立即向 stdout 打印 `LISTENING <地址>`。默认监听 TCP（`--addr`，打印 `host:port`）；
//!   `--ipc unix:<绝对路径>`（Unix）/ `--ipc pipe:\\.\pipe\<名称>`（Windows）改为监听本地 IPC，
//!   打印端点字符串本身（如 `LISTENING unix:/tmp/x/hub.sock`），可直接作为 SDK 的 Host 地址。
//!   `--addr` 与 `--ipc` 互斥。IPC 端点由测试方指定，不要用平台默认端点（常驻 Host 可能正在用）。
//! - 接受 WebSocket 连接，完成握手（直接 `paired`），回复 `ping`。
//! - 收到 `app/ready` 后打印当前工具与资源列表，然后按命令行顺序依次执行操作，每步打印一行 JSON：
//!   - `--invoke` / `--read`：发送请求，打印结果（`{"type":"invoke"|"read", "name", "result"|"error"}`）。
//!   - `--await-sleep`：等待 SDK 发送 `app/sleep` 并接受（返回 `resumeToken`），打印
//!     `{"type":"sleep","accepted":true,"reason","toolsHash"}`；随后 SDK 关闭连接。
//!     等待期间收到的 `app/sleep` 之外，其他时候收到的 `app/sleep` 一律以 `retryAfterMs: 200` 拒绝（不打印）。
//!   - `--wake`（必须在 `--await-sleep` 之后）：生成一次性唤醒令牌，打印
//!     `{"type":"wake","token","arg":"app-mcp-wake:<token>"}`，测试方把 `arg` 交给 SDK 的 `handleWake`；
//!     然后接受下一个连接，打印 `{"type":"hello","launchToken","resumeToken","wakeReason","toolsCurrent"}`，
//!     恢复令牌与 `toolsHash` 都与休眠时一致时返回 `toolsCurrent: true`（沿用工具快照）。
//!     该连接的 `app/ready` 之后照常打印工具列表，并附带 `"synced"`（本连接是否收到了 `tools/sync`）。
//! - 收到 `tools/progress` 时打印 `{"type":"progress","callId","progress","total"?,"message"?}`（在对应调用结果之前）。
//! - `--tool-info`：工具列表行另带 `"toolInfo": { <名称>: { risk, annotations?, outputSchema? } }`（核对工具声明）。
//! - `--lease-ms <ms>`：`app/ready` 之后与每个操作完成后发送 `app/lease { ttlMs }`。
//! - `--reject-sleep <ms>`：`--await-sleep` 期间收到的第一个 `app/sleep` 以 `retryAfterMs: <ms>` 拒绝，打印
//!   `{"type":"sleep","accepted":false,"reason","toolsHash"}`。
//! - 全部完成后关闭连接（最后一步是 `--await-sleep` 时由 SDK 关闭），退出码 0；超时退出码 2；
//!   其他错误退出码 1（信息写 stderr）。
//!
//! 一致性测试（conformance/README.md）用到的附加参数（都是可选的新增，不影响上面的行为）：
//!
//! - `--invoke` 之后的修饰：`--call-id <id>`（指定 `callId`，缺省 `c<序号>`）、`--invoke-timeout-ms <ms>`（缺省 5000）、
//!   `--cancel-after-ms <ms>`（发出调用后该时间仍未收到结果则发送 `tools/cancel`）、`--idempotency-key <key>`
//!   （`ToolsInvokeParams.idempotencyKey`，spec/protocol.md 3.3）、`--no-wait`（发出后立即执行下一步，不等结果；结果到达时
//!   照常打印，全部操作完成后等齐未回复的调用再关闭连接；用于核对 SDK 的调用调度，spec/protocol.md 5.3）。
//! - `--catalog <settleMs>`：继续处理消息 settleMs 后打印
//!   `{"type":"catalog","tools":{<名称>:ToolInfo},"resources":{<名称>:ResourceInfo},"toolsHash"}`（`toolsHash` 由 Host 按 8.4 计算）。
//! - `--delay <ms>`：继续处理消息 ms 后再执行下一步。
//! - `--navigate <page>`（可跟 `--nav-params '<json>'`）：发送 `app/navigate`（spec/protocol.md 3.4），打印
//!   `{"type":"navigate","name":<page>,"result"|"error"}`。
//! - `--trace`：SDK 发来的每个请求与通知（`ping` 除外）打印为 `{"type":"recv","method","params"}`；
//!   发出 `tools/cancel` 时打印 `{"type":"cancel","callId"}`。
//! - `--case <用例.json>`：从用例的 `host` 部分取参数（命令行上的其他参数追加在后），结束时按用例的期望核对，
//!   打印 `{"type":"verdict",...}`；`--sdk <名称>` 标明被测 SDK（查 `conformance/divergences/<sdk>.json` 中的已知偏差），
//!   `--report-dir <目录>` 写 `<目录>/<sdk>/<用例>.json`；`--skip <原因>` 不监听，只记录跳过。
//!   核对不通过时退出码 3（已登记的偏差为 0）。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

#[path = "support/conformance.rs"]
mod conformance;
// @why 示例的入口文件按 mod-rs 规则解析子模块（相对 examples/）；保留 `fake_host.rs` 入口路径（packages/node、sdks/cpp 的集成测试据此判断是否在仓库内），子模块放在 `fake_host/` 下并用 `#[path]` 指定。
#[path = "fake_host/args.rs"]
mod args;
#[path = "fake_host/listener.rs"]
mod listener;
#[path = "fake_host/session.rs"]
mod session;
#[cfg(test)]
#[path = "fake_host/tests.rs"]
mod tests;

use args::parse_args;
use session::run;

use app_mcp_protocol::{
    self as proto, Endpoint, HelloParams, HelloResult, LeaseParams, Message, PairingStatus, RequestId,
    ResourcesChangedParams, ResourcesReadParams, ResourcesSyncParams, RpcError, SleepParams,
    SleepResult, ToolsCancelParams, ToolsChangedParams, ToolsInvokeParams, ToolsSyncParams, method,
};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Invoke { name: String, args: Value, opts: InvokeOpts },
    Read { name: String },
    AwaitSleep,
    Wake,
    /// 继续处理消息 `settle_ms` 后打印当前目录（`--catalog`）。
    Catalog { settle_ms: u64 },
    /// 继续处理消息 `ms` 后再执行下一步（`--delay`）。
    Delay { ms: u64 },
    /// 发送 `app/navigate`（`--navigate`，可跟 `--nav-params`）。
    Navigate { page: String, params: Option<Value> },
}

/// `--invoke` 的修饰参数。
#[derive(Debug, Clone, Default, PartialEq)]
struct InvokeOpts {
    call_id: Option<String>,
    timeout_ms: Option<u64>,
    cancel_after_ms: Option<u64>,
    idempotency_key: Option<String>,
    /// `--priority`：原样写入 `tools/invoke` 参数 `priority`（可发不认识的取值，检验 SDK 的宽松解析）。
    priority: Option<String>,
    /// `--no-wait`：发出后不等结果就执行下一步（结果到达时照常打印；全部操作完成后等齐再关闭连接）。
    no_wait: bool,
}

#[derive(Debug)]
struct Options {
    /// TCP 监听地址；`None` 时为 [`DEFAULT_ADDR`]（未指定 `--ipc` 时）。
    addr: Option<String>,
    /// 本地 IPC 端点（`--ipc`）。
    ipc: Option<Endpoint>,
    ops: Vec<Op>,
    timeout_ms: u64,
    lease_ms: Option<u64>,
    reject_sleep_ms: Option<u64>,
    tool_info: bool,
    trace: bool,
    /// 一致性用例（`--case`）及其附带参数。
    case: Option<PathBuf>,
    sdk: Option<String>,
    report_dir: Option<PathBuf>,
    skip: Option<String>,
}

const DEFAULT_ADDR: &str = "127.0.0.1:0";

/// 一致性用例模式下记录的输出行（`LISTENING` 之后打印的每一行）；`None` = 不记录。
static RECORD: Mutex<Option<Vec<Value>>> = Mutex::new(None);

/// 输出一行到 stdout 并 flush。
fn emit(line: &str) {
    if let Ok(mut rec) = RECORD.lock()
        && let Some(lines) = rec.as_mut()
    {
        lines.push(serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_owned())));
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// 命令行参数；带 `--case` 时用例的 `host` 部分排在前面（命令行上的同名参数后出现，覆盖之）。
fn load_args() -> Result<(Options, Option<conformance::Case>), String> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let case_path = raw.iter().position(|a| a == "--case").and_then(|i| raw.get(i + 1));
    let Some(path) = case_path else {
        return Ok((parse_args(raw)?, None));
    };
    let case = conformance::Case::load(std::path::Path::new(path))?;
    let mut args = case.host_args()?;
    args.extend(raw);
    Ok((parse_args(args)?, Some(case)))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let (opts, case) = match load_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("fake_host: {e}");
            return ExitCode::from(1);
        }
    };
    let Some(case) = case else {
        return match serve_once(opts).await {
            Ok(()) => ExitCode::SUCCESS,
            Err((code, e)) => {
                eprintln!("fake_host: {e}");
                ExitCode::from(code)
            }
        };
    };
    run_case(opts, case).await
}

/// 监听并按参数执行全部操作。
///
/// @error `(退出码, 说明)`：1 = 绑定 / 协议错误，2 = 超时。
async fn serve_once(opts: Options) -> Result<(), (u8, String)> {
    let (listener, local) = Listener::bind(&opts).await.map_err(|e| (1, e))?;
    emit(&format!("LISTENING {local}"));
    if let Ok(mut rec) = RECORD.lock()
        && let Some(lines) = rec.as_mut()
    {
        lines.clear();
    }
    let timeout = Duration::from_millis(opts.timeout_ms);
    match tokio::time::timeout(timeout, run(listener, opts)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err((1, e)),
        Err(_) => Err((2, format!("超过 {} ms 仍未完成", timeout.as_millis()))),
    }
}

/// 一致性用例：运行后核对、打印结论、写报告。
async fn run_case(mut opts: Options, case: conformance::Case) -> ExitCode {
    let sdk = opts.sdk.take();
    let report_dir = opts.report_dir.take();
    let (verdict, lines) = match opts.skip.take() {
        Some(reason) => (conformance::skip_verdict(&case, sdk.as_deref(), &reason), Vec::new()),
        None => {
            if let Ok(mut rec) = RECORD.lock() {
                *rec = Some(Vec::new());
            }
            let run_error = serve_once(opts).await.err().map(|(_, e)| e);
            let lines = RECORD.lock().ok().and_then(|mut r| r.take()).unwrap_or_default();
            (conformance::verdict(&case, sdk.as_deref(), run_error, &lines), lines)
        }
    };
    emit(&verdict.to_string());
    if let Some(dir) = report_dir {
        let sdk_name = sdk.as_deref().unwrap_or("unknown");
        if let Err(e) = conformance::write_report(&dir, sdk_name, &case, &verdict, &lines) {
            eprintln!("fake_host: {e}");
        }
    }
    if verdict["status"] == "fail" {
        for f in verdict["failures"].as_array().into_iter().flatten() {
            eprintln!("fake_host: [{}] {}", case.id, f.as_str().unwrap_or_default());
        }
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

/// 一个已接受连接的字节流（TCP / Unix 域套接字 / 命名管道）。
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// 监听位置：TCP（默认）或本地 IPC（`--ipc`）。IPC 上跑的仍是同一套 WebSocket 帧（spec/protocol.md 1.2）。
///
/// @why 不复用 app-mcp-hub 的 IPC 监听：那是 Hub 的 crate 内部实现（单实例判断、残留清理、对端鉴权），
///      native 不依赖 Hub；这里只需在测试指定的全新路径上监听，端点解析、长度检查与管道安全描述符
///      复用 app-mcp-protocol。
enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixSocket),
    #[cfg(windows)]
    Pipe(PipeListener),
}

/// Unix 域套接字监听；Drop 时删除套接字文件。
#[cfg(unix)]
struct UnixSocket {
    listener: tokio::net::UnixListener,
    path: std::path::PathBuf,
}

/// Windows 命名管道监听：始终保留一个等待连接的实例（SDK 断开后可回连，如 `--wake`）。
///
/// @security 安全描述符与 Hub 相同（只允许当前用户、所有者为当前用户；SDK 连接时校验所有者），拒绝远程客户端。
#[cfg(windows)]
struct PipeListener {
    name: String,
    security: proto::endpoint::win::PipeSecurity,
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

/// 当前 SDK 同步过来的工具与资源（已按名称排序）。休眠期间作为快照保留。
#[derive(Default)]
struct Catalog {
    tools: BTreeMap<String, proto::ToolInfo>,
    resources: BTreeMap<String, proto::ResourceInfo>,
}

/// 等待中的定时动作（`--cancel-after-ms`、`--catalog`、`--delay`）。
enum Timer {
    Cancel(String),
    Catalog,
    Delay,
}

/// 跨连接的 Host 状态。
struct HostState {
    ops: std::vec::IntoIter<Op>,
    lease_ms: Option<u64>,
    /// 还需要拒绝的休眠次数（`--reject-sleep`）。
    reject_sleep_ms: Option<u64>,
    tool_info: bool,
    trace: bool,
    catalog: Catalog,
    /// 休眠被接受时发放的恢复令牌与当时的 toolsHash。
    resume: Option<(String, String)>,
    /// 等待回连的唤醒令牌。
    wake_token: Option<String>,
    connections: u32,
    next_call: u32,
    next_request: i64,
}

/// 一个连接结束的方式。
enum ConnEnd {
    /// 全部操作完成。
    Done,
    /// 休眠被接受、SDK 已断开，等待唤醒回连。
    Slept,
}
