//! stdio 子命令与不带子命令的旧用法：单客户端 stdio 模式。

use std::process::ExitCode;

use anyhow::Context;
use app_mcp_hub::{Hub, HubConfig};
use app_mcp_protocol::ConnectionErrorCode;

use super::hub_config::hub_config;
use super::serve::{describe_app_endpoints, running_instance, shutdown_signal};
use crate::cli::LegacyArgs;
use crate::config::{AppHome, Settings};
use crate::{agents, intents, load_file, log_notices, logging, policy};

pub(crate) async fn run_legacy(args: LegacyArgs) -> anyhow::Result<ExitCode> {
    let home = AppHome::resolve(args.hub.home.home.as_deref())?;
    // stdio 模式只在显式 --config 时读取配置文件（与旧行为一致，不受常驻服务配置影响）。
    let file = load_file(&home, args.hub.config.as_deref(), false)?;
    let s = Settings::resolve(&file, &args.hub.overrides()?, &home)?;
    logging::init(logging::LogOptions {
        level: &s.log_level,
        file: None,
    })?;
    log_notices(&s);
    let config = HubConfig {
        policy: policy::load(&home)?,
        agents: agents::load(&home)?,
        intent_defaults: intents::load(&home)?.defaults,
        ..hub_config(&s, &home)
    };
    let hub = match Hub::start(config).await {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy => {
            let hint = running_instance(&home)
                .await
                .and_then(|r| r.mcp_url())
                .map(|url| format!("；请让 MCP 客户端直接连接 {url}"))
                .unwrap_or_default();
            anyhow::bail!("[{}] {e}{hint}", ConnectionErrorCode::LockHeld);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("启动 App 连接服务 {} 失败", describe_app_endpoints(&s)));
        }
    };
    if let Some(addr) = &args.http {
        hub.serve_http(addr, args.http_allow_remote)
            .await
            .with_context(|| format!("启动 HTTP MCP 服务 {addr} 失败"))?;
    }
    if args.http.is_none() || args.stdio {
        hub.serve_stdio().await?;
        tracing::info!("MCP 客户端已断开，退出");
    } else {
        shutdown_signal().await;
        tracing::info!("收到退出信号");
    }
    hub.shutdown().await;
    Ok(ExitCode::SUCCESS)
}
