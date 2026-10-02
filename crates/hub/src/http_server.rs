//! Hub 的 HTTP 服务（spec/protocol.md 1.3、spec/hub-api.md 3.6）：同一个监听器上按路径分流。
//!
//! | 路径 | 内容 | 校验 |
//! |---|---|---|
//! | `/app` | WebSocket 升级 → App 连接（[`crate::app_server`]） | `Origin` 在 `app/hello` 时按允许列表 / 配对处理 |
//! | `/mcp` | MCP Streamable HTTP（rmcp [`StreamableHttpService`]，开启时） | `Origin` 允许列表（403）→ 令牌（401） |
//! | `/healthz` | `GET`：Host 身份与监听信息（[`Health`]），不需要令牌 | `Origin` 允许列表（403） |
//! | `/status` | `GET`：运行状态（[`crate::HubStatus`]：实例、最近错误、SDK 诊断上报） | `Origin` 允许列表（403）→ IPC 直接允许；TCP 必须携带有效令牌（未配置令牌时 403） |
//! | `/policy` | `POST`：替换策略规则（请求体为 [`crate::PolicyConfig`] JSON，spec/hub-api.md 3.13）；不合法时 400、保留之前的规则 | 同 `/status` |
//!
//! - 本地 IPC 端点上的 `/mcp`（[`crate::HubConfig::mcp_http`] 开启时）与 TCP 上相同，但**不校验令牌**：
//!   IPC 的对端用户已由操作系统核对为同一用户（[`crate::ipc`]），同一用户本来就能读取令牌文件，令牌没有额外防护。
//!   供厂商 Agent / 支持本地套接字的 MCP 客户端使用（客户端应同样核对监听方是同一用户，spec/protocol.md 1.4）。
//! - 根路径 `/` 的 WebSocket 升级按 `/app` 处理（合并端口之前的 SDK 连接根路径；兼容期内保留，首次出现时记录提示）。
//! - 同一个 [`Router`] 既服务回环 TCP 监听器，也服务本地 IPC 端点（[`Transport`]），两者只在连接级鉴权与
//!   对端信息上不同。
//! - 每个 MCP HTTP 会话（`Mcp-Session-Id`）对应一个独立的 [`McpSession`]。默认只接受回环 `Host` 头
//!   （rmcp 默认行为）；`allow_remote` 时不校验 `Host`。
//! - 本地访问令牌（[`HttpOptions::token`]）：带 `Origin` 头的请求（浏览器）必须携带
//!   `Authorization: Bearer <令牌>`；不带 `Origin` 的本地客户端是否必须携带由
//!   [`HttpOptions::require_token_without_origin`] 决定。只作用于 `/mcp`。

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use app_mcp_protocol::identity::HostIdentity;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
#[cfg(feature = "mcp-server")]
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
#[cfg(feature = "mcp-server")]
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use crate::app_server::Peer;
use crate::hub::HubShared;
#[cfg(feature = "mcp-server")]
use crate::mcp::McpSession;

/// `/mcp` 的服务实现（rmcp Streamable HTTP）。
#[cfg(feature = "mcp-server")]
type McpService = StreamableHttpService<McpSession, LocalSessionManager>;
/// 本构建未包含 MCP 出口（feature `mcp-server`）：不可构造，`/mcp` 总是 404。
#[cfg(not(feature = "mcp-server"))]
type McpService = Infallible;

/// 创建 `/mcp` 服务；缺少 MCP 出口时为 `None`（调用方已由 `features` 检查拒绝开启 MCP 的配置）。
#[cfg(feature = "mcp-server")]
fn mcp_service(shared: &Arc<HubShared>, options: &HttpOptions) -> Option<McpService> {
    let mut config = StreamableHttpServerConfig::default();
    if options.allow_remote {
        config = config.disable_allowed_hosts();
    }
    let factory_shared = shared.clone();
    Some(StreamableHttpService::new(
        move || Ok(McpSession::new(factory_shared.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    ))
}

#[cfg(not(feature = "mcp-server"))]
fn mcp_service(_shared: &Arc<HubShared>, _options: &HttpOptions) -> Option<McpService> {
    None
}

pub use app_mcp_protocol::{APP_PATH, HEALTH_PATH, MCP_PATH, STATUS_PATH};

/// 替换策略规则（`POST`，spec/hub-api.md 3.13）；授权同 `/status`。
pub const POLICY_PATH: &str = "/policy";

/// `/healthz` 响应中的服务标识（[`app_mcp_protocol::identity::SERVICE_NAME`]）。
pub const HEALTH_SERVICE: &str = app_mcp_protocol::identity::SERVICE_NAME;

/// HTTP 服务选项（[`crate::HubConfig::http`]，以及 [`crate::Hub::serve_http_with`] 的额外监听器）。
#[derive(Clone, Debug, Default)]
pub struct HttpOptions {
    /// 允许绑定非回环地址并接受任意 `Host` 头（有安全风险）。
    pub allow_remote: bool,
    /// 本地访问令牌（只作用于 `/mcp`）；`None` = 不校验令牌（只做 Origin / Host 校验）。
    pub token: Option<String>,
    /// 设置了令牌时，不带 `Origin` 头的请求（非浏览器本地客户端）是否也必须携带令牌。
    /// 带 `Origin` 头的请求始终必须携带。
    pub require_token_without_origin: bool,
}

/// `GET /healthz` 的响应体（也是 `app-mcp-host status` / 单实例探测读取的内容）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// Host 身份：`service`（固定 `"app-mcp"`）、`version`、`user`、`pid`。
    #[serde(flatten)]
    pub identity: HostIdentity,
    /// 主 HTTP 服务（`/app`、`/mcp`、`/healthz`）实际监听的地址；未开启时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// 本地 IPC 端点；未开启时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_endpoint: Option<String>,
    /// App 连接（WebSocket 升级）路径，固定 `/app`。
    pub app_path: String,
    /// MCP 端点路径；本监听器未开启 MCP 时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_path: Option<String>,
    /// `/mcp` 是否配置了访问令牌（浏览器来源必须携带）。
    pub token_required_for_browsers: bool,
}

impl Health {
    /// 服务标识是否为 app-mcp。
    pub fn is_app_mcp(&self) -> bool {
        self.identity.is_app_mcp()
    }
}

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;

fn plain(status: http::StatusCode, text: &str) -> http::Response<Body> {
    let mut resp = http::Response::new(Full::new(Bytes::from(text.to_owned())).boxed());
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp
}

fn json(status: http::StatusCode, value: &impl Serialize) -> http::Response<Body> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let mut resp = http::Response::new(Full::new(Bytes::from(body)).boxed());
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    resp
}

/// `POST /policy` 请求体的字节上限（B-07）。
pub const MAX_POLICY_BODY: usize = 1024 * 1024;

/// `POST /policy` 的响应体。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyReply {
    pub ok: bool,
    /// 成功时生效的规则条数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<usize>,
    /// 失败原因（之前的规则继续生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PolicyReply {
    fn error(message: String) -> Self {
        Self { ok: false, rules: None, error: Some(message) }
    }
}

/// 取 `Authorization: Bearer <令牌>` 中的令牌；空令牌视为未携带。
fn bearer(headers: &http::HeaderMap) -> Option<&str> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// 常量时间比较（避免按字节提前返回泄露令牌前缀）。
fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 令牌校验结果：`None` = 通过；`Some(原因)` = 拒绝。
fn check_token(
    options: &HttpOptions,
    has_origin: bool,
    headers: &http::HeaderMap,
) -> Option<&'static str> {
    let expected = options.token.as_deref()?;
    match bearer(headers) {
        Some(got) if token_eq(got, expected) => None,
        Some(_) => Some("访问令牌不正确。"),
        None if has_origin || options.require_token_without_origin => Some(
            "需要本地访问令牌：请求头 Authorization: Bearer <令牌>（令牌见 ~/.app-mcp/token）。",
        ),
        None => None,
    }
}

fn unauthorized(reason: &str) -> http::Response<Body> {
    let mut resp = plain(http::StatusCode::UNAUTHORIZED, reason);
    resp.headers_mut().insert(
        http::header::WWW_AUTHENTICATE,
        http::HeaderValue::from_static("Bearer realm=\"app-mcp\""),
    );
    resp
}


/// 连接所在的传输。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Transport {
    /// 回环 TCP（[`crate::HubConfig::listen`] 或额外的 HTTP 监听器）。
    Tcp,
    /// 本地 IPC（Unix 域套接字 / 命名管道），对端已确认是同一用户。
    Ipc,
}

/// 一个 HTTP 服务实例：路由、选项与（可选的）MCP 服务。
pub(crate) struct Router {
    shared: Arc<HubShared>,
    transport: Transport,
    options: HttpOptions,
    mcp: Option<McpService>,
    health: Health,
    /// 是否已提示过根路径兼容（只提示一次，之后记 debug 日志）。
    root_path_hinted: AtomicBool,
}

impl Router {
    /// `mcp` 为 `false` 时 `/mcp` 返回 404。`health` 的 `mcp_path` / `token_required_for_browsers`
    /// 按本实例的设置填写。
    pub fn new(
        shared: Arc<HubShared>,
        transport: Transport,
        options: HttpOptions,
        mcp: bool,
        mut health: Health,
    ) -> Arc<Self> {
        let mcp = if mcp { mcp_service(&shared, &options) } else { None };
        health.mcp_path = mcp.is_some().then(|| MCP_PATH.to_owned());
        health.token_required_for_browsers = mcp.is_some() && options.token.is_some();
        Arc::new(Self {
            shared,
            transport,
            options,
            mcp,
            health,
            root_path_hinted: AtomicBool::new(false),
        })
    }

    async fn handle(self: Arc<Self>, mut req: http::Request<Incoming>, peer: Peer) -> http::Response<Body> {
        let path = req.uri().path().to_owned();
        let origin = req
            .headers()
            .get(http::header::ORIGIN)
            .map(|v| v.to_str().unwrap_or("<invalid>").to_owned());

        // App 连接：来源校验在 app/hello 时进行（可能交给 PairingHandler），这里不做 403。
        if is_websocket_upgrade(&req) && (path == APP_PATH || path == "/") {
            if path == "/" {
                self.hint_root_path(peer);
            }
            return self.upgrade_app(&mut req, origin, peer);
        }

        if !self.shared.origins.allows(origin.as_deref()) {
            tracing::warn!(%peer, ?origin, %path, "拒绝来源不在允许列表中的 HTTP 请求");
            return plain(
                http::StatusCode::FORBIDDEN,
                "Origin 不在允许列表中；可用 --allow-origin 添加。",
            );
        }
        match path.as_str() {
            HEALTH_PATH => {
                if req.method() == http::Method::GET {
                    json(http::StatusCode::OK, &self.health)
                } else {
                    plain(http::StatusCode::METHOD_NOT_ALLOWED, "只支持 GET")
                }
            }
            STATUS_PATH => {
                if req.method() != http::Method::GET {
                    return plain(http::StatusCode::METHOD_NOT_ALLOWED, "只支持 GET");
                }
                if let Some(resp) = self.authorize_status(req.headers(), peer) {
                    return resp;
                }
                json(http::StatusCode::OK, &self.shared.status())
            }
            POLICY_PATH => {
                if req.method() != http::Method::POST {
                    return plain(http::StatusCode::METHOD_NOT_ALLOWED, "只支持 POST");
                }
                if let Some(resp) = self.authorize_status(req.headers(), peer) {
                    return resp;
                }
                self.replace_policy(req).await
            }
            MCP_PATH => {
                let Some(service) = &self.mcp else {
                    return plain(http::StatusCode::NOT_FOUND, "本端点未开启 MCP HTTP 服务。");
                };
                if let Some(reason) = check_token(&self.options, origin.is_some(), req.headers()) {
                    tracing::warn!(%peer, ?origin, "拒绝未携带有效令牌的 HTTP 请求");
                    return unauthorized(reason);
                }
                #[cfg(feature = "mcp-server")]
                return service.handle(req).await;
                #[cfg(not(feature = "mcp-server"))]
                match *service {}
            }
            APP_PATH => plain(
                http::StatusCode::UPGRADE_REQUIRED,
                "/app 是 App 连接端点，需要 WebSocket 升级（ws://<地址>/app）。",
            ),
            _ => plain(
                http::StatusCode::NOT_FOUND,
                "app-mcp：App 连接 /app（WebSocket），MCP /mcp，健康检查 /healthz，运行状态 /status。",
            ),
        }
    }

    /// `POST /policy`：读取请求体（上限 [`MAX_POLICY_BODY`]）并替换规则；不合法时 400，之前的规则继续生效。
    async fn replace_policy(&self, req: http::Request<Incoming>) -> http::Response<Body> {
        let body = http_body_util::Limited::new(req.into_body(), MAX_POLICY_BODY);
        let bytes = match body.collect().await {
            Ok(b) => b.to_bytes(),
            Err(e) => {
                return json(http::StatusCode::PAYLOAD_TOO_LARGE, &PolicyReply::error(format!("读取请求体失败（上限 {MAX_POLICY_BODY} 字节）：{e}")));
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let result = crate::PolicyConfig::from_json(&text).and_then(|config| {
            let n = config.rules.len();
            self.shared.set_policy(config).map(|()| n)
        });
        match result {
            Ok(n) => json(http::StatusCode::OK, &PolicyReply { ok: true, rules: Some(n), error: None }),
            Err(e) => {
                // from_json 失败时也记下错误（set_policy 内部校验失败已记录）。
                self.shared.record_policy_error(&e);
                json(http::StatusCode::BAD_REQUEST, &PolicyReply::error(e))
            }
        }
    }

    /// `/status` 的授权：IPC 对端直接允许；TCP 必须携带有效令牌（未配置令牌时一律拒绝）。`None` = 通过。
    fn authorize_status(&self, headers: &http::HeaderMap, peer: Peer) -> Option<http::Response<Body>> {
        if self.transport == Transport::Ipc {
            return None;
        }
        let Some(expected) = self.options.token.as_deref() else {
            return Some(plain(
                http::StatusCode::FORBIDDEN,
                "请经本地 IPC 访问 /status，或配置令牌（--auth browser|all）后携带 Authorization: Bearer <令牌>。",
            ));
        };
        match bearer(headers) {
            Some(got) if token_eq(got, expected) => None,
            _ => {
                tracing::warn!(%peer, "拒绝未携带有效令牌的 /status 请求");
                Some(unauthorized("/status 需要本地访问令牌：请求头 Authorization: Bearer <令牌>（令牌见 ~/.app-mcp/token）。"))
            }
        }
    }

    fn hint_root_path(&self, peer: Peer) {
        if self.root_path_hinted.swap(true, Ordering::Relaxed) {
            tracing::debug!(%peer, "App 以根路径 / 连接（兼容）");
        } else {
            tracing::warn!(
                %peer,
                "App 以根路径 / 连接：这是合并端口之前的 SDK 地址，兼容期内仍接受；请升级 SDK 或把端点改为 ws://<地址>/app"
            );
        }
    }

    /// 完成 WebSocket 握手（101），升级后的连接交给 App 连接服务。
    fn upgrade_app(&self, req: &mut http::Request<Incoming>, origin: Option<String>, peer: Peer) -> http::Response<Body> {
        let headers = req.headers();
        let version_ok = headers
            .get(http::header::SEC_WEBSOCKET_VERSION)
            .is_some_and(|v| v.as_bytes() == b"13");
        let Some(key) = headers.get(http::header::SEC_WEBSOCKET_KEY) else {
            return plain(http::StatusCode::BAD_REQUEST, "缺少 Sec-WebSocket-Key");
        };
        if req.method() != http::Method::GET || !version_ok {
            let mut resp = plain(http::StatusCode::BAD_REQUEST, "只支持 WebSocket 版本 13 的 GET 升级请求");
            resp.headers_mut().insert(
                http::header::SEC_WEBSOCKET_VERSION,
                http::HeaderValue::from_static("13"),
            );
            return resp;
        }
        let accept = derive_accept_key(key.as_bytes());
        let on_upgrade = hyper::upgrade::on(req);
        let shared = self.shared.clone();
        // 升级后的 App 连接比这条 HTTP 连接活得久：另计一次，直到 WebSocket 结束。
        let guard = self.shared.activity.hold();
        tokio::spawn(async move {
            let _guard = guard;
            match on_upgrade.await {
                Ok(upgraded) => {
                    let ws = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, Some(app_ws_config())).await;
                    crate::app_server::handle_websocket(shared, ws, origin, peer).await;
                }
                Err(e) => tracing::debug!(%peer, "WebSocket 升级失败：{e}"),
            }
        });
        let mut resp = http::Response::new(Full::new(Bytes::new()).boxed());
        *resp.status_mut() = http::StatusCode::SWITCHING_PROTOCOLS;
        let h = resp.headers_mut();
        h.insert(http::header::UPGRADE, http::HeaderValue::from_static("websocket"));
        h.insert(http::header::CONNECTION, http::HeaderValue::from_static("Upgrade"));
        if let Ok(v) = http::HeaderValue::from_str(&accept) {
            h.insert(http::header::SEC_WEBSOCKET_ACCEPT, v);
        }
        resp
    }

    /// 在一条连接（TCP 或 IPC）上提供 HTTP/1.1 服务（支持升级），直到连接结束。
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, peer: Peer)
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let router = self;
        let handler = service_fn(move |req: http::Request<Incoming>| {
            let router = router.clone();
            async move { Ok::<_, Infallible>(router.handle(req, peer).await) }
        });
        if let Err(e) = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(io), handler)
            .with_upgrades()
            .await
        {
            tracing::debug!(%peer, "HTTP 连接结束：{e}");
        }
    }

    /// 回环 TCP 监听器的接受循环，直到任务被中止。
    ///
    /// 每次 accept 前取得接受闸门的许可（[`crate::activity`]）：按需启动的 Host 空闲退出时闸门关闭，
    /// 正在等待的 accept 被取消（连接留在内核队列，由服务管理器再次启动 Host 接受）。
    pub async fn serve_tcp(self: Arc<Self>, listener: TcpListener) {
        debug_assert_eq!(self.transport, Transport::Tcp);
        let activity = self.shared.activity.clone();
        loop {
            let permit = activity.permit().await;
            let accepted = tokio::select! {
                r = listener.accept() => r,
                () = activity.draining() => continue,
            };
            match accepted {
                Ok((stream, addr)) => {
                    let guard = permit.admit();
                    disable_nagle(&stream, addr);
                    tokio::spawn(self.clone().serve_counted(stream, Peer::Tcp(addr), guard));
                }
                Err(e) => {
                    drop(permit);
                    tracing::warn!("接受 TCP 连接失败：{e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// 本地 IPC 端点的接受循环，直到任务被中止（中止时监听器被丢弃，Unix 上删除套接字文件）。
    /// 接受闸门同 [`Router::serve_tcp`]。
    pub async fn serve_ipc(self: Arc<Self>, mut listener: crate::ipc::IpcListener) {
        debug_assert_eq!(self.transport, Transport::Ipc);
        let activity = self.shared.activity.clone();
        loop {
            let permit = activity.permit().await;
            let accepted = tokio::select! {
                r = listener.accept() => r,
                () = activity.draining() => continue,
            };
            match accepted {
                Ok(a) => {
                    let guard = permit.admit();
                    tokio::spawn(self.clone().serve_counted(a.stream, Peer::Ipc { pid: a.pid }, guard));
                }
                Err(e) => {
                    drop(permit);
                    tracing::warn!("接受 IPC 连接失败：{e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// [`Router::serve_connection`]，连接结束时才交回计数。
    async fn serve_counted<IO>(self: Arc<Self>, io: IO, peer: Peer, _guard: crate::activity::ConnectionGuard)
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        self.serve_connection(io, peer).await;
    }
}

/// App 连接（TCP 与本地 IPC 共用）的 WebSocket 读缓冲初始容量。
///
/// @why tungstenite 缺省 128 KiB 且每条连接预先分配（第 4f 项 d 测得每条 App 连接约 135 KB）；App 消息多为数百字节，
/// 大消息按帧头中的长度临时扩容（不受此值限制，上限仍是消息 / 帧大小上限），所以取 8 KiB。
pub(crate) const APP_WS_READ_BUFFER: usize = 8 * 1024;

pub(crate) fn app_ws_config() -> WebSocketConfig {
    WebSocketConfig::default().read_buffer_size(APP_WS_READ_BUFFER)
}

/// 关闭 Nagle 算法。
///
/// @why 请求 / 响应式的小消息（JSON-RPC、SSE 分块）在 Nagle + 对端延迟确认下每次多等约 40 ms
/// （第 4f 项 d 测得每次小调用 46–53 ms，设置后 0.7 ms）。失败只影响延迟，记 debug 日志后继续。
fn disable_nagle(stream: &tokio::net::TcpStream, addr: std::net::SocketAddr) {
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!(%addr, "设置 TCP_NODELAY 失败：{e}");
    }
}

/// 是否为 WebSocket 升级请求（`Connection: upgrade` + `Upgrade: websocket`，大小写不敏感）。
fn is_websocket_upgrade<B>(req: &http::Request<B>) -> bool {
    let has_token = |name: http::header::HeaderName, token: &str| {
        req.headers().get_all(name).iter().any(|v| {
            v.to_str()
                .is_ok_and(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
        })
    };
    has_token(http::header::CONNECTION, "upgrade") && has_token(http::header::UPGRADE, "websocket")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepted_tcp_disables_nagle() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (stream, peer) = listener.accept().await.unwrap();
        assert!(!stream.nodelay().unwrap(), "前提：新接受的套接字缺省开着 Nagle");
        disable_nagle(&stream, peer);
        assert!(stream.nodelay().unwrap());
    }

    #[test]
    fn app_ws_buffer_is_small_but_message_limits_unchanged() {
        let c = app_ws_config();
        let d = WebSocketConfig::default();
        assert_eq!(c.read_buffer_size, APP_WS_READ_BUFFER);
        assert!(c.read_buffer_size < d.read_buffer_size);
        assert_eq!((c.max_message_size, c.max_frame_size), (d.max_message_size, d.max_frame_size));
    }

    fn headers(auth: Option<&str>) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        if let Some(a) = auth {
            h.insert(http::header::AUTHORIZATION, a.parse().unwrap());
        }
        h
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer(&headers(Some("Bearer abc"))), Some("abc"));
        assert_eq!(bearer(&headers(Some("bearer  abc "))), Some("abc"));
        assert_eq!(bearer(&headers(Some("Bearer "))), None);
        assert_eq!(bearer(&headers(Some("Basic abc"))), None);
        assert_eq!(bearer(&headers(None)), None);
    }

    #[test]
    fn token_policy() {
        let none = HttpOptions::default();
        assert!(check_token(&none, true, &headers(None)).is_none());

        let lax = HttpOptions {
            token: Some("secret".into()),
            ..Default::default()
        };
        // 浏览器（带 Origin）必须携带
        assert!(check_token(&lax, true, &headers(None)).is_some());
        assert!(check_token(&lax, true, &headers(Some("Bearer nope"))).is_some());
        assert!(check_token(&lax, true, &headers(Some("Bearer secret"))).is_none());
        // 本地客户端（无 Origin）默认可不携带；携带了就必须正确；空令牌视为未携带
        assert!(check_token(&lax, false, &headers(None)).is_none());
        assert!(check_token(&lax, false, &headers(Some("Bearer "))).is_none());
        assert!(check_token(&lax, false, &headers(Some("Bearer nope"))).is_some());

        let strict = HttpOptions {
            require_token_without_origin: true,
            ..lax
        };
        assert!(check_token(&strict, false, &headers(None)).is_some());
        assert!(check_token(&strict, false, &headers(Some("Bearer secret"))).is_none());
    }

    #[test]
    fn constant_time_eq() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
    }
}
