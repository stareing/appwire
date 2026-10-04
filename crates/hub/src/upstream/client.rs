//! 上游 MCP 服务器的子进程客户端：启动、初始化、列表变化转发与按退避重启（feature `upstream`）。

use std::sync::{Arc, Weak};
use std::time::Duration;

use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::NotificationContext;
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

use super::ui::{declares_mcp_apps, mcp_apps_extension};
use super::{UpstreamConfig, UpstreamHello};
use crate::hub::HubShared;

const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const STABLE_AFTER: Duration = Duration::from_secs(30);
const INIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 作为上游客户端的回调：列表变化时刷新缓存并转发 list_changed。
struct UpstreamClient {
    name: String,
    shared: Weak<HubShared>,
}

impl ClientHandler for UpstreamClient {
    async fn on_tool_list_changed(&self, context: NotificationContext<RoleClient>) {
        if let Some(shared) = self.shared.upgrade() {
            match context.peer.list_all_tools().await {
                Ok(tools) => shared.set_upstream_tools(&self.name, tools),
                Err(e) => tracing::warn!(upstream = %self.name, "刷新上游工具列表失败：{e}"),
            }
        }
    }

    async fn on_resource_list_changed(&self, context: NotificationContext<RoleClient>) {
        if let Some(shared) = self.shared.upgrade() {
            match context.peer.list_all_resources().await {
                Ok(resources) => shared.set_upstream_resources(&self.name, resources),
                Err(e) => tracing::warn!(upstream = %self.name, "刷新上游资源列表失败：{e}"),
            }
        }
    }

    fn get_info(&self) -> ClientConfig {
        // 声明 MCP Apps 扩展：上游据此在工具中给出界面资源，Hub 原样透传给 Agent（spec/hub-api.md 3.22）
        ClientConfig::new(
            ClientCapabilities::builder().enable_extensions_with(mcp_apps_extension()).build(),
            Implementation::new("app-mcp-host", env!("CARGO_PKG_VERSION")),
        )
    }
}

/// 启动一个上游并在退出后按退避重启，直到任务被中止。
pub(crate) async fn run(shared: Arc<HubShared>, name: String, config: UpstreamConfig) {
    let mut failures: u32 = 0;
    loop {
        let started = tokio::time::Instant::now();
        match connect(&shared, &name, &config).await {
            Ok(service) => {
                let peer = service.peer().clone();
                let info = service.peer_info();
                let caps = info
                    .as_ref()
                    .map(|i| i.capabilities.clone())
                    .unwrap_or_default();
                let tools = if caps.tools.is_some() {
                    peer.list_all_tools().await.unwrap_or_else(|e| {
                        tracing::warn!(upstream = %name, "获取上游工具列表失败：{e}");
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                let resources = if caps.resources.is_some() {
                    peer.list_all_resources().await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                let instructions = info.as_ref().and_then(|i| i.instructions.clone());
                let server_name = info
                    .as_ref()
                    .and_then(|i| i.server_info.as_ref())
                    .map(|s| s.title.clone().unwrap_or_else(|| s.name.clone()));
                tracing::info!(upstream = %name, tools = tools.len(), resources = resources.len(), "上游 MCP 服务器已连接");
                let mcp_apps = declares_mcp_apps(caps.extensions.as_ref());
                let hello = UpstreamHello { tools, resources, instructions, server_name, mcp_apps };
                shared.upstream_connected(&name, peer, hello);
                match service.waiting().await {
                    Ok(reason) => {
                        tracing::warn!(upstream = %name, ?reason, "上游 MCP 服务器已断开")
                    }
                    Err(e) => tracing::warn!(upstream = %name, "上游 MCP 服务器任务异常：{e}"),
                }
                shared.upstream_disconnected(&name, None);
            }
            Err(e) => {
                tracing::error!(upstream = %name, "启动上游 MCP 服务器失败：{e}");
                shared.upstream_disconnected(&name, Some(e));
            }
        }
        if started.elapsed() >= STABLE_AFTER {
            failures = 0;
        }
        let delay = INITIAL_BACKOFF
            .saturating_mul(1u32 << failures.min(10))
            .min(MAX_BACKOFF);
        failures = failures.saturating_add(1);
        tokio::time::sleep(delay).await;
    }
}

async fn connect(
    shared: &Arc<HubShared>,
    name: &str,
    config: &UpstreamConfig,
) -> Result<rmcp::service::RunningService<RoleClient, UpstreamClient>, String> {
    let mut cmd = tokio::process::Command::new(&config.command);
    cmd.args(&config.args).envs(&config.env).kill_on_drop(true);
    // 常驻 Host 没有控制台：上游（多为 npx / python 等控制台程序）不弹出窗口。
    #[cfg(windows)]
    cmd.creation_flags(crate::CREATE_NO_WINDOW);
    // 子进程的 stderr 继承 Host 的 stderr；stdout 专用于与 Host 的 MCP 通信。
    let transport =
        TokioChildProcess::new(cmd).map_err(|e| format!("无法启动 {}：{e}", config.command))?;
    let client = UpstreamClient {
        name: name.to_owned(),
        shared: Arc::downgrade(shared),
    };
    match tokio::time::timeout(INIT_TIMEOUT, client.serve(transport)).await {
        Ok(Ok(service)) => Ok(service),
        Ok(Err(e)) => Err(format!("MCP 初始化失败：{e}")),
        Err(_) => Err("MCP 初始化超时".into()),
    }
}
