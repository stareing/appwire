//! Hub SDK 的 Node 绑定（napi-rs），供 `@app-mcp/hub` 使用（spec/hub-api.md）。
//!
//! # 形态
//!
//! 复杂结构一律以 JSON 文本进出（与 C ABI 一致，避免 ABI 膨胀）：`crates/hub` 的公开类型均为
//! serde camelCase，TS 封装层（`packages/hub`）负责 `JSON.parse` 与类型声明。
//!
//! # 运行时与线程
//!
//! 使用 napi-rs 的全局 tokio 多线程运行时：`Hub.start` 是 async 方法，Hub 的后台任务
//! （App 连接服务、上游、通知合并）都跑在该运行时上。JS 回调（事件、审批、配对）包装为
//! **weak** [`ThreadsafeFunction`]，在 Node 事件循环上执行，不阻止进程退出。
//!
//! 审批 / 配对回调的 JS 函数必须返回 `Promise<boolean>`（TS 封装层把同步返回值、抛错、reject
//! 统一规整为 Promise：抛错或 reject 视为拒绝）；Rust 侧 `call_async` 得到 Promise 后再等待其结果。
//!
//! # 错误
//!
//! 抛出的 JS `Error` 的消息形如 `[CODE] 说明`（TS 封装层转为 `HubError`）：Hub 错误的 CODE 为协议错误类别（如 `TOOL_NOT_FOUND`），
//! 参数不合法为 `INVALID_ARG`，Hub 已关闭为 `SHUTDOWN`，启动失败为 `START_FAILED`。

#![deny(clippy::all)]

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use app_mcp_hub::{
    AgentCredential, AgentsConfig, AppEvent, ApprovalHandler, ApprovalRequest, CallRequest, ErrorKind, EventHandler, Hub, HubError, PairingHandler,
    PairingRequest, PolicyConfig, ToolFilter, ToolFormat, WakeRequest, Waker, async_trait,
};
use napi::bindgen_prelude::{Promise, spawn};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Result, Status};
use napi_derive::napi;
use serde::Deserialize;
use serde_json::Value;

mod config;
mod event_limits;
mod result_cache;
mod undo_limits;

use config::ConfigJson;

/// 事件回调：参数为事件 JSON 文本，返回值忽略。weak、无 error-first 参数。
type EventTsfn = ThreadsafeFunction<String, (), String, Status, false, true>;
/// 审批 / 配对回调：参数为请求 JSON 文本，返回 `Promise<boolean>`。
type DecisionTsfn = ThreadsafeFunction<String, Promise<bool>, String, Status, false, true>;
/// 调用进度回调：参数为 [`ProgressEvent`]，返回值忽略。weak、无 error-first 参数。
type ProgressTsfn = ThreadsafeFunction<ProgressEvent, (), ProgressEvent, Status, false, true>;
/// 唤醒回调：参数为 WakeRequest JSON 文本，返回 `Promise<string | null>`：
/// `null` = 已发出激活；字符串 = 失败 `{"kind","message"}` 的 JSON（TS 封装层规整）。
type WakerTsfn = ThreadsafeFunction<String, Promise<Option<String>>, String, Status, false, true>;

/// 一条调用进度（`callToolWithProgress` 的回调参数）。
#[napi(object)]
pub struct ProgressEvent {
    pub progress: f64,
    /// 未知时省略。
    pub total: Option<f64>,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 错误消息形如 `[CODE] 说明`：同步与 async 方法（后者只能携带 `Status`）统一编码方式，
/// TS 封装层解析前缀得到 `HubError.kind`。
fn err(code: &str, message: impl Into<String>) -> napi::Error {
    napi::Error::new(
        Status::GenericFailure,
        format!("[{code}] {}", message.into()),
    )
}

/// 启动类 I/O 错误：缺少 cargo feature（`ErrorKind::Unsupported`，spec/hub-api.md 3.10）→ `UNSUPPORTED`，其余 → `START_FAILED`。
fn start_error(context: &str, e: &std::io::Error) -> napi::Error {
    let code = match e.kind() {
        std::io::ErrorKind::Unsupported => "UNSUPPORTED",
        _ => "START_FAILED",
    };
    err(code, format!("{context}：{e}"))
}

fn invalid_arg(message: impl Into<String>) -> napi::Error {
    err("INVALID_ARG", message)
}

fn hub_error(e: HubError) -> napi::Error {
    let kind = serde_json::to_value(e.kind())
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "INTERNAL".to_owned());
    err(&kind, e.message().to_owned())
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| err("INTERNAL", format!("序列化失败：{e}")))
}

fn parse_json<T: for<'de> Deserialize<'de> + Default>(what: &str, text: Option<&str>) -> Result<T> {
    match text.map(str::trim) {
        None | Some("") | Some("null") => Ok(T::default()),
        Some(t) => {
            serde_json::from_str(t).map_err(|e| invalid_arg(format!("{what} 不是合法的 JSON：{e}")))
        }
    }
}

fn parse_format(format: &str) -> Result<ToolFormat> {
    ToolFormat::from_str(format).map_err(invalid_arg)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

// ---------------------------------------------------------------------------
// 回调适配
// ---------------------------------------------------------------------------

/// 把请求 JSON 交给 JS，等待其返回的 Promise。任何失败（事件循环关闭、reject、非布尔值）都视为拒绝。
async fn ask_js(tsfn: &DecisionTsfn, request: String) -> bool {
    match tsfn.call_async(request).await {
        Ok(promise) => promise.await.unwrap_or(false),
        Err(_) => false,
    }
}

struct JsApproval(DecisionTsfn);

#[async_trait]
impl ApprovalHandler for JsApproval {
    async fn approve(&self, req: ApprovalRequest) -> bool {
        match serde_json::to_string(&req) {
            Ok(json) => ask_js(&self.0, json).await,
            Err(_) => false,
        }
    }
}

struct JsPairing(DecisionTsfn);

#[async_trait]
impl PairingHandler for JsPairing {
    async fn pair(&self, req: PairingRequest) -> bool {
        match serde_json::to_string(&req) {
            Ok(json) => ask_js(&self.0, json).await,
            Err(_) => false,
        }
    }
}

struct JsWaker(WakerTsfn);

/// App 事件的厂商回调（spec/hub-api.md 3.17）：`None` = 已清除（Hub 没有撤销接口，换成空实现）。
struct JsEventHandler(Option<EventTsfn>);

impl EventHandler for JsEventHandler {
    /// @invariant 在 Hub 的连接任务上同步执行：只做序列化与非阻塞投递，JS 回调在 Node 事件循环上执行。
    fn on_event(&self, event: &AppEvent) {
        let Some(tsfn) = &self.0 else { return };
        if let Ok(json) = serde_json::to_string(event) {
            let _ = tsfn.call(json, ThreadsafeFunctionCallMode::NonBlocking);
        }
    }
}

fn launch_failed(message: impl Into<String>) -> HubError {
    HubError::new(ErrorKind::LaunchFailed, message)
}

/// `{"kind","message"}` → HubError；类别不认识时按 `LAUNCH_FAILED`。
fn wake_error(json: &str) -> HubError {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let message = v
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .unwrap_or("唤醒失败。")
        .to_owned();
    let kind = v
        .get("kind")
        .cloned()
        .and_then(|k| serde_json::from_value::<ErrorKind>(k).ok())
        .unwrap_or(ErrorKind::LaunchFailed);
    HubError::new(kind, message)
}

#[async_trait]
impl Waker for JsWaker {
    async fn wake(&self, req: WakeRequest) -> std::result::Result<(), HubError> {
        let json = serde_json::to_string(&req).map_err(|e| launch_failed(e.to_string()))?;
        let promise = self
            .0
            .call_async(json)
            .await
            .map_err(|e| launch_failed(format!("无法调用 JS 唤醒回调：{e}")))?;
        match promise.await {
            Ok(None) => Ok(()),
            Ok(Some(err)) => Err(wake_error(&err)),
            Err(e) => Err(launch_failed(e.reason.clone())),
        }
    }
}

// ---------------------------------------------------------------------------
// Hub
// ---------------------------------------------------------------------------

/// 嵌入式 Hub。用 `Hub.start(configJson)` 创建。
#[napi(js_name = "Hub")]
pub struct JsHub {
    hub: Mutex<Option<Arc<Hub>>>,
    listen_addr: Option<String>,
    ipc_endpoint: Option<String>,
    /// 事件转发任务（`onEvent` 设置）。
    events: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl JsHub {
    fn hub(&self) -> Result<Arc<Hub>> {
        lock(&self.hub)
            .clone()
            .ok_or_else(|| err("SHUTDOWN", "Hub 已关闭"))
    }

    fn stop_events(&self) {
        if let Some(task) = lock(&self.events).take() {
            task.abort();
        }
    }
}

#[napi]
impl JsHub {
    /// 启动 Hub。`configJson` 见 `@app-mcp/hub` 的 `HubConfig`。
    #[napi(factory)]
    pub async fn start(config_json: Option<String>) -> Result<JsHub> {
        let cfg: ConfigJson = parse_json("config", config_json.as_deref())?;
        let hub = Hub::start(cfg.into_config())
            .await
            .map_err(|e| start_error("Hub 启动失败", &e))?;
        let listen_addr = hub.listen_addr().map(|a| a.to_string());
        let ipc_endpoint = hub.ipc_endpoint().map(str::to_owned);
        Ok(JsHub {
            hub: Mutex::new(Some(Arc::new(hub))),
            listen_addr,
            ipc_endpoint,
            events: Mutex::new(None),
        })
    }

    /// HTTP 服务（`/app`、`/healthz`）实际监听的地址（`host:port`，App 端点为 `ws://<地址>/app`）；未开启时为 `null`。
    #[napi(getter)]
    pub fn listen_addr(&self) -> Option<String> {
        self.listen_addr.clone()
    }

    /// 本地 IPC 连接服务的端点（可直接作为原生 SDK 的 `hostUrl`）；未开启时为 `null`。
    #[napi(getter)]
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
    }

    /// 是否已关闭。
    #[napi(getter)]
    pub fn is_shutdown(&self) -> bool {
        lock(&self.hub).is_none()
    }

    /// 停止：中止后台任务、关闭所有 App 连接、释放 JS 回调。可重复调用。
    #[napi]
    pub async fn shutdown(&self) -> Result<()> {
        self.stop_events();
        let Some(mut hub) = lock(&self.hub).take() else {
            return Ok(());
        };
        // 进行中的调用持有 Arc<Hub>；稍等它们结束，仍未结束则直接丢弃（Drop 中止后台任务）。
        for _ in 0..50 {
            match Arc::try_unwrap(hub) {
                Ok(owned) => {
                    owned.shutdown().await;
                    return Ok(());
                }
                Err(shared) => {
                    hub = shared;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
        drop(hub);
        Ok(())
    }

    /// `AppInfo[]` 的 JSON。
    #[napi]
    pub fn apps(&self) -> Result<String> {
        to_json(&self.hub()?.apps())
    }

    /// `HubStatus` 的 JSON（运行状态、最近错误、SDK 上报；与 `GET /status` 相同，spec/hub-api.md 3.9）。
    #[napi]
    pub fn status(&self) -> Result<String> {
        to_json(&self.hub()?.status())
    }

    /// `PolicyStatus` 的 JSON：生效的策略规则、命中次数与最近的加载错误（spec/hub-api.md 3.13）。
    #[napi]
    pub fn policy(&self) -> Result<String> {
        to_json(&self.hub()?.policy())
    }

    /// 替换策略规则集（`PolicyConfig` 的 JSON，命中计数清零）。JSON 不合法 → `INVALID_ARG`；
    /// 规则不合法 → `INVALID_INPUT`（之前的规则继续生效，原因记入 `policy().lastError`）。
    #[napi]
    pub fn set_policy(&self, policy_json: String) -> Result<()> {
        let policy: PolicyConfig = serde_json::from_str(&policy_json)
            .map_err(|e| invalid_arg(format!("policy 不是合法的 JSON：{e}")))?;
        self.hub()?.set_policy(policy).map_err(hub_error)
    }

    /// `IntentsStatus` 的 JSON：标准意图的机主默认表与最近一次替换失败的原因（spec/intents.md 第 4 节）。
    #[napi]
    pub fn intents(&self) -> Result<String> {
        to_json(&self.hub()?.intents())
    }

    /// 整体替换意图默认表（`{动词: 工具全名}` 的 JSON，`{}` 清空）。JSON 不合法 → `INVALID_ARG`；
    /// 表不合法 → `INVALID_INPUT`（之前的默认表继续生效，原因记入 `intents().lastError`）。
    #[napi]
    pub fn set_intent_defaults(&self, defaults_json: String) -> Result<()> {
        let defaults: BTreeMap<String, String> = serde_json::from_str(&defaults_json)
            .map_err(|e| invalid_arg(format!("intentDefaults 不是合法的 JSON：{e}")))?;
        self.hub()?.set_intent_defaults(defaults).map_err(hub_error)
    }

    /// 替换 Agent 登记（`[{"name","token"}]` 的 JSON，`[]` 清空），只影响之后到达的 MCP 请求。JSON 不合法 → `INVALID_ARG`；
    /// 登记不合法 → `INVALID_INPUT`（之前的登记继续生效；信息不含令牌）。
    #[napi]
    pub fn set_agents(&self, agents_json: String) -> Result<()> {
        let agents: Vec<AgentCredential> = serde_json::from_str(&agents_json)
            .map_err(|e| invalid_arg(format!("agents 不是合法的 JSON：{e}")))?;
        self.hub()?.set_agents(AgentsConfig { agents }).map_err(hub_error)
    }

    /// `HubTool[]` 的 JSON。`filterJson` 为 `ToolFilter`（可省略）。
    #[napi]
    pub fn tools(&self, filter_json: Option<String>) -> Result<String> {
        let filter: ToolFilter = parse_json("filter", filter_json.as_deref())?;
        to_json(&self.hub()?.tools(&filter))
    }

    /// `HubResource[]` 的 JSON。
    #[napi]
    pub fn resources(&self) -> Result<String> {
        to_json(&self.hub()?.resources())
    }

    /// `AppOverviewInfo` 的 JSON；未知 App 或无总览时为 `null`。
    #[napi]
    pub fn overview(&self, app_id: String) -> Result<Option<String>> {
        self.hub()?
            .overview(&app_id)
            .map(|o| to_json(&o))
            .transpose()
    }

    /// 调用工具：`requestJson` 为 `CallRequest`，结果为 `CallOutcome` 的 JSON。
    /// 仅当名称无法解析时抛错（`code` 为错误类别）；工具层面的失败在 `result.error` 中。
    #[napi]
    pub async fn call_tool(&self, request_json: String) -> Result<String> {
        let req: CallRequest = serde_json::from_str(&request_json)
            .map_err(|e| invalid_arg(format!("request 不是合法的 CallRequest：{e}")))?;
        let hub = self.hub()?;
        let outcome = hub.call_tool(req).await.map_err(hub_error)?;
        to_json(&outcome)
    }

    /// 同 `callTool`，并接收调用进度（spec/hub-api.md 3.12）：`onProgress({ progress, total?, message? })` 在 Node
    /// 事件循环上逐条执行（已按 `progressIntervalMs` 合并、丢弃不递增的值）；全部进度先于返回的 Promise 完成投递，
    /// 调用结束后不再回调。
    #[napi]
    pub async fn call_tool_with_progress(&self, request_json: String, on_progress: ProgressTsfn) -> Result<String> {
        let req: CallRequest = serde_json::from_str(&request_json)
            .map_err(|e| invalid_arg(format!("request 不是合法的 CallRequest：{e}")))?;
        let hub = self.hub()?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let forward = |p: app_mcp_hub::ProgressUpdate| {
            let event = ProgressEvent { progress: p.progress, total: p.total, message: p.message };
            let _ = on_progress.call(event, ThreadsafeFunctionCallMode::NonBlocking);
        };
        let call = hub.call_tool_with_progress(req, tx);
        tokio::pin!(call);
        let outcome = loop {
            tokio::select! {
                biased;
                Some(p) = rx.recv() => forward(p),
                out = &mut call => {
                    while let Ok(p) = rx.try_recv() {
                        forward(p);
                    }
                    break out;
                }
            }
        };
        to_json(&outcome.map_err(hub_error)?)
    }

    #[napi]
    pub fn cancel_call(&self, call_id: String) -> Result<()> {
        self.hub()?.cancel_call(&call_id);
        Ok(())
    }

    /// 读资源，结果为 `ResourceContent` 的 JSON。
    #[napi]
    pub async fn read_resource(&self, uri: String) -> Result<String> {
        let hub = self.hub()?;
        let content = hub.read_resource(&uri).await.map_err(hub_error)?;
        to_json(&content)
    }

    #[napi]
    pub fn subscribe(&self, uri: String) -> Result<()> {
        self.hub()?.subscribe(&uri).map_err(hub_error)
    }

    #[napi]
    pub fn unsubscribe(&self, uri: String) -> Result<()> {
        self.hub()?.unsubscribe(&uri);
        Ok(())
    }

    /// 全局选择 App 的实例；`instanceId` 省略 / `null` 清除选择。
    #[napi]
    pub fn select_instance(&self, app_id: String, instance_id: Option<String>) -> Result<()> {
        self.hub()?.select_instance(&app_id, instance_id.as_deref());
        Ok(())
    }

    /// 重置会话状态（总览首次附带、`apps.select`）。`session` 省略为默认会话。
    #[napi]
    pub fn reset_session(&self, session: Option<String>) -> Result<()> {
        self.hub()?.reset_session(session.as_deref());
        Ok(())
    }

    /// 按格式导出工具定义（JSON）。`format`：`mcp` / `openai-chat` / `openai-responses` / `anthropic` / `gemini`。
    #[napi]
    pub fn export_tools(&self, format: String, filter_json: Option<String>) -> Result<String> {
        let format = parse_format(&format)?;
        let filter: ToolFilter = parse_json("filter", filter_json.as_deref())?;
        to_json(&self.hub()?.export_tools(format, &filter))
    }

    /// 执行模型返回的一个工具调用（该格式的 JSON），返回该格式的“工具结果”消息 JSON。不因工具失败而抛错。
    #[napi]
    pub async fn dispatch(
        &self,
        format: String,
        tool_call_json: String,
        session: Option<String>,
    ) -> Result<String> {
        let format = parse_format(&format)?;
        let call: Value = serde_json::from_str(&tool_call_json)
            .map_err(|e| invalid_arg(format!("toolCall 不是合法的 JSON：{e}")))?;
        let hub = self.hub()?;
        let result = hub
            .dispatch_in_session(format, call, session.as_deref())
            .await;
        to_json(&result)
    }

    /// 在 `addr` 上开 Streamable HTTP MCP 服务，返回实际地址。
    #[napi]
    pub async fn serve_http(&self, addr: String, allow_remote: Option<bool>) -> Result<String> {
        let hub = self.hub()?;
        let local = hub
            .serve_http(&addr, allow_remote.unwrap_or(false))
            .await
            .map_err(|e| start_error("HTTP 服务启动失败", &e))?;
        Ok(local.to_string())
    }

    /// 设置事件回调（替换之前的）：`listener(eventJson)` 在 Node 事件循环上执行。`null` 取消。
    #[napi]
    pub fn on_event(&self, listener: Option<EventTsfn>) -> Result<()> {
        let hub = self.hub()?;
        self.stop_events();
        let Some(tsfn) = listener else {
            return Ok(());
        };
        let mut rx = hub.events();
        drop(hub); // 转发任务不持有 Hub，避免阻碍 shutdown。
        let task = spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let Ok(json) = serde_json::to_string(&ev) else {
                            continue;
                        };
                        if tsfn.call(json, ThreadsafeFunctionCallMode::NonBlocking)
                            == Status::Closing
                        {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
        *lock(&self.events) = Some(task);
        Ok(())
    }

    /// 设置 App 事件回调（替换之前的）：每个通过校验的事件（不论有无订阅）`handler(appEventJson)` 一次，在 Node 事件循环上执行。
    /// `null` 清除。
    #[napi]
    pub fn set_event_handler(&self, handler: Option<EventTsfn>) -> Result<()> {
        self.hub()?.set_event_handler(Arc::new(JsEventHandler(handler)));
        Ok(())
    }

    /// 设置审批回调：`handler(requestJson) => Promise<boolean>`。reject / 非布尔值视为拒绝。
    #[napi]
    pub fn set_approval_handler(&self, handler: DecisionTsfn) -> Result<()> {
        self.hub()?
            .set_approval_handler(Arc::new(JsApproval(handler)));
        Ok(())
    }

    /// 设置唤醒回调：`handler(wakeRequestJson) => Promise<string | null>`（`null` = 已发出激活；
    /// 字符串 = 失败 `{"kind","message"}`）。`null` 恢复配置 `waker` 决定的实现（默认系统唤醒）。
    #[napi]
    pub fn set_waker(&self, handler: Option<WakerTsfn>) -> Result<()> {
        let hub = self.hub()?;
        match handler {
            Some(h) => hub.set_waker(Arc::new(JsWaker(h))),
            None => hub.reset_waker(),
        }
        Ok(())
    }

    /// 设置配对回调：`handler(requestJson) => Promise<boolean>`。reject / 非布尔值视为拒绝。
    #[napi]
    pub fn set_pairing_handler(&self, handler: DecisionTsfn) -> Result<()> {
        self.hub()?
            .set_pairing_handler(Arc::new(JsPairing(handler)));
        Ok(())
    }
}
