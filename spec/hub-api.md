# Hub SDK（Agent 端）接口规范

版本：草案 v0.1（2026-09-30）。本文件是 `crates/hub` 及其各语言绑定之间的契约（权威）。

## 1. 定位

App 端 SDK（`crates/core` 等）让 **App** 暴露工具；Hub SDK 让 **Agent / 助手厂商**
（手机厂商语音助手、车机、PC 助手、IDE、自研 Agent 框架）把"连接本机所有 App"的能力直接嵌进自己的产品，
而不必运行独立的 `app-mcp-host` 进程、也不必走 MCP：

```mermaid
flowchart TD
  agent["厂商 Agent<br/>（自有 LLM 循环 / 自有 UI）"]
  hub["Hub（嵌入厂商进程）"]
  agent -- "Hub SDK：列工具、调用、读资源、事件、<br/>审批回调、工具格式导出" --> hub
  hub -- "本地 IPC / WebSocket / 进程内" --> apps["各 App（App 端 SDK）"]
  hub -- "子进程" --> upstream["上游 MCP 服务器"]
  hub -. "可选：MCP stdio / Streamable HTTP" .-> mcp["MCP 客户端"]
```

`app-mcp-host` 可执行程序改为 Hub 之上的薄壳：命令行解析 + `Hub::serve_stdio` / `serve_http_with`。
推荐的接入方式是常驻的 `app-mcp-host serve`（一个进程、多个 MCP HTTP 会话共享 App 连接，见 3.6 与 `crates/host/README.md`）。
MCP 只是 Hub 的一种"对外出口"，与格式导出（第 5 节）并列。

## 2. 分层与包

| 包 | 内容 |
|---|---|
| `crates/hub`（`app-mcp-hub`） | Hub 库：注册表、路由、总览、App 连接服务、上游聚合、MCP 出口、格式导出、策略回调。由 `crates/host` 的实现迁移而来 |
| `crates/host`（`app-mcp-host`） | 可执行程序，只含 `main.rs`（命令行）与集成测试 |
| `bindings/hub-c` | C ABI，头文件 `include/app_mcp_hub.h` → C / C++ / C# / Dart |
| `bindings/hub-uniffi` | uniffi → Kotlin（Android 厂商）/ Swift / Python；Kotlin 为 JVM jar `sdks/kotlin/app-mcp-hub` + Android AAR `sdks/kotlin/app-mcp-hub-android`（四 ABI `.so`，R8 规则随 jar 发布） |
| `bindings/hub-node` | napi-rs → npm `@app-mcp/hub`（Electron 助手、Node Agent 框架） |

Hub 自带 tokio 多线程运行时（绑定层创建），Rust 用户可在自有运行时中直接使用 async API。

## 3. Rust API（`app_mcp_hub`）

```rust
pub struct HubConfig {
    // 原 HostConfig 全部字段（manifests、allow_origins、各超时、upstreams）
    pub listen: Option<String>,             // HTTP 服务（/app、/healthz、可选 /mcp），默认 127.0.0.1:7717；None = 不开（3.6）
    pub listen_alternates: Vec<String>,     // listen 被占用时依次尝试，默认 127.0.0.1:7737、127.0.0.1:7757
    pub http: HttpOptions,                  // 令牌（只作用于 /mcp）、allow_remote
    pub mcp_http: bool,                     // listen 与 IPC 端点上是否提供 /mcp，默认 false（3.6、3.8）
    pub run_dir: Option<PathBuf>,           // 单实例锁 + 登记文件目录，默认 None（3.6）
    pub state_dir: Option<PathBuf>,         // 持久状态（休眠记录）目录，默认 None = 不读写文件（3.5「持久化」）
    pub ipc_endpoint: Option<String>,       // 本地 IPC 端点，默认平台默认端点；None = 不开（3.8）
    pub approval: ApprovalPolicy,           // 见 3.3
    pub limits: LimitPolicy,                // 资源保护：限流与大小上限（3.11）
    pub output_validation: OutputValidation, // 结果与 outputSchema 不符时 Off / Log（默认）/ Reject（3.11）
    pub policy: PolicyConfig,               // 策略规则（hide / deny），默认无规则（3.13）
}

impl Hub {
    pub async fn start(config: HubConfig) -> io::Result<Hub>;
    pub fn listen_addr(&self) -> Option<SocketAddr>;          // 实际监听地址（3.6）
    pub fn ipc_endpoint(&self) -> Option<&str>;               // 3.8
    pub fn identity(&self) -> &HostIdentity;                  // service / version / user / pid
    pub fn endpoint_registry(&self) -> EndpointRegistry;      // 登记文件内容（spec/protocol.md 1.7）
    pub async fn shutdown(self);

    // ---- 查询（同步，读快照）----
    pub fn apps(&self) -> Vec<AppInfo>;                       // 含上游（kind = Upstream）
    pub fn tools(&self, filter: &ToolFilter) -> Vec<HubTool>;
    pub fn resources(&self) -> Vec<HubResource>;
    pub fn overview(&self, app_id: &str) -> Option<AppOverviewInfo>;
    pub fn status(&self) -> HubStatus;                        // 运行状态、最近错误、SDK 上报（3.9）

    // ---- 操作 ----
    pub async fn call_tool(&self, req: CallRequest) -> Result<CallOutcome, HubError>;
    pub async fn call_tool_with_progress(&self, req: CallRequest,
        progress: mpsc::UnboundedSender<ProgressUpdate>) -> Result<CallOutcome, HubError>;   // 3.12
    pub fn cancel_call(&self, call_id: &str);
    pub async fn read_resource(&self, uri: &str) -> Result<ResourceContent, HubError>;
    pub fn subscribe(&self, uri: &str) -> Result<(), HubError>;
    pub fn unsubscribe(&self, uri: &str);
    pub fn select_instance(&self, app_id: &str, instance_id: Option<&str>);

    // ---- 事件 ----
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<HubEvent>;

    // ---- 策略回调 ----
    pub fn set_approval_handler(&self, h: Arc<dyn ApprovalHandler>);
    pub fn set_pairing_handler(&self, h: Arc<dyn PairingHandler>);
    pub fn set_policy(&self, p: PolicyConfig) -> Result<(), HubError>; // 3.13
    pub fn policy(&self) -> PolicyStatus;                               // 3.13

    // ---- 对外出口 ----
    pub fn mcp_session(&self) -> McpSession;                  // rmcp ServerHandler
    pub async fn serve_stdio(&self) -> anyhow::Result<()>;
    pub async fn serve_http(&self, addr: &str, allow_remote: bool) -> io::Result<SocketAddr>; // 额外监听器
    pub async fn serve_http_with(&self, addr: &str, options: HttpOptions) -> io::Result<SocketAddr>; // 3.6

    // ---- 工具格式导出（第 5 节）----
    pub fn export_tools(&self, format: ToolFormat, filter: &ToolFilter) -> serde_json::Value;
    pub async fn dispatch(&self, format: ToolFormat, tool_call: serde_json::Value)
        -> serde_json::Value;                                   // 返回该格式的"工具结果"消息
}
```

### 3.1 数据类型

```rust
pub struct AppInfo {
    pub app_id: String, pub name: String, pub kind: AppKind,   // App | Upstream
    pub summary: Option<String>,
    pub connected: bool, pub instances: Vec<InstanceInfo>,
    pub selected_instance: Option<String>,
}
pub struct InstanceInfo { pub instance_id: String, pub client_kind: String,
    pub visibility: Visibility, pub focused: bool, pub last_active_ms: u64,
    pub pid: Option<u32>,    // pid：经本地 IPC 连接的实例进程号（3.8）
    pub connection_id: Option<String> }   // Hub 分配的连接 ID（3.9）；休眠实例为 None

pub struct HubTool {
    pub name: String,            // 全名 "<appId>.<tool>"，与 MCP 出口一致
    pub app_id: String, pub tool: String,
    pub title: Option<String>, pub description: String,
    pub input_schema: Value, pub risk: Risk, pub activation: Activation,
    pub availability: Availability,   // Available | Disconnected | NotRegistered | Dormant
    pub annotations: ToolAnnotations, // Agent 实际看到的 MCP 注解（声明优先，缺少的按 risk 推导；上游原样，spec/protocol.md 3.2）
    pub output_schema: Option<Value>, // App 声明的结果 schema（原样；MCP 出口按需包装）
    pub surface: Option<ToolSurface>, // App 工具的界面依赖 app | view（spec/protocol.md 3.4；未声明即 app）；内置 / 上游为 None
    pub page: Option<String>,         // App 工具所在页面（声明的 page，或页面目录中的页面，3.14）；没有时 None
}                                     // surface / page 为 None 时 JSON 中不出现
// HubResource 另有 annotations: Option<ContentAnnotations>（App 对资源内容的标注，原样）
pub struct ToolFilter {
    pub apps: Option<Vec<String>>,          // None = 全部
    pub max_risk: Option<Risk>,             // 只要不高于此风险的工具
    pub only_available: bool,               // 默认 false（静态工具也列出，调用时按需唤醒）
    pub include_builtin: bool,              // apps.list / apps.select / apps.overview（渐进暴露时另有 apps.tools），默认 true
    pub session: Option<String>,            // 渐进暴露按此会话计算（3.7）；None = 默认会话
}

pub struct CallRequest {
    pub name: String,                // 全名
    pub arguments: Value,
    pub instance_id: Option<String>, // 指定实例；None 按路由规则
    pub timeout: Option<Duration>,
    pub call_id: Option<String>,     // 供 cancel_call；None 自动生成。以同一 call_id 重试时 App 只执行一次（spec/protocol.md 3.3）
    pub session: Option<String>,     // 厂商会话 ID：用于"首次接触附带总览"按会话计算；None = 默认会话
    pub idempotency_key: Option<String>, // Agent 幂等键，原样转交 App（3.15；spec/protocol.md 3.3）
}
pub struct CallOutcome {
    pub call_id: String,
    pub result: Result<Value, ToolError>,     // ToolError 含 kind / message / details
    pub state_hints: Vec<String>,
    pub instance_id: Option<String>,
    pub overview: Option<AppOverviewInfo>,    // 该会话首次接触此 App 时附带（spec/protocol.md §7）
    pub status: ResultStatus,                 // done（缺省）| pending | partial | noop（spec/protocol.md 3.2）
    pub state_resource: Option<String>,       // pending 时的状态资源 URI（app-mcp://<appId>/<名>）
    pub summary: Option<String>,              // App 给出的一句结论
    pub annotations: Option<ContentAnnotations>, // App 对结果内容的标注（原样）
    pub routed_to: Option<String>,            // 改调了后台替代时实际调用的工具全名（3.14）
    pub duration_ms: u64,                     // Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App；3.15 键表）
    pub woke: bool,                           // 本次 App 工具调用是否经历了唤醒；内置 / 上游工具恒为 false（3.15 键表）
}

pub enum HubEvent {
    AppConnected { app_id: String, instance_id: String },
    AppDisconnected { app_id: String, instance_id: String },
    ToolsChanged,                                  // 已合并（list_changed_debounce）
    ResourcesChanged,
    ResourceUpdated { uri: String },
    VisibilityChanged { app_id: String, instance_id: String, visibility: Visibility },
    UpstreamState { name: String, connected: bool, error: Option<String> },
    AppDormant { app_id: String, instance_id: String },          // 3.5
    AppWaking { app_id: String, instance_id: Option<String> },   // 3.5；None = 冷启动
    AppDiagnostic { app_id: String, instance_id: String, code: String, message: String, count: u32 }, // SDK 上报（3.9）
}
```

`HubError`：`ToolError` 的包装（同一套错误码，spec/protocol.md §4）。

### 3.2 调用语义

与 MCP 出口完全一致（MCP 出口改为调用 `Hub::call_tool` 实现，不再有第二份逻辑）：
schema 校验 → 策略审批（3.3）→ 路由（selected → focused → 最近活跃 → 最早）→
只有休眠实例注册了该工具（或 App 未运行而清单声明了 `wake`）时先唤醒（3.5）；无法唤醒的静态工具且 App 未连接时返回
`APP_DISCONNECTED`（details 含 `launchUrl`）→ 转发、超时、取消。

App 工具与上游工具在路由 / 审批之前先过资源保护（3.11）：参数大小 → 限流；结果到达后检查结果大小。App 工具的结果再按
`output_validation` 核对 `outputSchema`；上游工具的结果只检查大小、不核对 `outputSchema`（上游 MCP 服务器自己负责其 schema，
结果原样转发）。内置工具 `apps.*` 不受限。

**MCP 出口的工具与结果**（spec/protocol.md 3.2 为唯一定义，此处只列映射）：`tools/list` 的 `annotations` = `HubTool.annotations`，
`outputSchema` = 声明的 schema（根类型非 object 时包装为 `{result}`）；成功结果的内容块依次为：状态说明（`status` 非 `done`）→
`summary` → 返回值 JSON（无返回值、无摘要且 `done` 时为"已完成"）→ 资源变化提示（`stateHints`），App 的内容标注只加在
摘要与返回值块上；`structuredContent` 按 `outputSchema` 决定（无返回值时不填）；`status` 非 `done` 时 `_meta` 带
`dev.appwire/status` 与 `dev.appwire/stateResource`（全部 `_meta` 键见 3.15 的键表）。
资源列表的 `annotations` 为 App 声明的内容注解。Hub API（`CallOutcome`）给出同样的信息：`result` 为原始 `data`（无返回值为 `null`），
另有 `status`、`state_resource`、`summary`、`annotations` 字段。

### 3.3 策略回调（厂商 UI 接管确认）

```rust
pub struct ApprovalPolicy { pub require_at_or_above: Option<Risk> }  // 默认 None：不审批（与现 Host 一致）

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// 返回 false → 调用以 USER_REJECTED 结束。超时（response_timeout）视为拒绝。
    async fn approve(&self, req: ApprovalRequest) -> bool;
}
pub struct ApprovalRequest { pub call_id: String, pub app_id: String, pub app_name: String,
    pub tool: String, pub title: Option<String>, pub description: String,
    pub risk: Risk, pub arguments: Value, pub session: Option<String>,
    pub annotations: ToolAnnotations,       // 与 HubTool.annotations 相同，供厂商按声明决定是否确认
    pub principal: Option<String>,          // 第 12 项 S6：MCP 出口的认证主体（传输层凭据，现在恒为 "local"）；Hub API 为 None
    pub client_name: Option<String> }       // 第 12 项 S6：MCP 客户端自报的 clientInfo.name，仅供显示（不可信，不得据此授权）
// session：Hub API 为 CallRequest.session 原样；MCP 出口为调用方键（legacy `mcp:<n>`，无会话请求 `principal:<主体>`，3.6）
// principal / client_name 为 None 时 JSON 中不出现（Hub API 发起的审批与之前逐字节相同）；client_name：legacy 取自 initialize，
// 无会话请求取自请求 _meta 的 io.modelcontextprotocol/clientInfo

#[async_trait]
pub trait PairingHandler: Send + Sync {
    /// 未知 App（无清单、非本机 Origin 白名单）首次连接时询问；默认实现：拒绝非白名单 Origin、接受其他。
    async fn pair(&self, req: PairingRequest) -> bool;
}
pub struct PairingRequest { pub app_id: String, pub app_name: String,
    pub origin: Option<String>, pub client_kind: String }
```

绑定层把 async trait 映射为"同步回调 + 完成句柄"：外部语言的回调在 Hub 线程上被同步调用、立即返回，
结果稍后在任意线程经句柄回传；回调线程上不要求外部语言有事件循环 / 协程上下文。

- C：`am_hub_approval_cb` + `am_hub_approval_complete(handle, bool)`（配对、唤醒同理）。
- uniffi（Kotlin / Swift / Python）：
  `ApprovalHandler.on_request(req, responder: ApprovalResponder)` → `responder.complete(bool)`；
  `PairingHandler.on_request(req, responder: PairingResponder)` → `responder.complete(bool)`；
  `HubWaker.wake(req, responder: WakeResponder)` → `responder.succeed()` / `responder.fail(kind, reason)`。
  只有第一次完成生效（返回是否生效）；句柄未完成即被释放、回调抛出异常 → 拒绝（唤醒为 `LAUNCH_FAILED`）。
  各语言封装把句柄适配为惯用 API：Kotlin `suspend` handler（在 Hub 的协程作用域、可指定 `CoroutineContext`
  如 `Dispatchers.Main` 中执行）、Swift `async` handler（新 `Task`，需要主线程时标注 `@MainActor`）、
  Python 同步函数（线程池 / `dispatcher`）或 `async def`（设置时的事件循环，无循环时 `asyncio.run`）。

### 3.4 实现补充（v0.1 实现，与上文并存）

- `Hub::call_tool`：工具层面的失败（参数不合法、用户拒绝、超时、App 报错、工具不存在但 App 已知等）放在
  `Ok(CallOutcome).result`；只有名称无法解析（不含 `.`，或 appId / 上游名未知）时返回 `Err(HubError)`。
- `CallRequest.instance_id` 为严格指定：实例不存在或未注册该工具 → `TOOL_NOT_FOUND`。
  `CallRequest.timeout` 同时作为 SDK 侧 `timeoutMs` 的上限。
- `select_instance` 的选择为全局默认（所有会话共用），会话内 `apps.select` 的选择优先；`AppInfo.selected_instance` 反映全局选择。
- 补充字段 / 方法：`ApprovalPolicy.timeout: Option<Duration>`（缺省 `response_timeout`）、
  `HubConfig.pairing_timeout`（默认 120s）、`InstanceInfo.title`、`PairingRequest.instance_id`、
  `AppOverviewInfo`（含注入文本 `text`）、`ResourceContent { uri, mime_type, text, blob }`、
  `HubResource`、`Hub::reset_session(session)`、`Hub::dispatch_in_session(format, call, session)`。
- 风险顺序：read < write < destructive < payment < os-sensitive（协议列举顺序）。
- 审批：策略要求审批但未设置 `ApprovalHandler` → `USER_REJECTED`。MCP 出口的 `ApprovalRequest.session` 为调用方键（3.3、3.6）。
  上游工具的风险取自 annotations（`readOnlyHint` → read，`destructiveHint` → destructive，否则 write）。
- 配对：设置 `PairingHandler` 后，无静态清单或 Origin 不在白名单的 App 握手返回 `pending`，
  handler 结果经 `app/pairingResult` 通知；同意过的 (appId, Origin) 或本 Hub 发出的 token 重连时不再询问。
- `dispatch`：OpenAI `id` / `call_id`、Anthropic `id`、Gemini `id` 同时作为 callId（可 `cancel_call`）。
  `Mcp` 格式也接受完整 JSON-RPC `tools/call` 请求。Gemini 的 `functionResponse.response` 为 `{output}` 或 `{error}`；
  无参数的工具不带 `parameters`；`oneOf` → `anyOf`、`const` → `enum`、`type: [T, "null"]` → `nullable`。
- `attach_local` 返回 `Result<LocalAppChannel, HubError>`，本期总是 `UNSUPPORTED_PROTOCOL`。
- 所有公开数据类型实现 serde（camelCase）；`HubEvent` 以 `{"type": "appConnected", ...}` 形式序列化；
  `CallOutcome.result` 序列化为 `{"ok": …}` / `{"error": {kind, message, details}}`；`Duration` 以毫秒数表示。

### 3.6 HTTP 服务：合并端口、单实例、多会话、令牌、健康检查

一个 HTTP/1.1 服务按路径分流（spec/protocol.md 1.3），同一个路由（`crates/hub/src/http_server.rs` 的 `Router`）
既服务 `listen` 的回环 TCP，也服务本地 IPC 端点（`mcp_http` 同时作用于两者，IPC 上的 `/mcp` 不校验令牌，见 3.8）：

| 路径 | 内容 | 校验 |
|---|---|---|
| `/app`（及兼容期的 `/`） | WebSocket 升级 → App 连接 | `Origin` 在 `app/hello` 时按允许列表 / `PairingHandler` 处理（不在 HTTP 层 403） |
| `/mcp` | MCP Streamable HTTP（`mcp_http` 或额外监听器） | `Origin` 允许列表（403）→ 令牌（401） |
| `/healthz` | `GET` → `Health` JSON | `Origin` 允许列表（403），不需要令牌 |
| `/status` | `GET` → `HubStatus` JSON（3.9） | `Origin` 允许列表（403）→ IPC 直接允许；TCP 必须带有效令牌，未配置令牌时 403 |
| `/policy` | `POST`（请求体为 `PolicyConfig` JSON，≤ 1 MiB）→ `{ok, rules?, error?}`；替换策略规则（3.13） | 同 `/status`；规则不合法时 400，之前的规则继续生效 |

```rust
pub struct HttpOptions {
    pub allow_remote: bool,                  // 允许非回环地址并关闭 Host 头校验
    pub token: Option<String>,               // 本地访问令牌（只作用于 /mcp）；None = 不校验
    pub require_token_without_origin: bool,  // 不带 Origin 的请求是否也必须带令牌
}
pub struct Health {                          // serde camelCase
    #[serde(flatten)] identity: HostIdentity,      // service: "app-mcp", version, user, pid
    listen: Option<String>, ipc_endpoint: Option<String>,
    app_path: "/app", mcp_path: Option<String>,    // 本监听器未开 MCP 时为 None
    token_required_for_browsers: bool,
}
```

- **监听**：`listen` 默认 `127.0.0.1:7717`；`AddrInUse` 时依次绑定 `listen_alternates`（默认 7737、7757，与网页 SDK
  依次握手的端口一致；显式指定 `listen` 时应清空——`app-mcp-host` 与各绑定在显式给出 `listen` 时都清空），
  全部失败返回 `AddrInUse`（消息列出尝试过的地址）。非回环地址需要 `http.allow_remote`，否则 `PermissionDenied`。
  端口 0 由系统分配，`Hub::listen_addr()` 给出实际地址。
- **单实例与登记文件**（spec/protocol.md 1.5、1.7）：`run_dir` 为 `Some` 时，`Hub::start` 在任何绑定之前锁定
  `<run_dir>/hub.lock`（已被锁定 → `ResourceBusy`，消息带持有者 pid 与地址），绑定完成后原子写
  `<run_dir>/endpoints.json`（`Hub::endpoint_registry()` 的内容），`shutdown` / Drop 时删除并释放锁。
  嵌入式 Hub 默认不参与（`None`）；`app-mcp-host` 为 `<配置目录>/run`。
- **身份**：握手结果（`service` / `user` / `pid` 与 `hostVersion`）、`/healthz`、登记文件共用 `Hub::identity()`。
- **额外监听器**：`serve_http_with(addr, options)` 另开一个提供同样路径的 HTTP 服务（总是提供 `/mcp`），用于额外的地址，
  如 `app-mcp-host` 兼容期内显式配置的旧 MCP 端口 7718。可多次调用。
- **多会话**：每个 `Mcp-Session-Id` 对应一个独立的 `McpSession`（会话键 `mcp:<n>`）：`apps.select` 选择、
  “已附带总览版本”、资源订阅按会话保存；App 连接、注册表、上游在所有会话间共享。会话结束（DELETE 或断开）时清理其状态。
  服务器主动通知（`tools/list_changed` 等）经各会话的 GET SSE 流发送（legacy；无会话请求见下方「通知」）。
- **协议版本**（第 12 项 S7）：`HubConfig.mcp_protocol_mode: McpProtocolMode`（serde `"auto"` / `"legacyOnly"`），默认 `Auto`。

  | 模式 | 声明的版本（`server/discover` 的 `supportedVersions`、每请求版本校验） | `initialize` | `subscriptions/listen` |
  |---|---|---|---|
  | `Auto`（默认） | 2024-11-05 … **2026-07-28**（上限写死为 2026-07-28，rmcp 升级不会自动扩大） | 至多 2025-11-25（rmcp `LATEST_WITH_INITIALIZE`），行为不变 | 提供 |
  | `LegacyOnly`（回退开关） | 2024-11-05 … 2025-11-25：声明 2026-07-28 的请求得 `-32022 UnsupportedProtocolVersion`（`data.supported` 不含 2026-07-28），能回退的客户端改用 `initialize`（S7 之前的行为） | 同上 | 不提供（method not found） |

  同一端点同时服务两代（dual-era）：`initialize` 客户端走 legacy 会话；以 2026-07-28 经 `server/discover` / 每请求 `_meta` 到达的请求
  走无会话路径（rmcp 不签发 `Mcp-Session-Id`）。modern 结果带 `resultType: "complete"`（rmcp 构造器填写，回复旧版本时去掉）；
  modern 下不存在空结果——`ping`、`resources/subscribe` / `unsubscribe` 对 modern 请求由 rmcp 返回 method not found，
  `logging/setLevel` 本 Hub 不实现。modern 请求的 JSON-RPC 错误不使用 AppWire 的 -32000…-32019 码：输入 / 资源不存在类
  （`data.kind` 为 `INVALID_INPUT` / `RESOURCE_NOT_FOUND`）→ `-32602`，其余 → `-32603`，类别仍在 `data.kind`；legacy 码值不变
  （docs/plans/12-mcp-stateless.md 第 5 节）。配置入口见下方「通知」之后的表（缺省均取 Hub 默认值）。
- **通知**（第 12 项 S7，`crates/hub/src/subscribers.rs`）：通知订阅方按 Hub 内部的订阅方 ID 寻址，两种——
  legacy 会话的 peer（`notifications/initialized` 时登记，会话结束移除；`resources/subscribe` 订阅按会话），与无会话请求的
  **`subscriptions/listen` 流**：订阅寿命即该请求的寿命，客户端关闭流（HTTP 断开 / `notifications/cancelled`）即结束并移除其资源订阅；
  Hub 停止（`shutdown` / Drop）时流以最终结果（`SubscriptionsListenResult`）正常结束。接受的类别：`toolsListChanged`、
  `resourcesListChanged`，以及 `resourceSubscriptions` 中可订阅的 URI（本 Hub 的 `app-mcp://` 资源、App 未被 `hide`、不是上游资源；
  去重后至多 `HubConfig.max_listen_resources`，默认 `DEFAULT_MAX_LISTEN_RESOURCES` = 256），确认通知只列出接受的部分；
  每条通知带 `subscriptionId`（rmcp 填写）。`tools/list_changed` / `resources/list_changed` 只在服务器状态变化（App 连接 / 断开 / 注册、
  全局选择、策略）时、经同一合并窗口（`list_changed_debounce`）发给所有订阅方；渐进暴露"本会话展开"的单会话通知只发 legacy 会话。
  上限（B-07）：每个主体同时打开的 listen 流至多 `HubConfig.max_listen_streams`（默认 `DEFAULT_MAX_LISTEN_STREAMS` = 16，`0` = 不提供
  listen）；确认由 rmcp 在 Hub 处理之前发出，因此超限表现为确认之后该 listen 请求以 `RATE_LIMITED` 错误（modern `-32603`）结束。
  listen 流不计入请求流活动（不阻止 3.5 的租约空闲收回与任务回收）；它占着一条 HTTP 连接，按需启动的 Host（spec/protocol.md 1.9）
  在流打开期间不会空闲退出（与 legacy 会话的 GET 流相同）。不轮询、不加定时器（rmcp 的 SSE 保活 15 秒与 legacy GET 流相同）。
  `HubStatus.mcp_listen_streams: Option<usize>`（只增字段）为当前 listen 流数，`mcp_sessions` 只计 legacy 会话。
- **配置入口**（`mcp_protocol_mode` / `max_listen_streams` / `max_listen_resources`，缺省均取 Hub 默认值）：

  | 绑定 | `mcp_protocol_mode` | `max_listen_streams` | `max_listen_resources` |
  |---|---|---|---|
  | `app-mcp-host` 配置文件 | `mcp.protocolMode`（`"auto"` / `"legacyOnly"`） | `mcp.maxListenStreams` | `mcp.maxListenResources` |
  | `app-mcp-host` 命令行 | `--mcp-protocol-mode auto\|legacy-only` | `--max-listen-streams` | （仅配置文件） |
  | hub-c（头文件 v16）/ hub-node / `@app-mcp/hub` JSON | `mcpProtocolMode`（`"auto"` / `"legacyOnly"`） | `maxListenStreams` | `maxListenResources` |
  | hub-uniffi `HubConfig` | `mcp_protocol_mode: McpProtocolMode?`（`Auto` / `LegacyOnly`） | `max_listen_streams: u32?` | `max_listen_resources: u32?` |
  | C# `HubOptions` | `McpProtocolMode`（枚举 `Auto` / `LegacyOnly`） | `MaxListenStreams`（`int?`，负数抛 `ArgumentOutOfRangeException`） | `MaxListenResources`（同左） |

  `HubStatus.mcp_listen_streams`：hub-c 为 JSON `mcpListenStreams`（头文件 v16）；`@app-mcp/hub` `mcpListenStreams?: number`；hub-uniffi
  `mcp_listen_streams: u64?`（Kotlin / Swift / Python 封装层另以同名类型别名导出 `McpProtocolMode`）；C# `HubStatusInfo.McpListenStreams`（`int?`）。
  旧 Host 不报告时为空。`app-mcp-host status` 的一行摘要与 doctor「App 实例」检查显示 listen 流数（旧 Host 不报告时省略）。
- **调用方与 Agent 任务**（第 12 项 S4、第 16 项 P1；`crates/hub/src/task.rs`）：调用方的跨请求状态（`apps.select` 选择、已附带总览、
  租约、渐进暴露已列出的 App）记在该调用方的 **Agent 任务**上，任务按**调用方键**寻址，不挂在传输会话上。每个调用方键至多一个任务，
  任务 ID 为 Hub 签发的 `task-<128 位随机数十六进制>`（`/status` 只读展示；无会话请求可另开任务并以其 ID 为句柄，见下方「任务句柄」）。

  | 调用方 | 调用方键 | 任务寿命 |
  |---|---|---|
  | legacy MCP：处理过 `initialize` 的连接（stdio / `serve_mcp_stream` 的一条流，HTTP 的一个 `Mcp-Session-Id`），请求未在 `_meta` 声明 2026-07-28 及以后的版本 | `mcp:<n>` | 会话结束（行为与之前相同） |
  | 无会话 MCP 请求：不经 `initialize`、每请求自带协议 `_meta`（rmcp `server/discover` 生命周期），HTTP、IPC、stdio 一律如此 | `principal:<主体>` | 请求流空闲达 `task_idle_ttl` 后回收 |
  | Hub API（`CallRequest.session`、`ToolFilter.session`、`dispatch_in_session`） | `api` / `api:<session>` | `reset_session` |
  | 无会话 MCP 请求出示任务句柄（参数 `taskId` / `_meta` `dev.appwire/taskId`，第 12 项 S8） | `principal:<主体>/<任务 ID>` | 同上一行的空闲回收，或 `apps.task.end` |

  主体只取自传输层凭据，不取自 `clientInfo`：TCP 上的本机令牌（及允许不带令牌的回环请求）、IPC 的同一用户、stdio 的父进程现在都是
  `principal:local`（第 16 项 N5 按 Agent 发令牌后细分）。因此**不带任务句柄的无会话请求共用一个任务**（一个 Agent 的 `apps.select` /
  `apps.release` 影响另一个，docs/plans/12-mcp-stateless.md R1；需要隔离时用下方「任务句柄」）。无会话请求不登记 peer、处理器析构无副作用（rmcp 无状态 HTTP 路径
  每请求构造一次处理器），通知经 `subscriptions/listen`（上方「通知」）。无会话请求可协商 2026-07-28（上方「协议版本」），也可以
  2025-11-25 及以前的版本经 `server/discover` 到达；其列表与总览按 3.7「无会话请求的列表与总览」（第 12 项 S5）。
  **主体级 `apps.select`**（第 12 项 S6）：无会话请求的选择记在主体任务上，对该主体的所有无会话客户端生效，**不改变工具列表**；
  另有空闲有效期 `HubConfig.principal_select_ttl: Duration`（默认 `DEFAULT_PRINCIPAL_SELECT_TTL` = 60 秒，`0` = 不单独过期）：
  选定或最近一次用于路由（工具调用、资源读取、`apps.navigate` / `apps.activate`）后这么久未再使用即失效，之后按默认规则路由，
  `apps.list` 不再显示；结果文本写明作用范围与有效期。过期在取用 / 列出时判定，不设定时器。legacy 会话与 Hub API 的选择不过期。
  `HubConfig.task_idle_ttl: Duration`（默认 `DEFAULT_TASK_IDLE_TTL` = 10 分钟，`0` = 不因空闲回收）：无会话调用方没有进行中的请求、
  距最近一次请求活动（与 3.5 租约的请求流空闲判定共用一份记录）达此时长时，回收其任务——收回仍未到期的租约（`ttlMs: 0`，其他调用方的
  未到期租约随后补发；已到期的不再发消息）、删除其租约统计与状态。没有按空闲回收的任务时 Hub 不设定时器。
- **任务句柄**（第 12 项 S8、第 16 项 P1；`crates/hub/src/task_handle.rs`；名称见 3.15 名称表）：同一无会话主体经句柄同时运行多个互相隔离的
  任务（各自的 `apps.select` 选择与租约）。只提供机制：开几个、何时结束由 Agent 决定。
  - **签发** `apps.task.begin {}` → `{taskId, idleTtlMs, message}`：为请求主体创建一个新任务，`taskId` 即其任务 ID（≥128 位随机，
    SEP-2567 的不可猜测要求），归该主体所有（其他主体出示与不存在相同）；`idleTtlMs` 为 `task_idle_ttl` 的毫秒数（`0` 时为 `null`）。
    不读任何句柄通道（总是新开）。每个主体同时存在的句柄至多 `HubConfig.max_task_handles`（默认 `DEFAULT_MAX_TASK_HANDLES` = 32，B-07），
    达到上限 → `RATE_LIMITED`（`data.limit`；与 listen 流上限相同，不带 `retryAfterMs`：释放靠 `apps.task.end` 或空闲回收）。
    `max_task_handles = 0` 关闭句柄：`apps.task.*` 不列出，签发与出示句柄 → `INVALID_INPUT`（`task-handle-unsupported`）。
  - **出示**（两条通道，取值相同可并存，不同 → `INVALID_INPUT`，不执行）：工具参数 `taskId`（模型可写，主通道）只在 `apps.list`、
    `apps.select`、`apps.navigate`、`apps.activate`、`apps.release`、`apps.task.end`（必填）上；请求 `_meta` `dev.appwire/taskId` 对任何
    `tools/call` 生效（含 App 工具与上游工具——App 工具的参数由 App 定义，Hub 不占用其 `taskId`，只能经此通道按任务路由；供自己实现客户端的
    Agent 宿主，通用客户端不会填写，docs/plans/12-mcp-stateless.md 3.4）。出示后该调用的调用方即 `principal:<主体>/<任务 ID>`：选择、
    租约、请求活动（空闲判定）、审批的 `ApprovalRequest.session` 都按该任务；`apps.overview` / `apps.tools` / `apps.page` 不持有按任务
    区分的状态，不接受参数。资源读取（`resources/read`）不读句柄。
  - **句柄任务的语义**与主体任务相同（列表与总览按无会话规则，3.7），差别只有：`apps.select` 选择只属于该任务、**不另设**
    `principal_select_ttl`（随任务结束清除），结果文本写明任务 ID 与作用范围。
  - **结束**：空闲达 `task_idle_ttl`（与主体任务同一回收循环；`0` 时不因空闲回收，只能 `apps.task.end`），或 `apps.task.end {taskId}`
    → `{taskId, ended: true, released, message}`（收回其全部租约——其他调用方的未到期租约随后补发——并清除选择；`released` = 仍未到期的
    租约数）；句柄已不存在时 `ended: false`（幂等）。
  - **错误**（工具错误，MCP 结果 `isError: true`）：参数 / `_meta` 不是字符串、格式不是 `task-<32 位小写十六进制>`、两通道冲突 → `INVALID_INPUT`；
    句柄不存在 / 已回收 / 属于其他主体 → `INVALID_INPUT`，`data.reason: "task-expired"`、`data.taskId`，消息说明回收原因并要求
    `apps.task.begin` 取新句柄后重试、在新任务中重新 `apps.select`（可恢复；不新增错误类别，spec/protocol.md 第 4 节不变）；legacy MCP 会话
    与 Hub API 出示句柄或调用 `apps.task.begin` → `INVALID_INPUT`，`data.reason: "task-handle-unsupported"`（它们已一会话一任务，
    Hub API 用 `CallRequest.session` 区分；不静默忽略）。
  - **列表**：`apps.task.begin` / `apps.task.end` 与上述工具 inputSchema 中的可选 `taskId` 只出现在无会话请求的 `tools/list`（且
    `max_task_handles > 0`）；legacy 会话、`Hub::tools` / `export_tools` 的内置工具定义与之前逐字节相同。
  - **`/status`**：句柄任务出现在 `tasks`，`caller` 为 `principal:<主体>/<任务 ID>`、`kind` 为 `principal`（`CallerKind` 不新增变体）。
  - `HubConfig.max_task_handles` 暂未经 `app-mcp-host` 配置与各语言绑定暴露（取默认值）。
- 校验顺序（`/mcp`、`/healthz`）：`Origin`（与 App 连接相同的允许列表，不通过 403）→ 路径 → 令牌（仅 `/mcp`）。
  令牌规则：`Authorization: Bearer <令牌>`；带 `Origin` 的请求必须携带；不带 `Origin` 的请求在
  `require_token_without_origin` 时必须携带；携带了错误令牌一律 401（带 `WWW-Authenticate: Bearer`）；空令牌视为未携带。常量时间比较。
  `/app` 非升级请求 426，未知路径 404。
- `/healthz` 不需要令牌（仍受 Origin 校验），供 `app-mcp-host service status` 等确认实例（与登记文件的 pid 一致）、
  以及端口被占用时说明占用者。
- Windows：Hub 启动的子进程（唤醒命令、上游）带 `CREATE_NO_WINDOW`，嵌入无控制台进程时不弹窗。

绑定（`ws_addr` → `listen` 为不兼容改名，没有旧名别名）：C 配置 JSON `listen`（省略 = 默认地址含备选端口；显式地址或
`null` = 只绑该地址 / 不开）、`mcpHttp`、`runDir`，`am_hub_ws_addr` → `am_hub_listen_addr`（`AM_HUB_API_VERSION 3`）；
Node 配置 `listen` / `mcpHttp` / `runDir`，`Hub.wsAddr` → `Hub.listenAddr`（`@app-mcp/hub` 的 `wsUrl` 为 `ws://<listenAddr>/app`）；
uniffi `HubConfig.listen` / `enable_listen` / `mcp_http` / `run_dir`，`ws_addr()` → `listen_addr()`；
C# `HubOptions.Listen` / `DisableListen` / `McpHttp` / `RunDir`，`AppMcpHub.ListenAddress`；
Kotlin / Swift `listenAddr`、Python `listen_addr`（配置参数 `listen` / `enableListen`，Python `enable_listen`）。

### 3.5 生命周期配合：休眠、唤醒、租约（spec/lifecycle.md §9）

**休眠**：SDK 发送 `app/sleep` 时，若本实例有已路由但未完成的调用 / 资源读取，或有等待该实例回连的唤醒，
Hub 返回 `{accepted: false, retryAfterMs: 1000}`；否则生成恢复令牌（128 位随机），把实例从已连接列表移入
**休眠记录**（保留工具 / 资源快照、SDK 上报的 `toolsHash` 与 `wake`），返回 `{accepted: true, resumeToken}`，
发 `HubEvent::AppDormant`，**不**发 `ToolsChanged` / MCP `list_changed`。

- 工具仍列出：只由休眠实例提供的工具 `availability = Dormant`（MCP 描述不加前缀）；资源仍列出（`available = true`，读取时唤醒）。
  `ToolFilter.only_available` 只保留 `Available`，因此不含 `Dormant`。
- `AppInfo.dormant_instances: Vec<InstanceInfo>`（新增字段）列出休眠实例；`connected` 只看已连接实例。
  `apps.list` 每个 App 增加 `dormant`（无已连接实例且有休眠实例）与 `dormantInstances`（`instanceId`、`sleptAt`、`wake`、`tools` 等）。
- 休眠记录保留 `HubConfig.dormant_ttl`（默认 24 小时）；同一 appId 以**新的**实例 ID 连接时（`dormant_replaced_by_new_instance`，默认开）
  移除该 App 的全部休眠记录。记录被移除时发 `AppDisconnected` 与列表变化。

**持久化**（4f G11，spec/lifecycle.md 第 9 节）：`HubConfig.state_dir` 为 `Some` 时休眠记录跨 Hub 重启保留；缺省 `None` 时
Hub 不读写任何文件（嵌入式厂商按需开启；`app-mcp-host` 为 `<home>/state`）。

- 文件：每个 App 一个 `<state_dir>/dormant/<appId>.json`（appId 须满足 `[a-z][a-z0-9-]{0,62}`），原子写（同目录临时文件 + 改名），
  Unix 目录 0700、文件 0600。内容 `{version: 1, appId, savedAtMs, tools: [ToolInfo], instances: [...], pageTools: [下标]}`：工具定义
  按内容去重只存一份，实例与页面目录以下标引用，读回后同一定义共享一个对象（与内存中相同）；实例含 `instanceId`、`appName`、
  `clientKind`、可选 `appVersion` / `title` / `url` / `overview` / `visibility`、`tools`（下标）、`resources`、`resumeToken`、
  `toolsHash`、`wake`、`sleptAtMs`、`connectedAtMs`、`lastActiveAtMs`。只有声明，没有调用参数 / 结果。
- 保存的实例：休眠记录，加上已就绪、在 `app/hello.wake` 中声明了唤醒描述的在线实例（Host 异常退出时它们来不及转为休眠；
  `resumeToken` / `toolsHash` 为空、`sleptAtMs` 为写出时刻，读回后按休眠实例列出，回连时完整同步）。在线实例的工具变化不触发重写，
  文件中是就绪时（或该 App 其他变化时）的工具，回连同步后即更正。
- 写入时机：实例休眠、休眠记录被回连取走 / 被新实例替换 / 过期移除、声明了唤醒描述的实例就绪 / 断开时，由一个后台任务按 App 重写
  （记录为空则删除文件）；没有变化时不设定时器。`Hub::shutdown` 时写出未写的变化。Host 被强制结束时，最近一次变化之前的记录已在磁盘上（写入在变化后毫秒级完成）。
- 读回：`Hub::start` 在开始监听之前读回（单实例锁之后）；`sleptAtMs` 早于 `dormant_ttl` 的实例丢弃（全部过期的文件删除），保留的
  按原活跃顺序登记，工具 / 资源按 SDK 上报同一套规则过滤。已有连接后才读回的情况不存在（读回先于监听）。
- 上限与容错：单个文件 ≤ 4 MiB（写入超出时不写并删除旧文件，`last_error` 记原因）；读回最多 1024 个文件（按修改时间从新到旧）。
  内容损坏、版本未知（如更新的 Host 写的）、文件名与内容不符、下标越界、超出上限 → 跳过该文件并记 warn 日志与
  `HubStatus.dormant_store.issues`，不删除、不中断启动。
- 观测：`HubStatus.dormant_store: Option<DormantStoreStatus>`（3.9）；`app-mcp-host doctor` 的「休眠记录」检查离线读目录
  （Host 未运行也可用，`app_mcp_hub::dormant_store::inspect`），列出可读回的 App / 实例数与被跳过的文件。
- 绑定：hub-c / hub-node JSON `stateDir`、`@app-mcp/hub` `HubConfig.stateDir`、hub-uniffi `HubConfig.state_dir`、C# `HubOptions.StateDir`。

**快速恢复**：`app/hello` 带 `resumeToken` + `toolsHash`，且令牌与该实例的休眠记录一致、`toolsHash` 等于
`app_mcp_protocol::tools_hash(快照)` → `HelloResult.toolsCurrent = true`，Hub 直接沿用快照；否则 `false`（SDK 完整同步）。
恢复令牌一次性：同一实例 ID 回连即消耗休眠记录。需要 `PairingHandler` 重新确认配对的握手不做快速恢复。

**唤醒**：调用 / 读资源时，若没有已连接实例提供该工具 / 资源，而休眠实例的快照中有（选定实例优先，其次最近活跃），
或 App 未运行而清单为当前平台（或 `web`）**显式**声明了 `wake`：

1. 用快照（或清单）中的定义做 schema 校验与审批（拒绝则不唤醒）；
2. 生成一次性 `wakeToken`（128 位随机，`HubConfig.wake_token_ttl` 默认 60 秒），发 `HubEvent::AppWaking`，调用当前 `Waker`；
   同一目标已在唤醒中的并发调用共用一次唤醒；
3. 等待回连：`app/ready` 时 `launchToken` 等于令牌、或实例 ID 与被唤醒的休眠实例相同、或（冷启动时）该 App 的任何实例就绪；
   `HubConfig.wake_timeout`（默认 15 秒）内未回连 → `APP_NOT_RESPONDING`；`Waker` 报错 → 该错误（通常 `LAUNCH_FAILED`）；
4. 按路由（优先回连的实例）派发，不再重复审批。

唤醒描述的解析：休眠时上报的 `wake`（`kind: none` 视为未上报）→ 清单 `wake.<平台>` → 清单 `wake.web`；
`HubConfig.wake_from_launch = true` 时再由清单 `launch.<平台>`（`uri` / `aumid` / `bundle` → `apple-event` / `dbus`）与 `launch.web` 推导。
默认不从 `launch` 推导（避免调用静态工具时自动打开浏览器 / 程序），此时行为与之前相同（`APP_DISCONNECTED` + `launchUrl`）。

```rust
#[async_trait]
pub trait Waker: Send + Sync {
    /// 按描述激活 App；Ok 表示已发出激活，回连由 Hub 等待。
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError>;
}
pub struct WakeRequest {
    pub app_id: String,
    pub instance_id: Option<String>,     // None = 冷启动
    pub descriptor: WakeDescriptor,      // app_mcp_protocol::WakeDescriptor（hub 重新导出）
    pub token: String,                   // 32 个十六进制字符
    pub activation_arg: String,          // "app-mcp-wake:<token>"
}
impl Hub { pub fn set_waker(&self, w: Arc<dyn Waker>); }
```

默认实现 `SystemWaker`（`SystemWaker::action(&req)` 只生成动作不执行：`WakeAction::Command` 或 Windows 的
`WakeAction::ActivateApplication`）：命令用 `tokio::process` 执行固定程序、逐个传参，
不经 shell 拼接；令牌、scheme、AUMID、bundle id、D-Bus 名称、URL 都先校验字符集；子进程 stdio 全部重定向到空设备。

| kind | Windows | macOS | Linux |
|---|---|---|---|
| `uri` | `cmd.exe /d /c start "" <scheme>://app-mcp/wake?token=<t>` | `open -g <uri>` | `xdg-open <uri>` |
| `aumid` | `IApplicationActivationManager::ActivateApplication(<aumid>, "app-mcp-wake:<t>", AO_NONE)`（COM，不启动子进程；参数经 UWP `LaunchActivatedEventArgs.Arguments` / 打包桌面应用的命令行送达，App 已运行时交给现有实例） | — | — |
| `apple-event` | — | `open -g -b <bundle> --args app-mcp-wake:<t>`（参数仅在冷启动时送达） | — |
| `dbus` | — | — | `gdbus call --session --dest <name> --object-path </name/路径> --method org.freedesktop.Application.ActivateAction app-mcp-wake "[<'<t>'>]" "{}"` |
| `web-url` | `rundll32.exe url.dll,FileProtocolHandler <url>#app-mcp-wake=<t>` | `open <url>#…` | `xdg-open <url>#…` |
| `android-intent` | 不支持（`LAUNCH_FAILED`）：Android 上的 Hub 由厂商实现 `Waker` 发送显式广播 | | |
| `none` | `LAUNCH_FAILED` | | |

**唤醒器配置**（`HubConfig.waker: WakerConfig`；`app-mcp-host` 配置 `lifecycle.waker` / 命令行 `--waker`）：

| 值（JSON） | 行为 |
|---|---|
| `"system"`（默认） | `SystemWaker`（上表） |
| `"none"` | 不唤醒：休眠实例 / 未运行 App 的调用与资源读取直接返回 `APP_DISCONNECTED`（`details.launchUrl` 为清单 `launch.web`），不生成令牌、不发 `AppWaking`、不询问审批。休眠仍被接受（工具保持列出） |
| `{"exec": [program, ...args]}` | `ExecWaker`：执行 `program args…`（逐个传参，**不经 shell**）；`WakeRequest` 序列化为一行 camelCase JSON（`{"appId","instanceId","descriptor":{"kind","target","background"},"token","activationArg"}`）写入其 stdin 后关闭；stdout 丢弃，stderr 收集。退出码 0 = 已发出激活（随后等待回连），非 0 → `LAUNCH_FAILED`（附 stderr）；10 秒未退出视为已发出。`exec` 为空时 `Hub::start` 报错 |

`Hub::set_waker` 设置的实现覆盖配置（包括 `none`）。`ExecWaker` 适合厂商脚本、测试替身和平台上没有内置实现的激活方式。

**租约**：MCP 会话或 API 会话每次调用某实例（请求已送达）完成后，Hub 发送 `app/lease { ttlMs }`
（`0` 关闭租约：`HubConfig.lease_ttl = 0`）。MCP 会话关闭、`Hub::reset_session` 时向该会话租约过的实例发送
`ttlMs: 0`；若其他会话对同一实例仍有未到期租约，随后补发剩余时长。本节的"会话"指调用方（3.6「调用方与 Agent 任务」）：
无会话 MCP 请求的租约按 `principal:<主体>` 发放与统计，没有"会话结束"，只按到期、请求流空闲、`apps.release` 与任务空闲回收（`task_idle_ttl`）收回。

**自适应租约**（4e B2，spec/lifecycle.md 第 13 节；`HubConfig.lease: LeasePolicy`，默认开启）：

```rust
pub struct LeasePolicy {        // Default：adaptive true, window 20, margin 5 s, min 5 s, max 60 s, idle_revoke 30 s
    pub adaptive: bool,         // false = 每次固定 lease_ttl，只在会话结束时收回（4e 之前的行为）
    pub window: u32,            // 统计最近 N 个间隔（≥ 1）
    pub margin: Duration, pub min: Duration, pub max: Duration,   // min ≤ max
    pub idle_revoke: Duration,  // 0 = 不因请求流空闲收回
}
impl LeasePolicy { pub fn validate(&self) -> Result<(), String> }   // Hub::start 调用，不合法 → InvalidInput
pub struct LeaseOverrides {     // JSON 形式（camelCase，未知字段报错）：adaptive / window / marginMs / minMs / maxMs / idleRevokeMs
    pub fn apply(&self, p: &mut LeasePolicy); pub fn merge(&mut self, other: &LeaseOverrides);
}
```

- **间隔样本**：按（会话键, appId）统计：上一次调用完成（发租约）到下一次调用开始的时长。大于 `max` 的间隔视为一轮对话后的停顿，
  不计入。窗口保留最近 `window` 个。
- **租约时长**：样本 ≥ 3 个时 = clamp(p90（最近秩法）+ `margin`, `min`, `max`)；否则用 `lease_ttl`（无历史时的保守默认值）。
- **收回**：会话结束（MCP 会话关闭、`reset_session`）收回其全部租约并删除其统计；**请求流空闲**——会话没有进行中的请求（MCP 的
  `tools/list`、`tools/call`、`resources/list`、`resources/read`、`resources/subscribe`，Hub API 的调用 / 读取）且距最近一次请求活动
  （开始或结束）已达 `idle_revoke`——收回该会话以**默认值**发出、仍未到期的租约（`ttlMs: 0`，其他会话的未到期租约随后补发）。
  自适应租约本身就是对下一次调用的预测，按时到期，不提前收回（收回默认值部分后随 `ttlMs: 0` 补发其剩余时长）。
  SDK 取较大截止时刻（spec/lifecycle.md 4.2），样本凑满后发出的较短自适应租约不会缩短先前的默认值租约，因此 Hub 按（会话, 实例）
  分别记默认值与自适应租约的最晚截止，空闲收回看前者。没有待收回的会话时 Hub 不设定时器。
- **租约种类**：按统计发出的租约在 `app/lease` 上带 `adaptive: true`（默认值租约不带）；收回后补发时两种租约分别补发
  （其他会话的默认值部分只在晚于自适应部分时补发）。SDK 的后台连接只认自适应租约（spec/lifecycle.md 第 13 节 B4），Hub 不区分连接是否在后台。
- **内存上界**：（会话, App）统计与会话活动表各最多 1024 项，超出时淘汰最久未活动（会话：且无进行中请求）的一项。
- **观测**：`HubStatus.lease: Option<LeaseStatus>`（3.9）。
- **定位**：自适应租约是 Hub 的**缺省策略**，不是机制本身（CLAUDE.md「微内核范围」）：`--fixed-lease` / `LeasePolicy.adaptive = false`
  关闭它，回到固定 `lease_ttl`；之后可经第 16 项 P2 的策略挂点替换。Agent 的显式决定优先：`apps.release`（3.15）立即收回本会话在
  该 App 上的全部租约（默认值与自适应部分），`apps.activate` 发一次租约。

**新增配置**（`HubConfig`）：`lease_ttl`、`wake_timeout`、`wake_token_ttl`、`dormant_ttl`、

**新增配置**（`HubConfig`）：`lease_ttl`、`wake_timeout`、`navigate_timeout`（3.14，默认 5 秒）、`wake_token_ttl`、`dormant_ttl`、
`dormant_replaced_by_new_instance`、`wake_from_launch`、`waker`。`app-mcp-host` 对应命令行：`--lease-ms`、`--wake-timeout-ms`、
`--navigate-timeout-ms`（配置文件 `lifecycle.navigateTimeoutMs`）、`--wake-from-launch`、`--waker system|none|<JSON>`。

**功耗相关配置（4e，spec/lifecycle.md 第 11–12 节）**：

| `HubConfig` 字段 | 默认 | 含义 | `app-mcp-host` 命令行 / 配置文件（`lifecycle` 下） | hub-c / hub-node JSON |
|---|---|---|---|---|
| `wake_token_ttl` | 60 s | 唤醒令牌有效期（4.4） | `--wake-token-ttl-ms` / `wakeTokenTtlMs` | `wakeTokenTtlMs` |
| `wake_rate_limit: u32` | `DEFAULT_WAKE_RATE_LIMIT` = 6 | 每 App 每 60 秒滑动窗口内最多实际发出的唤醒激活次数；`0` 不限 | `--wake-rate-limit` / `wakeRateLimit` | `wakeRateLimit` |
| `legacy_heartbeat: bool` | `false` | 回退到旧心跳：忽略 SDK 的 `heartbeatMs` 声明，对所有连接发 `ping` 并按无消息断开 | `--legacy-heartbeat` / `legacyHeartbeat` | `legacyHeartbeat` |
| `lease: LeasePolicy` | 见上 | 自适应租约（B2） | `--fixed-lease`、`--lease-window`、`--lease-margin-ms`、`--lease-min-ms`、`--lease-max-ms`、`--lease-idle-revoke-ms` / `lease: {adaptive, window, marginMs, minMs, maxMs, idleRevokeMs}` | `lease`（同左对象；hub-c 不合法 → `AM_HUB_ERR_INVALID_CONFIG`） |

- **心跳（A3）**：握手后按 `app/hello.heartbeatMs` 决定——缺省（旧 SDK）或 `legacy_heartbeat`：每 `ping_interval` 发 `ping`，
  `idle_timeout` / `hidden_idle_timeout` 内无消息断开（原行为）；`0`（本地传输）：不发 `ping`、不做无消息断开，靠连接断开
  （EOF）感知，半开连接由调用的 `response_timeout` 发现；`> 0`：不发 `ping`，无消息断开取 `max(按可见性的配置值, 3 × heartbeatMs)`。
  握手前、等待配对确认期间与多路复用连接级空闲规则不变。
- **唤醒速率上限（O4）**：只对真正要发出激活的唤醒计数（加入已有等待、目标已就绪 / 握手中不计；激活任务开始前已被认领的撤销计数）。
  超出时不激活、不登记等待，调用以工具错误 `LAUNCH_FAILED` 结束，`data` 为 `{ appId, code: "WAKE_RATE_LIMITED", retryAfterMs }`，
  并记为该 App 的最近错误（`code = WAKE_RATE_LIMITED`，spec/protocol.md 10.1）。
- **资源订阅（B3，spec/lifecycle.md 第 13 节）**：实例休眠时 MCP 侧订阅关系保留；实例回连 `app/ready` 后对仍被订阅的资源重新发送
  `resources/subscribe`（同步时已发过的不重复）；读取只由休眠实例提供的资源先唤醒（受 `wake_rate_limit` 约束）。
  App 端是否因订阅保持在线由资源的 `realtime` 声明决定（SDK 侧），Hub 不因订阅拒绝 `app/sleep`。
- 其他绑定：hub-uniffi `HubConfig.wake_rate_limit: u32?`、`legacy_heartbeat: bool?`、`lease: LeaseConfig?`（字段同 JSON，`*_ms: u64?`，
  不合法 → `HubError::InvalidConfig`）；C# `HubOptions.WakeRateLimit`、`LegacyHeartbeat`、`Lease`（`LeaseOptions`：`Adaptive`、`Window`、
  `Margin` / `Min` / `Max` / `IdleRevoke` 为 `TimeSpan?`）；`@app-mcp/hub` `HubConfig.wakeRateLimit`、`legacyHeartbeat`、`lease: LeaseConfig`。

`Hub::reset_waker()`（补充方法）撤销 `set_waker`，恢复按 `HubConfig.waker` 构造的实现；各绑定清除自定义唤醒回调
（C `cb = NULL`、Node / uniffi `setWaker(null)`）时调用它，因此配置为 `none` / `exec` 时清除回调后仍按配置执行。

### 3.7 工具渐进暴露

App / 工具很多时，一次列出全部工具会占满模型上下文。Hub 支持只列出“入口”，模型按需展开：

```rust
pub enum ToolExposure { All, Progressive, Auto }   // serde："all" / "progressive" / "auto"
pub struct HubConfig {
    pub tool_exposure: ToolExposure,               // 默认 Auto
    pub tool_exposure_threshold: usize,            // 默认 40（DEFAULT_TOOL_EXPOSURE_THRESHOLD）
    // …
}
pub const TOOL_APPS_TOOLS: &str = "apps.tools";    // app_mcp_hub::mcp
```

- **是否生效**：`All` 从不；`Progressive` 总是；`Auto` 在 App 工具（注册表列出的，含静态、休眠）与上游工具总数
  **大于** `tool_exposure_threshold` 时生效。每次列出时重新判断（App 连接 / 断开会使结果变化，已有 `ToolsChanged` 覆盖）。
- **生效时的列表**（MCP `tools/list`、`Hub::tools`、`Hub::export_tools`）：内置工具 `apps.list`、`apps.select`、`apps.overview`、
  `apps.tools`、`apps.activate`、`apps.release`（有页面目录时另有 `apps.page`、`apps.navigate`，3.14 / 3.15），加上**会话已列出的 App** 的全部工具。会话已列出的 App =
  本会话调用过 `apps.tools` 的 App ∪ 本会话调用过其工具的 App（含上游；无论结果成功与否）∪ 本会话 `apps.select` 选定实例的 App ∪
  `Hub::select_instance` 全局选定实例的 App。
- **未生效时**：列表与之前完全相同（不含 `apps.tools`）；已列出的 App 仍照常记录，切换为生效时沿用。
- **会话**：MCP 出口为 `mcp:<n>`（无会话请求见下方「无会话请求的列表与总览」）；Hub API 由 `ToolFilter.session`（列表 / 导出）与 `CallRequest.session` /
  `dispatch_in_session` 的 `session`（调用）决定，二者用同一 ID 即对应同一会话。`reset_session` / MCP 会话结束时清除。
- **`ToolFilter.apps` 显式给出时**不受渐进暴露影响（列出这些 App 的全部工具），供厂商 UI 使用。
- **`apps.tools`**（`{appId}`，只读）：返回 `{appId, tools: HubTool[], message}`（`HubTool` 为 3.1 的 camelCase 形态，
  含 `inputSchema`）；appId 未知 → `TOOL_NOT_FOUND`。任何模式下都可调用。
- **通知**：会话已列出的 App 因 `apps.tools` / 调用 / `apps.select` 新增时，MCP 出口只向**该会话**发送
  `notifications/tools/list_changed`（不发 `HubEvent::ToolsChanged`）；`Hub::select_instance` 新增 / 移除全局选择且渐进暴露生效时
  按普通列表变化处理（合并后发 `ToolsChanged` 与所有会话的 `list_changed`）。Hub API 调用方在每轮对话重新 `export_tools` 即可。
- **路由不变**：未列出但存在的工具按全名（或导出名）仍可调用；导出名按全部工具（含 `apps.tools`）计算，展开前后稳定。
- **`instructions`**：MCP `initialize` 时渐进暴露已生效，则在 7.2 的文本末尾追加一句说明（先调用 `apps.tools`，也可按全名直接调用）。

**无会话请求的列表与总览**（第 12 项 S5，docs/plans/12-mcp-stateless.md 3.3；单一定义，以上各条只适用于 legacy 会话与 Hub API）：

- **列表规则**：无会话请求（`principal:<主体>`，3.6）的 `tools/list` / `resources/list` 只是注册表与 App 连接状态、全局选择
  （`Hub::select_instance`）、策略 `hide`、请求主体与 Hub 配置的函数，**不读**调用方任务上的展开记录与 `apps.select` 选择——
  两次 `tools/list` 之间夹任意 `apps.tools` / 工具调用 / `apps.select` / `apps.overview`，结果逐字节相同（MCP 2026-07-28：列表不随
  其他请求的副作用变化，SEP-2567）。列表顺序确定：内置工具在前，App 按 appId、上游按名称（均为有序表）。
  内置工具另含 `apps.task.begin` / `apps.task.end` 与可选 `taskId` 参数（3.6「任务句柄」；`max_task_handles = 0` 时不含）。
  已知例外：同一 App 有多个已连接实例且都未聚焦时，`view` 工具取首选实例界面上的，而首选实例的"最近活跃"含"最近一次完成调用"
  （routing.rs，全局实例状态），调用可能改变列出哪个实例的 `view` 工具（docs/plans/12-mcp-stateless.md S5 记录 U8）。
- **暴露方式**：`HubConfig.stateless_tool_exposure: ToolExposure`，**默认 `All`**（全部列出，不含 `apps.tools`）。设为
  `Progressive` / `Auto`（阈值同 `tool_exposure_threshold`）且生效时列表 = 内置工具（含 `apps.tools`）+ 全局选定实例的 App 的工具；
  `apps.tools` 只返回定义、不改变列表，模型须按全名调用未列出的工具。
  @why 默认 `All`：实测（2026-10-02，Claude Code 2.1.281 以 2026-07-28 连接，docs/plans/12-mcp-stateless.md U3）客户端只能调用
  `tools/list` 中列出的工具——`Progressive` 下模型经 `apps.tools` 看到定义后仍无法调用未列出的工具（Host 未收到该调用）。
  因此 modern 下渐进暴露只适合自己实现客户端、能按名调用的 Agent；通用客户端保持 `All`。
- **缓存提示**（SEP-2549）：无会话请求的 `tools/list`、`resources/list`、`resources/templates/list`、`server/discover` 结果带
  `ttlMs` = `HubConfig.stateless_list_ttl`（默认 `DEFAULT_STATELESS_LIST_TTL` = 5 秒）与 `cacheScope: "private"`；`resources/read`
  带 `ttlMs: 0`、`cacheScope: "private"`。legacy 会话的结果不带这两个字段（线上格式不变）。
- **总览**：不在调用结果中"首次附带"（无会话可去重），改为——`server/discover` 的 `instructions`：各 App 一句话简介（同 7.2）+
  "调用 `apps.overview` 查看完整总览（`apps.tools` 的结果也附带）"，渐进暴露生效时另说明列表不随调用变化；`apps.tools`：每次在内容
  最前附带该 App 的总览文本（格式同 spec/protocol.md 7.3，说明句为"可用 apps.overview 重新查看"），`structuredContent` 另有
  `overview` 字段（`apps.overview` 的结构）；`apps.overview` 不变。无会话调用方的任务不记"已附带"。
- **通知**：无会话请求不登记 peer，经 `subscriptions/listen` 流收 `list_changed`（3.6「通知」，S7）；`Hub::select_instance` 在 legacy 或
  无会话渐进暴露生效时按普通列表变化处理。
- **配置入口**（3.6 `task_idle_ttl` / `principal_select_ttl` 与本节 `stateless_tool_exposure` / `stateless_list_ttl`，缺省均取 Hub 默认值）：

  | 绑定 | `task_idle_ttl` | `stateless_tool_exposure` | `principal_select_ttl` | `stateless_list_ttl` |
  |---|---|---|---|---|
  | `app-mcp-host` 配置文件 | `lifecycle.taskIdleTtlMs` | `tools.statelessExposure` | `lifecycle.principalSelectTtlMs` | `tools.statelessListTtlMs` |
  | `app-mcp-host` 命令行 | `--task-idle-ttl-ms` | `--stateless-tool-exposure` | `--principal-select-ttl-ms` | （仅配置文件） |
  | hub-c（头文件 v15）/ hub-node / `@app-mcp/hub` JSON | `taskIdleTtlMs` | `statelessToolExposure` | `principalSelectTtlMs` | `statelessListTtlMs` |
  | hub-uniffi `HubConfig` | `task_idle_ttl_ms: u64?` | `stateless_tool_exposure: ToolExposure?` | `principal_select_ttl_ms: u64?` | `stateless_list_ttl_ms: u64?` |
  | C# `HubOptions` | `TaskIdleTtl` | `StatelessToolExposure` | `PrincipalSelectTtl` | `StatelessListTtl` |

配置入口：`app-mcp-host` 配置文件 `tools: {exposure, threshold}`、命令行 `--tool-exposure auto|progressive|all`、
`--tool-exposure-threshold <N>`；C / Node 配置 JSON `toolExposure`、`toolExposureThreshold`（同时新增 `waker`：`"system"` /
`"none"` / `{"exec": [...]}`）；uniffi `HubConfig.tool_exposure: ToolExposure?`、`tool_exposure_threshold: u32?`、
`waker: WakerConfig?`（`System` / `Disabled` / `Exec { argv }`，`Disabled` 即 `"none"`）、`ToolFilter.session`。

### 3.8 本地 IPC 传输

原生 App 默认经本地 IPC（Unix 域套接字 / Windows 命名管道）连接 Hub，网页只能用回环 TCP WebSocket。
端点格式、默认位置、连接鉴权与单实例语义见 spec/protocol.md 第 1 节；两种传输上的消息完全相同。

- `HubConfig.ipc_endpoint: Option<String>`：`unix:<绝对路径>` / `pipe:\\.\pipe\<名称>`，默认
  `app_mcp_protocol::endpoint::default_ipc_endpoint()`（Android / iOS 为 `None`）；`None` = 不开。
  不是 IPC 形式、或本平台不支持时 `Hub::start` 返回 `InvalidInput`；端点已有 Hub 监听时返回 `AddrInUse`。
  Unix 套接字路径超过 `sockaddr_un.sun_path` 上限（`app_mcp_protocol::endpoint::MAX_UNIX_SOCKET_PATH_BYTES`，Linux 107 / macOS 103 字节）
  时在建目录之前返回 `InvalidInput`，错误内含 `ConnectionIssue { code: IPC_PATH_TOO_LONG, .. }`（`err.get_ref()` 可 downcast；
  `Display` 为 `[IPC_PATH_TOO_LONG] …（N 字节，本平台上限 M 字节）：<路径>。建议：…`）。
  与 `listen` 一样，默认值会占用本机唯一的端点：同机器上的第二个 Hub（含测试）应改用其他端点或设为 `None`。
- `Hub::ipc_endpoint()`：实际监听的端点字符串，可直接作为原生 SDK 的 `host_url`。
- `InstanceInfo.pid: Option<u32>`：IPC 连接的对端进程号（操作系统提供）；TCP 连接与休眠实例为 `None`。
  JSON 中为 `pid`，缺省时省略。

绑定：C 配置 JSON `ipcEndpoint`（缺省 = 平台默认端点，`null` = 不开）+ `am_hub_ipc_endpoint`（头文件 v4）；
Node 配置 `ipcEndpoint`（同上）+ `Hub.ipcEndpoint`；uniffi `HubConfig.ipc_endpoint: String?` + `enable_ipc: bool`（默认 `true`）+
`AppMcpHub.ipc_endpoint()` + `InstanceInfo.pid: u32?`；C# `HubOptions.IpcEndpoint` / `DisableIpc` + `AppMcpHub.IpcEndpoint`；
Kotlin `Hub.ipcEndpoint`、Swift `Hub.ipcEndpoint`、Python `Hub.ipc_endpoint`。
`app-mcp-host`：配置文件 `ipcEndpoint`（`"none"` 关闭）、命令行 `--ipc-endpoint <ENDPOINT|none>`。`/healthz` 带 `ipcEndpoint`。

**MCP over IPC**：`mcp_http` 为 `true` 时 IPC 端点上同样提供 `/mcp`（同一个 `Router`，spec/protocol.md 1.3），供厂商 Agent /
支持本地套接字的 MCP 客户端使用。IPC 上不校验令牌（连接级鉴权已确认对端是同一用户，同一用户本可读取令牌文件）；
客户端应核对监听方是同一用户（Unix `SO_PEERCRED` / Windows 管道所有者 SID）。示例：`crates/hub/tests/mcp_ipc.rs`——
Unix 用 rmcp 的 `UnixSocketHttpClient` + `StreamableHttpClientTransport::with_client`（URL `http://localhost/mcp`）完成
initialize → tools/list → tools/call；Windows 用 hyper 客户端经命名管道发 `initialize`。不提供 stdio→HTTP 转发程序。

### 3.9 诊断：运行状态、连接 ID、错误码

```rust
impl Hub { pub fn status(&self) -> HubStatus; }        // GET /status 返回同样的 JSON（camelCase）
pub struct HubStatus {
    #[serde(flatten)] identity: HostIdentity,          // service / version / user / pid
    listen: Option<String>, ipc_endpoint: Option<String>, started_at_ms: u64,
    mcp_http: bool, auth: AuthStatus,                  // { token_configured, token_required_without_origin }
    mcp_sessions: usize,                               // legacy MCP 会话数
    mcp_listen_streams: Option<usize>,                 // 第 12 项 S7：subscriptions/listen 流数（3.6「通知」；旧 Host → None）
    apps: Vec<AppStatus>,                              // 按 appId 排序，含上游
    reports: Vec<DiagnosticReport>,                    // 最近 32 条 SDK 上报（MAX_REPORTS），旧的在前
    lease: Option<LeaseStatus>,                        // 4e B2：租约策略与统计（旧 Host 无此字段 → None）
    limits: Option<LimitOverrides>,                    // 第 14 项：资源保护策略（3.11；旧 Host → None）
    output_validation: Option<OutputValidation>,       // 第 19 项 R2（3.11；旧 Host → None）
    dormant_store: Option<DormantStoreStatus>,         // 4f G11：休眠记录持久化（3.5；未配置 state_dir / 旧 Host → None）
    tasks: Option<Vec<AgentTaskStatus>>,               // 第 12 项 S6：Agent 任务（3.6），按调用方键排序；只读；旧 Host → None
}
pub struct AgentTaskStatus { id: String,               // task-<128 位十六进制>
    caller: String,                                    // 调用方键 mcp:<n> | principal:<主体> | api | api:<session>
    kind: CallerKind,                                  // mcpSession | principal | api
    selections: Vec<TaskSelectionStatus>,              // 未过期的 apps.select：{ app_id, instance_id, expires_in_ms: Option<u64> }（主体级才有有效期）
    leases: Vec<TaskLeaseStatus>,                      // 未到期且实例仍连接的租约：{ connection_id, expires_in_ms }
    inflight: u32,                                     // 进行中的请求数
    idle_ms: Option<u64> }                             // 距最近一次请求活动；无活动记录时省略
pub struct DormantStoreStatus { dir: String,           // <state_dir>/dormant
    loaded_instances, expired_instances, writes: u64,  // 启动时读回 / 因过期丢弃的实例数；启动以来写入（含删除）次数
    issues: Vec<StoreIssue>,                           // 启动时跳过的文件 { file, reason }
    last_error: Option<String> }                       // 最近一次写入失败
pub struct LeaseStatus { mode: String,                 // adaptive | fixed | off（lease_ttl = 0）
    default_ms, min_ms, max_ms, margin_ms: u64, window: u32, idle_revoke_ms: u64,
    adaptive_grants, default_grants, revoked_session_end, revoked_idle: u64,
    pairs: Vec<LeasePairStatus> }                      // { session, app_id, samples: u32, next_ttl_ms: u64, adaptive: bool }
pub struct AppStatus { app_id, name, kind: AppKind, state: AppState,   // connected | waking | dormant | disconnected
    instances: Vec<InstanceStatus>,                    // InstanceInfo（flatten）+ state: connected | dormant | waking
                                                       //   + power: Option<InstancePower>（4e，见下）
    last_error: Option<LastError>,                     // { code: Option<String>, message, at_ms }
    wakes: u64,                                        // 4e：Hub 启动以来为该 App 实际发出的唤醒激活次数（含冷启动）
    rate_limited: u64, too_large: u64,                 // 第 14 项：启动以来因限流 / 大小上限被拒绝的次数（3.11）
    tools: Vec<ToolDeclaration> }                      // 第 14 项 S5：各工具的 risk 与注解（声明原样 + 实际生效），旧 Host 为空
pub struct InstancePower {                             // 4e 功耗观测（spec/lifecycle.md 第 12 节），JSON 字段可选新增
    reconnects: u64,                                   // 同一 instanceId 完成握手的次数 - 1（Hub 启动以来）
    wakes: u64,                                        // 以该休眠实例为目标实际发出的唤醒激活次数
    online_secs: u64,                                  // 累计在线秒数（含当前连接）
    heartbeats: u64,                                   // Hub 发出的 ping + 收到 SDK 的 ping
    heartbeat_ms: Option<u64>,                         // SDK 声明（app/hello.heartbeatMs）；None = 旧 SDK
    lifecycle_mode: Option<LifecycleMode>,             // SDK 声明（app/hello.lifecycleMode）
    awake_reasons: Vec<AwakeReason> }                  // 已连接实例当前不能休眠的原因：persistent | call | lease | subscription | wake-pending
                                                       // subscription 只计声明 realtime 的资源的订阅（4e B3）
pub struct DiagnosticReport { app_id, instance_id, connection_id, code, message, count: u32, received_at_ms: u64 }
```

- **最近错误**（`last_error`）：握手被拒（带 spec/protocol.md 10.1 的错误码）、配对被拒、唤醒失败 / 超时（工具错误类别，如
  `APP_NOT_RESPONDING`）；上游为进程错误。只因握手被拒出现过的 appId 也列出（`disconnected`）。appId 不合法的握手不记录。
- **SDK 上报**：SDK 的 `app/diagnostic` 通知（spec/protocol.md 10.2）记入 `reports`，同时发 `HubEvent::AppDiagnostic` 并记 warn 日志。
- **连接 ID**：Hub 启动时生成 6 位十六进制标记，每条 App 连接（多路复用时每个通道）的 ID 为 `<标记>-<序号>`，经
  `HelloResult.connectionId` 返回 SDK，`InstanceInfo.connection_id` 与日志字段 `cid` 中相同；MCP 会话为 `mcp-<序号>`。
- **拒绝错误码**：握手被拒时 `HelloResult.code`（`PROTOCOL_INCOMPATIBLE` / `ORIGIN_NOT_ALLOWED` / `INVALID_HELLO` / `REJECTED`），
  配对被拒时 `PairingResultParams.code = PAIRING_REJECTED`。
- **功耗观测**：按 `(appId, instanceId)` 计数，跨重连与休眠保留，Hub 重启清零；最多 1024 个实例（超出时淘汰最久未活动的离线实例），
  休眠记录过期 / 被新实例替换时移除。`awake_reasons` 只含 Hub 可见的原因——App 的 `hold()` 只有 SDK 知道，不在其中。
  `app-mcp-host doctor` 的「App 实例」检查逐实例显示（文本一行摘要，`--json` 在 `details` 中原样给出）。hub-c（JSON）与
  hub-node / `@app-mcp/hub`（`InstanceStatus.power?`、`AppStatus.wakes?`、`HubStatus.lease?`）带上这些字段；hub-uniffi
  `InstanceStatus.power: InstancePower?`（`LifecycleMode`、`AwakeReason` 枚举）、`AppStatus.wakes`、`HubStatus.lease: LeaseStatus?`；C#
  `InstanceStatusInfo.Power`（`InstancePowerInfo`，原因字符串见 `HubAwakeReasons`）、`AppStatusInfo.Wakes`、`HubStatusInfo.Lease`
  （`LeaseStatusInfo`）。`app-mcp-host doctor` 另有「租约」检查（策略、发出 / 收回次数、各（会话, App）下一次租约）。
- `app-mcp-host doctor` / `status` 经本地 IPC（核对监听方用户后）读 `/status`；IPC 关闭时改用 TCP + 令牌。

绑定：

| 绑定 | `status()` | `InstanceInfo.connection_id` | `AppDiagnostic` 事件 |
|---|---|---|---|
| hub-c | `am_hub_status_json`（HubStatus JSON，与 `/status` 相同） | JSON `connectionId` | `{"type":"appDiagnostic","appId","instanceId","code","message","count"}` |
| hub-node / `@app-mcp/hub` | `hub.status(): HubStatus`（TS 类型） | `connectionId?` | 同上（`HubEvent` 联合） |
| C#（`AppMcp.Hub`，基于 hub-c） | `Status()`（`HubStatusInfo`）/ `GetStatus()`（`JsonElement`） | `ConnectionId` | `HubEventTypes.AppDiagnostic` |
| hub-uniffi（Kotlin / Swift / Python） | `status()` → `HubStatus` 记录（已停止时 `HubError::Shutdown`） | `connection_id` 字段 | 专门变体 `HubEvent::AppDiagnostic { app_id, instance_id, code, message, count }` |

uniffi 的 `HubStatus` 把 `identity` 展开为 `service` / `version` / `user` / `pid` 四个字段，`InstanceStatus` 为 `{ info: InstanceInfo, state }`
（JSON 中 `info` 为 flatten），`mcp_sessions` 为 `u64`；其余字段与 JSON 一一对应。

Agent 任务（`HubStatus.tasks`，第 12 项 S6）与审批的 `principal` / `client_name`（3.3）：hub-c 为 JSON 原样（头文件 v15）；
`@app-mcp/hub` `HubStatus.tasks?: AgentTaskStatus[]`（`CallerKind`、`TaskSelectionStatus`、`TaskLeaseStatus`）、`ApprovalRequest.principal?` /
`clientName?`；hub-uniffi `HubStatus.tasks: [AgentTaskStatus]?`（`kind: CallerKind` 枚举 `McpSession` / `Principal` / `Api`）、
`ApprovalRequest.principal` / `client_name: String?`（Kotlin / Swift / Python 封装层以同名类型别名导出）；C# `HubStatusInfo.Tasks`
（`AgentTaskStatusInfo`，`Kind` 为字符串）、`ApprovalRequest.Principal` / `ClientName`。`app-mcp-host status` 的一行摘要与 doctor
「App 实例」检查显示 Agent 任务数（旧 Host 不报告时省略）。

### 3.10 cargo features（能力裁剪）

`app-mcp-hub` 默认 `["mcp-server", "upstream", "schema-validation", "dbus"]`，与此前行为、公开 API 完全一致。关闭某项时 `HubConfig`
字段与方法签名保留（各绑定源码不需改动），用到该能力时返回明确错误；检查在 `Hub::start` 开头（`features` 模块，常量
`features::{MCP_SERVER, UPSTREAM, SCHEMA_VALIDATION}`）。

| feature | 内容 | 关闭时 |
|---|---|---|
| `mcp-server` | MCP 出口：`/mcp`（Streamable HTTP）、`serve_http(_with)`、`serve_stdio`、`McpSession` | `mcp_http = true` / `serve_http(_with)` → `ErrorKind::Unsupported`（说明缺哪个 feature）；`/mcp` 404；`mcp` 模块、`McpSession`、`Hub::mcp_session`、`Hub::serve_stdio` 不编译 |
| `upstream` | 上游聚合：以子进程启动其他 MCP 服务器并汇入工具 | `upstreams` 非空 → `Unsupported`；`UpstreamConfig` 与配置解析保留 |
| `dbus` | Linux 名字服务连接器 `connector::DbusConnector`（zbus，3.16）；鸿蒙（`*-linux-ohos`）与非 Linux 平台不编译 | `DbusConnector` 不存在；`HubConfig::connectors` 仍可放自定义连接器 |
| `schema-validation` | 调用前按 inputSchema 校验参数（jsonschema） | `schema::check` 返回 `SchemaCheck::Unchecked`，参数原样交给 App（由 App 的处理函数报参数错误；与 spec/protocol.md 第 6 节"Host 校验参数"不同） |

各绑定把 `ErrorKind::Unsupported` 映射为**专门的错误类别**（不与其他 I/O 错误混在一起；说明文字含缺少的 feature 名，重试无效）：

| 绑定 | 类别 | 其他启动 / 绑定错误 |
|---|---|---|
| hub-uniffi（Kotlin / Swift / Python） | `HubError::Unsupported { detail }`（Kotlin `dev.appmcp.hub.ffi.HubException.Unsupported`——嵌套类别不能经 `dev.appmcp.hub.HubException` typealias 访问；Swift `.Unsupported(detail:)`；Python `HubError.Unsupported`） | `Io { detail }` |
| hub-c | `AM_HUB_ERR_UNSUPPORTED = 10`（`am_hub_start`、`am_hub_serve_http`；说明经 `am_hub_last_error_message`） | `AM_HUB_ERR_IO` |
| hub-node / `@app-mcp/hub` | `HubError.kind` / `code` = `'UNSUPPORTED'`（`BindingErrorCode`） | `'START_FAILED'` |

这些类别为新增（2026-10-01）：此前同类错误分别报 `Io` / `AM_HUB_ERR_IO` / `START_FAILED`。`AM_HUB_API_VERSION` 不变（只新增枚举值）。

HTTP 服务（`/app`、`/healthz`、`/status`）与本地 IPC 始终编译：移动端 App 也经 `ws://127.0.0.1:7717/app` 连接 Hub。
rmcp 的 `server` / `client` 始终开启（模型类型与 `Peer`）。

`bindings/hub-uniffi` 的组合：默认 `["cli", "desktop"]`（`desktop` = 全部能力，jar / wheel / Swift 包用）；`mobile` = 不含上述三项；
`mcp-server`（2026-10-02 拆出，`desktop` 包含）= MCP 出口（含 `serve_mcp_fd`），独立 Hub App 用 `--android-features mobile,schema-validation,mcp-server`
（arm64 mobile-release 9.48 MB，比默认组合 7.71 MB 多约 1.8 MB，gzip 3.47 / 2.91 MB）。
`scripts/generate.sh --android` 用 `--no-default-features --features mobile,schema-validation`（保留 Hub 侧参数校验）输出到
`sdks/kotlin/app-mcp-hub-android`（给嵌入 Hub 的厂商），`--android-features <list>` 可改（体积优先用 `mobile`，去掉校验）；
`--hub-app`（2026-10-02）另编一份 `mobile,schema-validation,mcp-server` 输出到 `sdks/kotlin/hub-app-android/src/main/jniLibs`，
独立 Hub App 打包时以这份为准（`packaging.jniLibs.pickFirsts`，缺少所请求 ABI 的这份时构建失败）。
运行时查询编进的能力：hub-uniffi `hub_features() -> HubFeatures { mcp_server, upstream, schema_validation }`（Kotlin `Hub.features()`）；
独立 Hub App 在 Agent `open()` 时检查 `mcp_server`，缺少时回 `HUB_UNSUPPORTED`（spec/naming.md 第 12 节）。各组合导出的 uniffi 接口相同。arm64（mobile-release）：完整 8.98 MB（gzip 3.26）、
`mobile,schema-validation` 6.66 MB（2.52）、`mobile` 3.92 MB（1.56）。

### 3.11 资源保护：限流与大小上限（第 14 项 S3 / S4）

保护 App 与设备（调用频率、数据大小）是本库职责；是否确认、是否放行不是（docs/plans/14-safety.md 第 1 节）。超出时返回明确错误，
不静默丢弃、不截断。错误码（`RATE_LIMITED` -32016、`PAYLOAD_TOO_LARGE` -32017）与 `data` 字段的唯一定义见 spec/protocol.md 第 4 节。

| `HubConfig` 字段 | 默认 | 含义 | `app-mcp-host` 命令行 / 配置文件 | hub-c / hub-node JSON |
|---|---|---|---|---|
| `limits.tool_rate: RateLimit` | 每分钟 120 次、突发 30 | 每（App, 工具）的令牌桶：每分钟补充 `per_minute` 个，最多攒 `burst` 个；`per_minute = 0` 不限 | `--tool-rate-limit`、`--tool-rate-burst` / `limits.toolRatePerMinute`、`toolRateBurst` | `limits.toolRatePerMinute`、`toolRateBurst` |
| `limits.app_rate: RateLimit` | 每分钟 600 次、突发 60 | 每 App（所有工具合计）的令牌桶 | `--app-rate-limit`、`--app-rate-burst` / `limits.appRatePerMinute`、`appRateBurst` | 同左 |
| `limits.max_arguments_bytes: u64` | 1 MiB | 调用参数序列化后的字节上限；0 不限 | `--max-arguments-bytes` / `limits.maxArgumentsBytes` | 同左 |
| `limits.max_result_bytes: u64` | 4 MiB | App 回给 Hub 的整个结果（含 `summary`）序列化后的字节上限；上游结果同样适用；0 不限 | `--max-result-bytes` / `limits.maxResultBytes` | 同左 |
| `limits.max_resource_bytes: u64` | 4 MiB | 资源内容（文本 / base64）的字节上限；0 不限 | `--max-resource-bytes` / `limits.maxResourceBytes` | 同左 |
| `output_validation: OutputValidation` | `Log` | App 工具的结果与声明的 `outputSchema` 不符时（上游工具不核对）：`Off` 不校验 / `Log` 只记 warn 日志、照常返回 / `Reject` 调用以 `HANDLER_ERROR` 结束（`details.outputSchemaError`）。无返回值不校验；未启用 `schema-validation` 时不校验 | `--output-validation off\|log\|reject` / `tools.outputValidation` | `outputValidation` |

- JSON 形式 `LimitOverrides`（`{toolRatePerMinute, toolRateBurst, appRatePerMinute, appRateBurst, maxArgumentsBytes, maxResultBytes,
  maxResourceBytes}`，缺省字段取默认，未知字段报错）为各绑定与配置文件共用；`per_minute > 0` 而 `burst = 0` 时 `Hub::start` 返回
  `InvalidInput`（host：配置无效；hub-c：`AM_HUB_ERR_INVALID_CONFIG`）。默认值宽松：只拦失控循环与异常数据，均低于 WebSocket
  单条消息 64 MiB 的上限（tungstenite 默认，超过时连接被断开而不是返回错误）。
- 检查顺序：参数大小 → 两级限流（两级都有令牌才各扣一个；任一级不足都不扣，`retryAfterMs` 取较长的等待）→ 路由 / 审批 / 唤醒
  （被限流的调用不会唤醒 App、不会触发审批）→ 转发 → 结果大小 → `outputSchema` 核对（只对 App 工具；上游工具的结果只检查大小）。`result` 超限时调用可能已在 App 内执行（错误信息如实说明）。
- 计数：`AppStatus.rate_limited` / `too_large`（Hub 启动以来被拒绝的次数）；`HubStatus.limits`（`LimitOverrides`，全部字段给出）与
  `HubStatus.output_validation`。令牌桶表最多 4096 个、计数表最多 1024 个 App（超出时淘汰，见 `crates/hub/src/limits.rs`）。
- `app-mcp-host doctor`：「资源保护」检查显示策略与各 App 被拒绝次数（有拒绝时为注意）；「工具声明」检查逐个列出每个工具的
  `risk` 与 Agent 实际看到的注解（`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint` / `title`，注明是声明的还是按
  `risk` 推导的、是否有 `outputSchema`），`--json` 的 `details` 原样给出 `AppStatus.tools`（`ToolDeclaration { name, risk, annotations?, effective, output_schema }`）。

### 3.12 进度与取消（第 16 项 O2）

App 报告进度的消息与 SDK 行为见 spec/protocol.md 3.3（唯一定义）；Hub 侧只做转发与保护 Agent：

- **接收方**：MCP 出口——`tools/call` 请求带 `_meta.progressToken` 时，进度以 `notifications/progress { progressToken, progress,
  total?, message? }` 发给该会话（rmcp `Peer::notify_progress`）；Hub API——`Hub::call_tool_with_progress` 的 `progress` 通道收到
  `ProgressUpdate { progress, total, message }`。没有接收方时（普通 `call_tool`、MCP 请求不带 token）进度被丢弃。上游 MCP 服务器的进度不转发。
- **合并**：`HubConfig::progress_interval`（默认 250 ms；`app-mcp-host` 配置文件 `tools.progressIntervalMs`）——两次转发至少间隔该时长，
  间隔内只保留最新一条、到期再发；0 = 不合并。任何情况下不递增的进度都丢弃（MCP 要求递增），`message` 截断到 200 字符
  （`crates/hub/src/progress.rs`）。调用结束时未发出的进度丢弃。
- **各语言绑定**（与 `call_tool` 同语义，只多一个进度回调；不传回调时等同于普通调用、进度丢弃；回调全部先于结果送达，结果之后不再有进度）：
  - C（`app_mcp_hub.h` v11）：`typedef void (*AmHubProgressFn)(void *user_data, char *progress_json)`（`{"callId","progress","total"?,"message"?}`）；
    `am_hub_call_with_progress(hub, request_json, cb, on_progress, user_data, out_call_id)`，`on_progress` 可为 NULL，与 `cb` 共用
    `user_data`，二者在同一分发线程串行。
  - uniffi：`async fn call_tool_with_progress(request: CallRequest, listener: ProgressListener) -> CallOutcome`，
    `callback interface ProgressListener { on_progress(update: ProgressUpdate) }`，`record ProgressUpdate { progress, total?, message? }`；
    回调在专用线程上按序执行，panic 被捕获并跳过该条；future 被丢弃时取消调用（同 `call_tool`）。
  - Kotlin `hub.callTool(..., onProgress: ((ProgressUpdate) -> Unit)? = null)`、Swift `callTool(..., onProgress: (@Sendable (ProgressUpdate) -> Void)? = nil)`
    （在 Hub 进度线程上调用，须尽快返回）；Python `await hub.call_tool(..., on_progress=callable)`（在调用方事件循环上按序调用，
    返回 awaitable 时调度为任务，异常记日志后忽略）。
  - Node 原生模块 `callToolWithProgress(requestJson, onProgress)`（弱 threadsafe function，不阻止进程退出）；`@app-mcp/hub`
    `hub.callTool(req, { onProgress })`，回调抛错交给 `onListenerError`，不影响调用。
  - .NET `AppMcpHub.CallAsync(request, IProgress<CallProgress>? progress, CancellationToken ct = default)`，
    `record CallProgress(string CallId, double Progress, double? Total, string? Message)`。
  - 合并间隔配置：C `am_hub_start` 配置 `progressIntervalMs`、Node / `@app-mcp/hub` `progressIntervalMs`、.NET `HubOptions.ProgressInterval`
    （缺省 250 ms，即 `HubConfig::progress_interval`）；uniffi `HubConfig` 暂未提供，用默认值。
- **路由**：只接受被路由到该调用的那条 App 连接发来的 `tools/progress`，其他连接以同一 `callId` 发来的进度忽略（记 warn 日志）。
- **取消**：MCP `notifications/cancelled`（rmcp 取消请求的 `CancellationToken`）与 `Hub::cancel_call` 共用一条路径：等待审批 / 唤醒中
  直接结束；已转发给 App 时发送 `tools/cancel`，SDK 取消 handler（各语言的取消信号 / 监听），调用以 `CANCELLED` 结束。

### 3.13 策略挂点（第 16 项 P2、第 18 项 L5）

类比 LSM：本库只提供**执行点**，不内置任何判断；规则由用户（常驻 Host）或厂商（嵌入式 Hub）写。**没有规则时所有执行点直接放行，
行为与没有策略时完全一致**。按 Agent 区分的规则依赖 Agent 任务对象（docs/plans/16-agent-os.md P1），尚未支持。
实现：`crates/hub/src/policy.rs`（纯数据与匹配）。

**执行点**（`PolicyHook`）：

| 执行点 | 位置 | 使用的动作 |
|---|---|---|
| `list` | MCP `tools/list`、`resources/list`、`instructions` 中的 App 简介；`apps.list` / `apps.tools` / `apps.overview` / `apps.page`（页面目录：去掉被隐藏的工具，工具全部被隐藏的页面不列出，3.14）；Hub API `apps()` / `tools()` / `export_tools()` / `resources()` / `overview()` | `hide` |
| `call` | 名称解析之后，资源保护（3.11）、审批（3.3）、唤醒之前；App 工具与上游工具（内置 `apps.*` 不受影响） | `hide`、`deny` |
| `wake` | 调用 / 资源读取需要唤醒休眠或未运行的 App 时，发起唤醒之前（3.5） | `deny` |
| `handle` | 数据句柄访问（第 17 项）：只定义执行点，尚未接入；规则写 `handle` 时校验报错 | — |

**动作**：

- `hide`：App / 工具不出现在任何列表中；调用按不存在处理（App 整体隐藏 = appId 未知：Hub API `Err(TOOL_NOT_FOUND)`；工具隐藏 =
  结果中的 `TOOL_NOT_FOUND`），资源读取 / 订阅按 `RESOURCE_NOT_FOUND`，隐藏 App 的资源变化不再通知；`apps.select` 隐藏的 App 与没有该实例相同。
  错误信息与真正不存在时相同，不提及规则。`hide` **只能全局生效**（MCP 2026-07-28 要求列表不得按连接变化）。
  `/status`、`doctor`、`HubEvent` 仍包含被隐藏的 App（诊断用）。App 提供的总览正文按原样给出（本库不改写 App 的文字）。
- `deny`：可见，在 `hooks` 指定的执行点拒绝，返回 `POLICY_DENIED`（错误码与 `data` 的唯一定义见 spec/protocol.md 第 4 节；只附命中规则的
  `id`，不附规则内容）。被拒绝的调用不转发、不唤醒、不触发审批、不消耗限流令牌。

**规则**（`PolicyConfig`，`HubConfig.policy`、`Hub::set_policy`、`<home>/policy.json`、各绑定共用的 JSON 形式，未知字段报错）：

```json
{ "rules": [
  { "id": "hide-notes", "action": "hide", "app": "notes" },
  { "id": "no-admin",   "action": "hide", "app": "shop", "tool": "admin.*" },
  { "id": "no-destroy", "action": "deny", "app": "*", "annotations": { "destructiveHint": true } },
  { "id": "no-wake-music", "action": "deny", "app": "music", "hooks": ["wake"] }
] }
```

| 字段 | 含义 |
|---|---|
| `id` | `[A-Za-z0-9_.-]{1,64}`，规则集中唯一 |
| `action` | `hide` / `deny` |
| `app` | appId（或上游名）：精确名，或以 `*` 结尾的前缀（`shop*`）；`*` 匹配全部。`*` 只能在末尾 |
| `tool` | 可选，工具局部名（不含 appId），规则同 `app` |
| `annotations` | 可选，`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint` 的布尔值（至少一项）：每项都与工具的注解（`HubTool.annotations`，Agent 实际看到的）相等才命中；工具**没有声明**该项时不命中（本库不按 MCP 缺省值推断）。这是用户的规则引用 App 的声明，本库不推断风险、不改写声明 |
| `hooks` | 只用于 `deny`：`call` / `wake` 的非空子集，缺省 `["call"]`。`hide` 不能写 |

- 作用范围：`tool` 与 `annotations` 都缺省 → 整个 App（`hide` 时连同资源、`apps.list` 中的条目；不针对具体工具的唤醒——资源读取——只匹配这类规则）；
  否则只作用于匹配的工具（`apps.list` 中实例的 `tools` 与 `staticToolCount` 同样去掉被隐藏的工具）。
- 匹配顺序：按规则顺序，`hide` 先于 `deny`；`deny` 取第一条命中的规则。规则最多 `MAX_POLICY_RULES`（1024）条。
- `Hub::start`：规则不合法 → `InvalidInput`。`Hub::set_policy`：不合法 → `INVALID_INPUT`，**之前的规则继续生效**，错误记入
  `PolicyStatus.last_error`；成功时命中计数清零，并按工具 / 资源列表变化通知（`ToolsChanged` / `ResourcesChanged`、MCP `list_changed`）。
- 状态：`Hub::policy()` / `HubStatus.policy`：`PolicyStatus { rules: [PolicyRule + hits], loaded_at_ms, last_error? }`。`hits` = 该规则拒绝或按不存在处理的
  调用 / 唤醒 / 资源读取次数（列表过滤不计）。
- 厂商回调：`ApprovalHandler`（3.3）保留，是调用执行点上的回调形态（在规则之后、转发之前）；不新增其他回调。

**常驻 Host**：规则文件 `<home>/policy.json`（`PolicyConfig` JSON）。

- 启动（`serve` / `stdio`）时加载；文件不存在 = 无规则；文件不合法时**拒绝启动**（不在规则失效的情况下静默放行）。
- `app-mcp-host policy reload`：把文件原文交给运行中的 Host（`POST /policy`，经本地 IPC 或 TCP + 令牌）；不合法时 Host 保留之前的规则，
  命令退出码 1；Host 未运行时退出码 3。`policy validate [FILE]` 只校验；`policy show [--json]` 显示文件与生效规则、命中次数、最近的重载错误；
  `policy hide <app> [--tool T] [--id ID]`、`policy deny <app> [--tool T] [--wake] [--id ID]`、`policy remove <id>` 编辑文件（临时文件 + rename）
  并在 Host 运行时立即重载。按注解匹配等完整写法直接编辑文件后 `reload`。
- `doctor`「策略规则」检查：生效规则与命中次数；规则文件不合法、最近一次重载失败为错误；文件与生效规则不一致（未重载）为注意。
- Host 不支持调用外部程序做判断（避免执行任意进程）；需要时由厂商嵌入 Hub 并用 `ApprovalHandler`。

**绑定**：hub-c 配置 JSON `policy` 与 `am_hub_set_policy(hub, policy_json)`；hub-node 配置 `policy` 与 `setPolicy`；uniffi `HubConfig.policy`、
`Hub.set_policy`；各语言封装同名（`Policy` / `SetPolicy`、`policy` / `set_policy`）。生效规则与命中次数在各绑定的状态（`HubStatus.policy`）中。

### 3.14 页面目录、渐进披露与导航（第 4c 项）

协议部分（`ToolInfo.surface` / `page`、`app/navigate`、`capabilities.navigate`、`NAVIGATION_FAILED` / `NAVIGATION_DENIED`）的唯一
定义见 spec/protocol.md 3.4，清单 `pages` 见 spec/manifest.md 2.3。Hub 只提供机制：页面目录、披露、导航与等待；是否允许导航由
App 决定（拒绝即 `NAVIGATION_DENIED`），Hub 不经 `ApprovalHandler` 另加确认（工具本身的审批照常，3.3）。
实现：`crates/hub/src/pages.rs`（目录，纯数据）、`crates/hub/src/navigate.rs`（导航与等待）。

**页面目录**（每个 App 一份）：清单 `pages`（含 `pages[].tools`，以及顶层 `tools` 中带 `page` 的工具）∪ SDK 上报过的带 `page` 的工具
（工具因切页注销后仍保留；运行时定义覆盖清单中的同名定义；同一工具只属于一个页面）。运行时上报部分有上限：每个 App 最多
`MAX_LEARNED_PAGES`（64）个页面、每页 `MAX_LEARNED_PAGE_TOOLS`（128）个工具，超出的忽略并记 warn 日志。目录随 App 记录保留
（App 既无清单也无实例时清除）。页面"当前"= 有已连接实例注册了该页面的某个工具。

**渐进披露**：

| 层 | 入口 | 内容 |
|---|---|---|
| L0 | `apps.list` | 各 App 新增 `pageCount`（Agent 可见的页面数），各实例新增 `navigation`（是否声明了导航能力） |
| L1 | MCP `tools/list`、`Hub::tools`、`export_tools` | 与之前相同（含 3.7 的渐进暴露），另：`surface: view` 的工具只列**首选实例**（按路由优先级：焦点 / 最近活跃）当前注册的，休眠快照中的 `view` 工具不列；页面目录中未注册的工具**不列**。有 Agent 可见的页面目录时内置工具多一个 `apps.page` |
| L2 | `apps.tools(appId)` | 结果新增 `pages: [{name, title?, description?, navigable, current, toolCount}]` |
| L3 | `apps.page({appId, page})`（只读，任何时候可调用） | `{appId, page: {name, title?, description?, route?, params?, navigable, current}, tools: HubTool[], message}`；未注册的工具 `availability` 为 `notRegistered`。appId 未知 / 被隐藏 → `TOOL_NOT_FOUND`；页面不存在 → `TOOL_NOT_FOUND` |

**调用不在当前页面的工具**（App 工具调用路径，3.2）：名称解析 → 策略 `call` 执行点（3.13；按目录中声明的注解匹配，`hide` 的工具
与不存在相同、不导航）→ 资源保护（3.11，被限流的调用不导航）→ 已有实例注册该工具则照常派发；否则休眠快照中有则先唤醒（3.5）；
仍未注册而目录中有：

1. 页面 `navigable: false` → `NAVIGATION_DENIED`（`not-navigable`），不发请求。
2. 按目录定义校验参数（`INVALID_INPUT`）并审批（3.3，未审批过时）。
3. App 没有已连接实例：按唤醒规则唤醒（选定 / 最近活跃的休眠实例，否则按清单冷启动；`wake` 执行点、唤醒速率上限、`waker: none`
   与资源读取触发的唤醒相同）。唤醒后实例已注册该工具则直接派发。
4. 选导航目标：已就绪、声明了 `capabilities.navigate`、未冻结的实例，按路由优先级（调用方指定的 `instanceId` 严格、唤醒的实例、
   会话选定的实例优先）。没有 → `NAVIGATION_FAILED`（`unsupported`）。
5. 发 `app/navigate {page}`（自动导航不带 `params`；需要页面参数时由 Agent 用 `apps.navigate`，3.15），等待回复；回复后等待该实例注册目标工具（`tools/sync` / `tools/changed`）。
   回复与等待合计受 `HubConfig::navigate_timeout`（默认 5 秒，独立于 `wake_timeout`）约束（超时分别为 `timeout` / `tool-not-registered`），调用取消（3.12）随时结束等待；
   期间该连接记为有进行中的工作（`app/sleep` 被拒绝）。旧 SDK 回 `-32601` 按 `unsupported`；App 回 `NAVIGATION_*` 与
   `USER_ACTION_REQUIRED`（实例在后台、不能自行回到前台，spec/protocol.md 3.4）原样；其他错误归为 `NAVIGATION_FAILED`（`error`）。
   错误另带 `appId`、`page`。
6. 路由到该实例并派发（不再审批）。

**后台替代**（`ToolInfo.backgroundTool`，声明与选择指引见 spec/protocol.md 3.4「后台与前台」）：调用的工具没有实例注册、页面目录中
有它且它是 `view` 工具、声明了 `backgroundTool`，并且替代**可用**——同一 App 中已知（已连接实例注册的、休眠快照或清单中的）、
`surface` 为 `app`、参数符合其 inputSchema（未启用 schema-validation 时不校验）——时：

- 名称解析与策略 `call` 执行点先按被调用的 view 工具执行（`hide` / `deny` 照常，不改调）。
- **已知在后台**：将被导航的实例（第 4 步的选法）不可见，或没有可导航的已连接实例（App 休眠、未运行、不支持导航）→ 不导航，直接改调。
- 否则照常走上面的导航；导航以 `USER_ACTION_REQUIRED`（`reason: "foreground"`）失败（SDK 发现实例不可见，或 App 的导航回调自行如此回复）
  → 再改调（被调用工具的资源保护已计过一次）。
- 改调 = 对替代工具执行一次 App 工具调用：策略 `call` 执行点、资源保护（3.11）、唤醒、审批（3.3，按替代工具的定义）都按替代工具执行，
  之后与直接调用它相同（不再改调）。
- 结果标出改调：Hub API `CallOutcome.routed_to`（`routedTo`，实际调用的工具全名，未改调时不出现）；MCP 结果 `_meta` 的
  `dev.appwire/routedTo`（同值，成功与失败结果都有）。各 Hub 绑定按 JSON 透传 `CallOutcome` 的（hub-c、hub-node、`@app-mcp/hub`）带
  `routedTo`；hub-uniffi `CallOutcome.routed_to`（Kotlin / Swift 封装 `CallResult.routedTo`、Python `CallResult.routed_to`），C# `CallOutcome.RoutedTo`。
- 替代不可用（不存在、不是 app 工具、指向自身、参数不符）时不改调、记 warn 日志，按上面的导航规则处理（在后台时即得到
  `USER_ACTION_REQUIRED` / `foreground`）。实现：`crates/hub/src/navigate.rs`（判定）、`crates/hub/src/call.rs`（改调）。

**绑定**：内置工具与结果形状对所有入口一致（MCP 出口、Hub API、各格式导出与分派、hub-c / hub-uniffi / hub-node），各绑定无需新增
接口；导出名按全部内置工具（含 `apps.tools`、`apps.page`、3.15 的内置工具）计算，展开前后稳定。`HubTool` 带 `surface` / `page`（3.1）：
hub-c / hub-node / `@app-mcp/hub` 为 JSON 字段 `surface` / `page`，hub-uniffi `HubTool.surface: ToolSurface?` / `page: String?`，
C# `HubToolInfo.Surface` / `Page`（字符串 `"app"` / `"view"`，常量在 `HubToolSurface`），Kotlin / Swift / Python 同名属性
（枚举类型：Kotlin / Python `ToolSurface`，Swift `HubToolSurface`）。

### 3.15 Agent 显式控制：导航、激活 / 释放、截止时间与幂等键（第 4c 项决定、第 4f 项 a / c / j）

都是机制：何时导航、预热、释放，截止时间多长，幂等键怎么取，由 Agent 决定；Hub 只执行并保证策略挂点（3.13）、资源保护（3.11）、
唤醒速率上限（3.5）照常生效。实现：`crates/hub/src/agent_control.rs`（内置工具）、`crates/hub/src/request_meta.rs`（请求 `_meta`）。

**名称表**（本节为唯一定义；代码中只在 `crates/hub/src/names.rs` 定义一次，`app_mcp_hub::names`）。`_meta` 键用反向域名前缀 `dev.appwire/`（MCP 规范 SHOULD；docs/plans/12-mcp-stateless.md 第 4 节）。
**弃用期**：旧前缀 `app-mcp/` 的请求键（`dev.appwire/timeoutMs`、`dev.appwire/idempotencyKey`）在下一个小版本之前仍接受；新旧键同时出现且值不同 →
`INVALID_INPUT`，值相同按新键处理。结果 `_meta` 只写新键。

| 名称 | 方向 | 含义 |
|---|---|---|
| `apps.list` / `apps.select` / `apps.overview` / `apps.tools` / `apps.page` | 内置工具 | 3.7、3.14 与第 7 节 |
| `apps.navigate` | 内置工具 | 显式导航（下文） |
| `apps.activate` | 内置工具 | 只唤醒不调用（下文） |
| `apps.release` | 内置工具 | 收回本会话在该 App 上的租约（下文） |
| `apps.task.begin` / `apps.task.end` | 内置工具（只对无会话请求列出） | 签发 / 结束任务句柄（3.6「任务句柄」） |
| `taskId` | 内置工具参数（`apps.list` / `select` / `navigate` / `activate` / `release` 可选，`apps.task.end` 必填） | 任务句柄（3.6「任务句柄」） |
| `dev.appwire/status`、`dev.appwire/stateResource` | 结果 `_meta` | 结果状态（spec/protocol.md 3.2，3.2） |
| `dev.appwire/routedTo` | 结果 `_meta` | 改调后台替代时实际调用的工具全名（3.14） |
| `dev.appwire/callId` | 结果 `_meta`（每个工具调用结果） | 本次调用的 callId：即转交 App 的 `tools/invoke` 参数 `callId`（App handler 所见，如原生 `CallHandle::call_id()`）与 Hub 日志「转发工具调用」记录的 `call_id` 字段（同一记录带该 App 连接的 `cid`，spec/protocol.md 10.3）；Hub API 为 `CallOutcome.call_id` |
| `dev.appwire/instanceId` | 结果 `_meta`（路由到 App 实例时） | 实际处理调用的实例；Hub API 为 `CallOutcome.instance_id` |
| `dev.appwire/durationMs` | 结果 `_meta`（每个工具调用结果） | Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App）；Hub API 为 `CallOutcome.duration_ms` |
| `dev.appwire/woke` | 结果 `_meta`（只在 App 工具结果中） | 本次调用是否经历了唤醒：调用时目标未连接（休眠实例、按清单冷启动、页面工具所在 App 未运行），唤醒回连后才送达；并发调用合并到同一次唤醒时各自为 `true`；Hub API 为 `CallOutcome.woke`（内置 / 上游工具恒为 `false`，`apps.activate` / `apps.navigate` 的结果自带 `woke`） |
| `dev.appwire/timeoutMs` | 请求 `_meta`（`tools/call`） | Agent 的截止时间（下文） |
| `dev.appwire/idempotencyKey` | 请求 `_meta`（`tools/call`） | Agent 的幂等键（下文） |
| `dev.appwire/taskId` | 请求 `_meta`（`tools/call`，可选） | 任务句柄，与参数 `taskId` 等价、对任何工具调用生效（3.6「任务句柄」）；无旧前缀键 |

**调用元信息**（第 19 项 R4）：`callId`、`durationMs` 在每个工具调用结果（含错误结果、内置与上游工具）的 `_meta` 中；`instanceId`、
`woke` 见上表。只增字段：各 Hub 绑定按 JSON 透传 `CallOutcome` 的（hub-c v14、hub-node、`@app-mcp/hub`）带 `durationMs`、`woke`；
hub-uniffi `CallOutcome.duration_ms` / `woke`（缺省 0 / false）。上游结果原有的 `_meta` 键保留，Hub 的键覆盖同名键。

**`apps.navigate {appId, page, params?}`**（有 Agent 可见的页面目录时列出，任何时候可调用；注解 `readOnlyHint: false`、
`idempotentHint: true`，风险 write）：承载显式导航参数；调用不在当前页面的工具时的自动导航（3.14）仍不带参数。

1. appId 未知 / 被 `hide` 整体隐藏（上游 MCP 服务器同样）→ `TOOL_NOT_FOUND`；`page` 不在 Agent 可见的页面目录中 → `TOOL_NOT_FOUND`；
   页面 `navigable: false` → `NAVIGATION_DENIED`（`not-navigable`），不发请求。
2. `params`（对象；缺省按 `{}` 校验、请求中不带）按页面的 `params` schema（清单 `pages[].params`）校验 → `INVALID_INPUT`（未启用
   `schema-validation` 时不校验；页面没有 schema 时不校验）。
3. 策略 `call` 执行点：只有 App 级规则（不带 `tool` / `annotations`）匹配 → `POLICY_DENIED`。资源保护：限流按（App, `apps.navigate`）
   与 App 两级计数（3.11），参数大小同调用参数。
4. App 没有已连接实例：按唤醒规则唤醒（与 3.14 第 3 步相同，策略 `wake` 执行点只匹配 App 级规则）。
5. 选导航目标（3.14 第 4 步；会话选定 / 刚唤醒的实例优先），发 `app/navigate {page, params?}`，等回复（`navigate_timeout`）；
   错误与 3.14 第 5 步相同（`unsupported` / `error` / `timeout`、`NAVIGATION_DENIED`、`USER_ACTION_REQUIRED`），不等待工具注册。
6. 结果 `{appId, instanceId, page, ok: true, woke, message}`。不经 `ApprovalHandler`（是否允许由 App 决定，3.14）。

**`apps.activate {appId}`**（总是列出；`readOnlyHint: false`、`idempotentHint: true`，风险 write）：只唤醒不调用，供 Agent 在即将连续使用
某 App 时预热（取代第 16 项 O5 的预测预热）。已有已连接实例 → 不唤醒；否则按唤醒规则唤醒（策略 `wake` 执行点只匹配 App 级规则、
唤醒速率上限、`waker: none` → `APP_DISCONNECTED`，与资源读取触发的唤醒相同）。随后向首选实例发一次本会话的租约（与调用完成后相同，
3.5），实例在租约内不休眠。结果 `{appId, instanceId, state: "connected", woke, message}`；appId 未知 / 隐藏 → `TOOL_NOT_FOUND`。

**`apps.release {appId}`**（总是列出；同上注解）：收回本会话在该 App 各已连接实例上的全部租约——`app/lease {ttlMs: 0}`，其他会话对同一
实例的未到期租约随后补发（与会话结束时的收回相同）。不断开、不要求休眠：之后何时休眠由 App 的生命周期设置决定；再次调用其工具时照常
唤醒 / 续租。结果 `{appId, released, message}`（`released` = 发出收回的实例数，没有租约时为 0、不发消息）。Hub API 的会话为
`CallRequest.session`。

**截止时间 `dev.appwire/timeoutMs`**（MCP `tools/call` 请求 `_meta`）：正整数毫秒，含义为"从 Hub 收到请求起还愿意等待多久"。用相对时长
而不是绝对时刻：不依赖 Agent 与 Hub 的时钟一致，与协议 `ToolsInvokeParams.timeoutMs`、Hub API `CallRequest.timeout` 同一语义。
Hub 以 min(该值, `response_timeout`) 作为本次调用等待 App 结果的上限（等同于 Hub API 的 `CallRequest.timeout`；SDK 侧 `timeoutMs`
相应为 min(它, `invoke_timeout`)），超时 → `TIMEOUT`。审批、唤醒与导航的等待仍按各自的配置（`ApprovalPolicy.timeout`、`wake_timeout`、
`navigate_timeout`）。上游 MCP 服务器的调用同样适用。

**幂等键 `dev.appwire/idempotencyKey`**（MCP `tools/call` 请求 `_meta`；Hub API `CallRequest.idempotency_key`）：1..=256 个字符的字符串，
原样进入 `ToolsInvokeParams.idempotencyKey`（App 侧语义与 SDK 去重见 spec/protocol.md 3.3）。改调后台替代（3.14）时随调用转交。
不转发给上游 MCP 服务器。

**不合法的值**（`timeoutMs` 不是正整数；幂等键不是字符串、为空或超过 256 个字符）：调用不执行，以工具错误 `INVALID_INPUT` 结束
（MCP 结果 `isError: true`），不静默忽略。没有这两个键时行为与之前完全相同。
`Hub::dispatch`（第 5 节，含 `Mcp` 格式的完整 JSON-RPC 请求）不读这两个键：厂商自有循环用 `CallRequest.timeout` /
`CallRequest.idempotency_key`。

**绑定**：内置工具对所有入口一致，无需新增接口。`CallRequest.idempotency_key`：hub-c / hub-node / `@app-mcp/hub` 请求 JSON
`idempotencyKey`，hub-uniffi `CallRequest.idempotency_key`，C# `CallRequest.IdempotencyKey`，Kotlin / Swift / Python 的调用方法可选参数（`idempotencyKey` /
`idempotency_key`）。`HubConfig.navigate_timeout`：hub-c / hub-node JSON `navigateTimeoutMs`、`@app-mcp/hub` `navigateTimeoutMs`、
hub-uniffi `HubConfig.navigate_timeout_ms`、C# `HubOptions.NavigateTimeout`。

### 3.16 按名寻址：名字服务连接器（第 4d 项，spec/naming.md）

行为契约以 spec/naming.md 为准，本节只定义 Hub API。实现状态（2026-10-02）：Linux D-Bus 全链路；Android（宿主实现的连接器，未经真机验证）；
Windows / macOS 待 4d E / F。

```rust
pub struct HubConfig {
    // ...
    /// 名字服务连接器；默认空（不按名寻址）。
    pub connectors: Vec<Arc<dyn Connector>>,
    /// 按名拨入的通道在最后一次调用后保持的时间（spec/naming.md 7.2 graceMs），默认 DEFAULT_CHANNEL_GRACE = 15 s。
    pub channel_grace: Duration,
}

#[async_trait]
pub trait Connector: Send + Sync + Debug + 'static {
    fn kind(&self) -> &'static str;                                         // 来源名，如 "dbus"
    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError>; // 一次性枚举，只读、不激活
    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError>; // 名字出现 / 消失
    async fn dial(&self, address: &Address, timeout: Duration) -> Result<DialedChannel, ConnectorError>;
    fn manifest(&self, app_id: &str) -> Option<Manifest> { None }          // 安装元数据中的静态清单（2026-10-02 新增，默认无）
}
pub struct DiscoveredName { pub address: Address, pub activatable: bool, pub running: bool, pub detail: String }
pub enum NameEvent {
    Appeared(Address), Vanished(Address),        // 名字出现 / 消失（运行状态）
    Installed(DiscoveredName), Removed(Address), // 安装 / 更新、卸载（包变更，2026-10-02 新增）
}
pub struct DialedChannel { pub stream: Box<dyn ChannelIo>, pub pid: Option<u32> }
pub struct ConnectorError { pub code: &'static str, pub message: String }   // code ∈ spec/naming.md 第 12 节
```

- **宿主实现的连接器**（Unix，2026-10-02，Android 段）：发现与拨号由宿主语言完成（Android：`PackageManager` + `bindService`），
  Rust 一侧只做适配。

  ```rust
  pub trait HostNameService: Send + Sync + 'static {                       // 方法在 Hub 的阻塞线程上调用，可阻塞
      fn discover(&self) -> Result<Vec<HostedName>, ConnectorError>;       // 一次性枚举，只读安装元数据，不启动进程
      fn dial(&self, address: &Address, timeout: Duration) -> Result<HostedChannel, ConnectorError>;
      fn release(&self, lease: u64);                                       // 每个成功拨号的租约恰好一次（Android：unbindService）
  }
  pub struct HostedName { pub name: DiscoveredName, pub manifest: Option<Manifest> }  // 清单 appId 与名字不符时忽略清单
  pub struct HostedChannel { pub fd: OwnedFd, pub lease: u64, pub peer_uid: Option<u32> }
  impl HostedConnector {
      pub fn new(kind: &'static str, service: Arc<dyn HostNameService>) -> Self;
      pub fn installed(&self, app: HostedName);                            // 宿主推送：安装 / 更新 → NameEvent::Installed
      pub fn removed(&self, app_id: &str);                                 // 宿主推送：卸载 → NameEvent::Removed
  }
  pub fn naming_code(code: &str) -> &'static str;                          // 宿主回传的码 → codes 常量，未知为 ACTIVATION_DENIED
  ```

  拨号得到的 fd 包成通道：`peer_uid` 给出时按对端凭据（`SO_PEERCRED`）核对，不符 → `PEER_IDENTITY_MISMATCH`；通道被丢弃（宽限到期关闭、
  对端 EOF、核对失败）时调用一次 `release`。宿主超过 `timeout` + 2 s 未返回 → `ACTIVATION_TIMEOUT`；超时或 Hub 放弃这次拨号后宿主才交回的
  通道由回调线程立即释放（不留绑定）。安装事件按启动扫描同样登记发现记录与清单（未运行的 App 按清单列出工具，不拨号）；卸载事件移除发现记录
  与来自连接器的清单。
- **hub-uniffi**（Kotlin / Swift / Python）：外部实现的回调接口与 Hub 构造：

  ```rust
  #[uniffi::export(foreign)]
  pub trait HubNameService: Send + Sync {
      fn discover(&self) -> Vec<NamedApp>;
      fn dial(&self, app_id: String, timeout_ms: u64) -> DialOutcome;
      fn release(&self, lease: u64);
  }
  pub struct NamedApp { app_id, activatable /* 默认 true */, running /* false */, detail /* "" */, manifest_json: Option<String> }
  pub enum DialOutcome {
      Channel { fd: i32, lease: u64, peer_uid: Option<u32> },
      Failed { code: String, message: String },
      Blocked { package_name: String, app_label: String, message: String },  // 2026-10-02 新增
  }
  impl AppMcpHub {
      #[uniffi::constructor] pub fn start_with_name_service(config: HubConfig, kind: String, service: Arc<dyn HubNameService>)
          -> Result<Arc<Self>, HubError>;                                  // kind "android"（其他记为 "host"）；非 Unix → Unsupported
      pub fn name_service_installed(&self, app: NamedApp);
      pub fn name_service_removed(&self, app_id: String);
      pub async fn serve_mcp_fd(&self, fd: i32) -> Result<(), HubError>;  // fd 上的 MCP，见下
  }
  // HubConfig 新增 channel_grace_ms: Option<u64>（默认 15 s）
  ```

  `DialOutcome::Channel.fd` 的所有权交给 Hub（Kotlin `ParcelFileDescriptor.detachFd()`）；`Failed.code` 为 spec/naming.md 第 12 节的码。
  `Blocked`：目标已安装、组件存在，但系统拒绝绑定（`ACTIVATION_BLOCKED`）——Hub 侧为 `ConnectorError::blocked(BlockedTarget { package_name,
  app_name }, message)`，调用以 `USER_ACTION_REQUIRED`（`reason: "os-permission"`，spec/protocol.md 第 4 节）结束；`message` 只进日志。
  回调抛出的异常按失败处理（发现为空、拨号 `ACTIVATION_DENIED`）。Kotlin 封装：`Hub.startWithNameService`、`Hub.nameServiceInstalled` /
  `nameServiceRemoved`、`Hub.serveMcpFd`；Android 实现 `dev.appmcp.hub.android.AndroidNameService`（spec/naming.md 4.2）。
- **fd 上的 MCP**（TASKS 4g d 独立 Hub App）：`Hub::serve_mcp_stream<S: AsyncRead + AsyncWrite>(stream)`（feature `mcp-server`）在任意双向字节流上
  提供 MCP，帧与 `serve_stdio` 相同（每行一条 JSON-RPC），每次调用是一个独立会话，对端关闭后返回。hub-uniffi `serve_mcp_fd(fd)` 接管一个 Unix
  套接字 fd 调用它；本构建不含 `mcp-server`（Android 默认精简库）时为 `Unsupported`。
- **平台实现**：`connector::DbusConnector::new(address: Option<String>)`（Linux，cargo feature `dbus`，默认开启；`None` = 当前用户的
  会话总线）。另有 `DbusConnector::reload_config()`（写 / 删激活文件后调用，spec/naming.md 4.1）。`Address` 为
  `app_mcp_protocol::naming::Address`。
- **发现**：`Hub::start` 为每个连接器起一个任务——先订阅 `watch`，再 `discover` 一次，之后只随事件更新发现记录；没有定时器、不轮询、
  不为发现启动进程。事件流结束（总线断开）后任务结束，不重试。名字消失只把记录的运行状态改为否，不删除记录（spec/naming.md 5.4）。
- **路由**（每次调用 / 资源读取 / `apps.activate`，spec/naming.md 9.2）：已有活连接 → 按名拨号（发现记录可激活或正在运行）→
  唤醒描述（`HubConfig::waker`）→ `APP_DISCONNECTED`。按名拨号不需要唤醒器（`waker: None` 时也可用）；与唤醒共用去重、等待
  （`wake_timeout`，同时是 `dial` 的超时）与速率上限（`wake_rate_limit`）。同一 App 只拨默认名字（`appmcp://<appId>`；按实例地址拨号未实现）。
- **认领**：在 Hub 自己拨出的通道上 `app/ready` 的实例认领该 App 经拨号发起的全部等待中的唤醒，不需要令牌。
- **关闭**：通道上不做无消息断开（SDK 声明 `heartbeatMs: 0`）；没有进行中调用与待派唤醒时，于
  `max(最后一条消息 + channel_grace, 该连接上各会话租约的最晚到期)` 关闭通道（忙时每隔一个宽限复查）。通道关闭或对端 EOF 后实例转为
  休眠快照（`HubEvent::AppDormant`；快照的恢复令牌不下发，下次握手按完整同步），Hub 不再持有该 App 的任何连接、fd 或定时器
  （7.1 不变式，`crates/hub/tests/naming.rs` 断言 fd / 线程数回到基线）。
- **错误**：拨号失败 → `LAUNCH_FAILED`，系统确认未安装（`NAME_NOT_FOUND`：没有所有者且没有激活文件、激活文件指向的程序不存在）→
  `APP_NOT_INSTALLED` 并移除发现记录；两者 `details.code` 为名字服务错误码，同时记入该 App 的 `last_error`。
- **`apps.list`**：每个 App 增加 `nameService`（无发现记录时为 `null`）：`{source, name, activatable, running, firstSeenAt, lastSeenAt}`。
  只有发现记录、没有清单与快照的 App 也会列出（`connected: false`），可用 `apps.activate` 拨号后获得工具。发现记录变化时发
  `tools/list_changed`。`AppInfo`（Hub API）暂无对应字段。
- **未做**（spec/naming.md 第 14 节）：App 登记文件（5.3）的读取与目录监视、签名指纹 / 名字所有者与登记程序的核对（10.3，当前只核对
  通道对端 uid）、`maxBoundApps` LRU（7.3）、内存压力关闭（7.4）、`app/hold`（7.2）、hub-c / hub-node 的连接器回调与 `channel_grace`。

`app-mcp-host`：`serve --name-service`（配置 `lifecycle.nameService: true`）在 Linux 上加入 `DbusConnector::new(None)`，
`--channel-grace-ms` / `lifecycle.channelGraceMs` 设置宽限；默认关闭。`app-mcp-host app install --app-id <id> --exec <程序>
[--manifest app-mcp.json] [--name …]` 写 D-Bus 激活文件与 App 登记文件（spec/naming.md 4.1、5.3，`source: "manual"`）并调用
`ReloadConfig`，清单复制到 `<home>/manifests/<appId>.json`（Host 启动时加载）；`app uninstall --app-id <id>` 删除这些文件。

## 4. 进程内 App（可选，M2）

`Hub::attach_local(hello) -> LocalAppChannel`：厂商自带的系统 App 与 Hub 同进程时，
不走 WebSocket，直接交换协议消息（`app_mcp_protocol::Message`）。本期只预留签名，可不实现。

## 5. 工具格式导出与分派

厂商自有 LLM 循环时不需要 MCP：

| `ToolFormat` | `export_tools` 输出 | `dispatch` 输入 → 输出 |
|---|---|---|
| `Mcp` | `[{name, title, description, inputSchema, annotations, outputSchema?}]`（`annotations` = `HubTool.annotations`；`outputSchema` 同 MCP 出口） | `{name, arguments}` → MCP `CallToolResult` JSON |
| `OpenAiChat` | `[{type:"function", function:{name, description, parameters}}]` | `{id, type:"function", function:{name, arguments:"<json 字符串>"}}` → `{role:"tool", tool_call_id, content}` |
| `OpenAiResponses` | `[{type:"function", name, description, parameters}]` | `{type:"function_call", call_id, name, arguments}` → `{type:"function_call_output", call_id, output}` |
| `Anthropic` | `[{name, description, input_schema}]` | `{type:"tool_use", id, name, input}` → `{type:"tool_result", tool_use_id, content, is_error?}` |
| `Gemini` | `{functionDeclarations:[{name, description, parameters}]}` | `{name, args}`（functionCall）→ `{functionResponse:{name, response}}` |

**名称编码**：OpenAI / Anthropic 名称只允许 `[a-zA-Z0-9_-]{1,64}`，不能含 `.`。
导出名 = 全名把 `.` 换成 `__`；若冲突或超长，截断并追加 `_<4 位十六进制哈希>`。
Hub 保存双向映射，`dispatch` 同时接受导出名与全名。`Mcp`、`Gemini` 也使用同一导出名以保持一致（Gemini 实测不接受部分字符时不致出错）。

**Schema 适配**：OpenAI strict 模式不在本期；导出原样 JSON Schema，只去掉 `$schema`。
Gemini 不支持的关键字（`additionalProperties`、`$ref` 等）在 `Gemini` 格式中剔除。

**结果内容**：成功 → JSON 文本（与 MCP 出口一致，含 `stateHints`；首次接触附带总览文本）；
失败 → `is_error` / 错误文本 `<KIND>: <message>`（如 `USER_REJECTED: …`），与 MCP 出口一致。

**描述增强**：导出时在描述末尾追加 `（风险：payment）` 之类标记，仅当 risk 不是 read/write。

## 6. C ABI 要点（`bindings/hub-c/include/app_mcp_hub.h`）

- 前缀 `am_hub_`；`AM_HUB_API_VERSION 3`（v3：`am_hub_listen_addr`、配置 `listen`）；与 `app_mcp.h` 相同的字符串所有权规则（回调中字符串由接收方 `am_string_free`）。
- 所有复杂结构以 JSON 字符串传递（`am_hub_tools_json(hub, format, filter_json)`、`am_hub_call(hub, request_json, cb, user_data)`），避免 ABI 膨胀。
- 事件：`am_hub_set_event_cb(hub, cb(event_json, user_data))`。
- 回调在 Hub 的分发线程上执行，不在调用方线程。

## 7. 测试要求

- Hub 单元测试 + 迁移后的 Host 全部集成测试保持通过（MCP 出口行为不变）。
- 每种 `ToolFormat` 的导出与 `dispatch` 往返测试（对 fake App 实例）。
- 审批：拒绝 → `USER_REJECTED`；超时 → 拒绝；低于阈值不询问。
- 策略（3.13）：`hide` 的 App / 工具不在任何列表中、调用按不存在；`deny` → `POLICY_DENIED`（只附规则 id）；不合法的规则不生效且保留旧规则；无规则时行为不变。
- 会话维度的总览首次附带：两个 `session` 各附带一次。
- 绑定：各语言用 `app-mcp-native` 的 App 端 SDK（或 `fake_app`）连上嵌入式 Hub，完成列工具 + 调用 + 事件 + 审批。
