//! Streamable HTTP MCP 传输：用 rmcp 的 [`StreamableHttpService`]，路径 `/mcp`。
//!
//! - 每个 HTTP 会话（`Mcp-Session-Id`）对应一个独立的 [`McpSession`]。
//! - 请求的 `Origin` 按与 WebSocket 侧相同的允许列表校验（防 DNS rebinding / 跨站），不通过返回 403。
//! - 默认只接受回环 `Host` 头（rmcp 默认行为）；`allow_remote` 时不校验 `Host`。
//! - 可选本地访问令牌（[`HttpOptions::token`]）：带 `Origin` 头的请求（浏览器）必须携带
//!   `Authorization: Bearer <令牌>`；不带 `Origin` 的本地客户端是否必须携带由
//!   [`HttpOptions::require_token_without_origin`] 决定。不通过返回 401。
//! - `GET /healthz`：返回服务标识、版本与进程号（[`Health`]），供单实例探测；不需要令牌。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::hub::HubShared;
use crate::mcp::McpSession;

/// MCP 端点路径。
pub const MCP_PATH: &str = "/mcp";
/// 健康检查路径。
pub const HEALTH_PATH: &str = "/healthz";
/// `/healthz` 响应中的服务标识。
pub const HEALTH_SERVICE: &str = "app-mcp";

/// Streamable HTTP 服务选项。
#[derive(Clone, Debug, Default)]
pub struct HttpOptions {
    /// 允许绑定非回环地址并接受任意 `Host` 头（有安全风险）。
    pub allow_remote: bool,
    /// 本地访问令牌；`None` = 不校验令牌（只做 Origin / Host 校验）。
    pub token: Option<String>,
    /// 设置了令牌时，不带 `Origin` 头的请求（非浏览器本地客户端）是否也必须携带令牌。
    /// 带 `Origin` 头的请求始终必须携带。
    pub require_token_without_origin: bool,
}

/// `GET /healthz` 的响应体。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// 固定为 [`HEALTH_SERVICE`]。
    pub service: String,
    /// Hub 版本（crate 版本）。
    pub version: String,
    /// 提供服务的进程号。
    pub pid: u32,
    /// App 连接服务（WebSocket）地址；未开启时为 `None`。
    pub ws_addr: Option<String>,
    /// MCP 端点路径。
    pub mcp_path: String,
    /// 是否配置了访问令牌。
    pub token_required_for_browsers: bool,
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

pub(crate) async fn serve(
    shared: Arc<HubShared>,
    listener: TcpListener,
    options: HttpOptions,
    ws_addr: Option<SocketAddr>,
) {
    let mut config = StreamableHttpServerConfig::default();
    if options.allow_remote {
        config = config.disable_allowed_hosts();
    }
    let factory_shared = shared.clone();
    let service = StreamableHttpService::new(
        move || Ok(McpSession::new(factory_shared.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let health = Arc::new(Health {
        service: HEALTH_SERVICE.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        ws_addr: ws_addr.map(|a| a.to_string()),
        mcp_path: MCP_PATH.to_owned(),
        token_required_for_browsers: options.token.is_some(),
    });
    let options = Arc::new(options);

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("HTTP 接受连接失败：{e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let service = service.clone();
        let shared = shared.clone();
        let options = options.clone();
        let health = health.clone();
        tokio::spawn(async move {
            let handler = service_fn(move |req: http::Request<Incoming>| {
                let service = service.clone();
                let shared = shared.clone();
                let options = options.clone();
                let health = health.clone();
                async move {
                    let origin = req
                        .headers()
                        .get(http::header::ORIGIN)
                        .map(|v| v.to_str().unwrap_or("<invalid>"));
                    if !shared.origins.allows(origin) {
                        tracing::warn!(%addr, ?origin, "拒绝来源不在允许列表中的 HTTP 请求");
                        return Ok::<_, Infallible>(plain(
                            http::StatusCode::FORBIDDEN,
                            "Origin 不在允许列表中；可用 --allow-origin 添加。",
                        ));
                    }
                    let has_origin = origin.is_some();
                    match req.uri().path() {
                        HEALTH_PATH => {
                            return Ok(if req.method() == http::Method::GET {
                                json(http::StatusCode::OK, health.as_ref())
                            } else {
                                plain(http::StatusCode::METHOD_NOT_ALLOWED, "只支持 GET")
                            });
                        }
                        MCP_PATH => {}
                        _ => return Ok(plain(http::StatusCode::NOT_FOUND, "MCP 端点为 /mcp")),
                    }
                    if let Some(reason) = check_token(&options, has_origin, req.headers()) {
                        tracing::warn!(%addr, ?origin, "拒绝未携带有效令牌的 HTTP 请求");
                        return Ok(unauthorized(reason));
                    }
                    Ok(service.handle(req).await)
                }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .await
            {
                tracing::debug!(%addr, "HTTP 连接结束：{e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
