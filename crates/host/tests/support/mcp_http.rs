//! 测试用的 MCP Streamable HTTP 客户端传输（`serve.rs`、`upstream_http.rs` 经 `#[path]` 引入）。
//!
//! @why reqwest 缺省读取 `HTTP_PROXY` 等环境变量，且不认 `NO_PROXY=127.*` 这类通配写法，回环地址会被送进代理；
//! 代理不可用时测试失败。测试只连本机 Hub，客户端一律不走代理（T-06：不依赖运行环境）。

use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

/// 与 rmcp 缺省客户端相同（不复用空闲连接、不跟随重定向），另外不走任何代理。
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client")
}

/// 连接 `uri`（如 `http://127.0.0.1:port/mcp`）的传输；`token` 为 Bearer 令牌。
pub fn transport(uri: String, token: Option<&str>) -> StreamableHttpClientTransport<reqwest::Client> {
    let mut cfg = StreamableHttpClientTransportConfig::with_uri(uri);
    if let Some(t) = token {
        cfg = cfg.auth_header(t.to_owned());
    }
    StreamableHttpClientTransport::with_client(http_client(), cfg)
}
