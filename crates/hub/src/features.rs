//! 可选能力（cargo features，见 Cargo.toml 的 `[features]`）与配置的一致性检查。
//!
//! @invariant 关闭某个 feature 时公开配置字段与方法保持不变（绑定层无需条件编译）；
//! 请求被关闭的能力时返回 `ErrorKind::Unsupported`，说明缺少哪个 feature，而不是静默忽略。

use std::io;

use crate::hub::HubConfig;

/// MCP 出口（`/mcp`、`serve_stdio`、`McpSession`）是否编译在内。
pub const MCP_SERVER: bool = cfg!(feature = "mcp-server");
/// 上游聚合（子进程 MCP 服务器）是否编译在内。
pub const UPSTREAM: bool = cfg!(feature = "upstream");
/// 调用前按 inputSchema 校验参数是否编译在内。
pub const SCHEMA_VALIDATION: bool = cfg!(feature = "schema-validation");

fn unsupported(what: &str, feature: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("本构建未包含{what}（app-mcp-hub 的 cargo feature `{feature}` 未开启），请改用完整构建或关闭该配置"),
    )
}

/// `Hub::serve_http(_with)` 等总是提供 `/mcp` 的入口在缺少 MCP 出口时报错。
pub(crate) fn require_mcp_server(what: &str) -> io::Result<()> {
    if MCP_SERVER {
        return Ok(());
    }
    Err(unsupported(what, "mcp-server"))
}

/// `Hub::start` 前检查配置是否用到了本构建未包含的能力。
/// @error `Unsupported`：`mcp_http = true` 但无 `mcp-server`；`upstreams` 非空但无 `upstream`。
pub(crate) fn check_config(config: &HubConfig) -> io::Result<()> {
    if config.mcp_http {
        require_mcp_server("MCP 出口（HubConfig::mcp_http）")?;
    }
    if !config.upstreams.is_empty() && !UPSTREAM {
        return Err(unsupported("上游聚合（HubConfig::upstreams）", "upstream"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UpstreamConfig;

    fn upstream() -> UpstreamConfig {
        UpstreamConfig {
            command: "true".into(),
            args: Vec::new(),
            env: Default::default(),
        }
    }

    #[test]
    fn default_config_needs_no_optional_feature() {
        check_config(&HubConfig::default()).unwrap();
    }

    #[test]
    fn mcp_http_requires_mcp_server() {
        let c = HubConfig {
            mcp_http: true,
            ..HubConfig::default()
        };
        let r = check_config(&c);
        assert_eq!(r.is_ok(), MCP_SERVER);
        if let Err(e) = r {
            assert_eq!(e.kind(), io::ErrorKind::Unsupported);
            assert!(e.to_string().contains("mcp-server"), "{e}");
        }
        assert_eq!(require_mcp_server("x").is_ok(), MCP_SERVER);
    }

    #[test]
    fn upstreams_require_upstream() {
        let mut c = HubConfig::default();
        c.upstreams.insert("files".into(), upstream());
        let r = check_config(&c);
        assert_eq!(r.is_ok(), UPSTREAM);
        if let Err(e) = r {
            assert_eq!(e.kind(), io::ErrorKind::Unsupported);
            assert!(e.to_string().contains("`upstream`"), "{e}");
        }
    }
}
