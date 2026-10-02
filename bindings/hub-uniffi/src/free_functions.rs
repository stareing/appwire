//! 顶层导出函数：格式名解析、本原生库能力组合、日志初始化。

use app_mcp_hub as hub;

use crate::types::{HubError, ToolFormat};

/// 解析格式名：`mcp`、`openai-chat`（或 `openai`）、`openai-responses`、`anthropic`、`gemini`，
/// 不区分大小写，`-` / `_` 可省略。
#[uniffi::export]
pub fn parse_tool_format(name: String) -> Result<ToolFormat, HubError> {
    name.parse::<hub::ToolFormat>()
        .map(Into::into)
        .map_err(|detail| HubError::InvalidConfig { detail })
}

/// 本原生库编译进的可选能力（cargo features，spec/hub-api.md 3.10）。各组合的 uniffi 接口相同，宿主据此在启动前检查，
/// 而不是等到用到时才得到 `Unsupported`（如独立 Hub App 需要 `mcp_server`）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct HubFeatures {
    /// MCP 出口（`serve_http`、`mcp_http`、`serve_mcp_fd`），cargo feature `mcp-server`。
    pub mcp_server: bool,
    /// 上游聚合（`upstreams`），cargo feature `upstream`。
    pub upstream: bool,
    /// 调用前按 inputSchema 校验参数，cargo feature `schema-validation`。
    pub schema_validation: bool,
}

/// 本原生库的能力组合（[`HubFeatures`]）。
#[uniffi::export]
pub fn hub_features() -> HubFeatures {
    HubFeatures {
        mcp_server: hub::features::MCP_SERVER,
        upstream: hub::features::UPSTREAM,
        schema_validation: hub::features::SCHEMA_VALIDATION,
    }
}

/// 把 Hub 日志（tracing）输出到 stderr。`filter` 同 `RUST_LOG` 语法，为空时读 `RUST_LOG`，
/// 再为空时为 `info`。只有第一次调用生效；返回是否本次完成了初始化。
#[uniffi::export]
pub fn init_logging(filter: Option<String>) -> bool {
    use tracing_subscriber::EnvFilter;
    let filter = match filter {
        Some(f) => EnvFilter::try_new(f).unwrap_or_else(|_| EnvFilter::new("info")),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok()
}
