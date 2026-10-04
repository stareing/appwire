//! Hub 启动与停止：单实例锁、监听绑定、后台任务、登记文件。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_protocol::identity::HostIdentity;
use app_mcp_protocol::registry::EndpointRegistry;
use tokio::net::TcpListener;

use crate::http_server::{Health, HttpOptions, Router, Transport};
use crate::instance::Instance;
use crate::upstream::UpstreamConfig;

use super::{Hub, HubConfig, HubShared, lock, unix_millis};

/// 绑定 `listen`；被占用时依次尝试 `alternates`。全部失败时返回第一个错误（带尝试过的地址）。
async fn bind_listen(listen: &str, alternates: &[String]) -> std::io::Result<TcpListener> {
    let first = match TcpListener::bind(listen).await {
        Ok(l) => return Ok(l),
        Err(e) => e,
    };
    if first.kind() != std::io::ErrorKind::AddrInUse || alternates.is_empty() {
        return Err(first);
    }
    for alt in alternates {
        match TcpListener::bind(alt).await {
            Ok(l) => {
                tracing::warn!("监听地址 {listen} 已被占用，改用备选地址 {alt}（网页 SDK 会依次尝试这些端口）");
                return Ok(l);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        format!("{first}（{listen} 及备选地址 {} 均已被占用）", alternates.join("、")),
    ))
}

impl Hub {
    /// 取得单实例锁（若配置 [`HubConfig::run_dir`]），绑定 HTTP 服务与本地 IPC（若配置），启动后台任务，
    /// 写登记文件。必须在 tokio 运行时中调用。
    ///
    /// 错误：`ResourceBusy`（单实例锁已被持有）、`AddrInUse`（地址 / IPC 端点被占用）、
    /// `PermissionDenied`（非回环地址未允许远程）、`InvalidInput`（配置不合法）。
    pub async fn start(config: HubConfig) -> std::io::Result<Hub> {
        Self::start_with(config, crate::PreboundListeners::default()).await
    }

    /// 同 [`Hub::start`]，但用服务管理器交来的监听器（按需启动，[`crate::PreboundListeners`]）代替绑定
    /// [`HubConfig::listen`] / [`HubConfig::ipc_endpoint`]；没有交来的那一个仍按配置绑定。
    pub async fn start_with(config: HubConfig, mut prebound: crate::PreboundListeners) -> std::io::Result<Hub> {
        crate::features::check_config(&config)?;
        config
            .lease
            .validate()
            .and_then(|()| config.limits.validate())
            .and_then(|()| config.policy.validate())
            .and_then(|()| config.agents.validate())
            .and_then(|()| config.result_cache.validate())
            .and_then(|()| config.undo.validate())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        // 锁先于任何监听：并发启动的两个 Host 只有一个能走到绑定。
        let instance = config.run_dir.as_deref().map(Instance::acquire).transpose()?;
        let listener = match (prebound.take_tcp()?, &config.listen) {
            (Some(l), _) => Some(l),
            (None, Some(addr)) => Some(bind_listen(addr, &config.listen_alternates).await?),
            (None, None) => None,
        };
        let listen_addr = listener.as_ref().map(TcpListener::local_addr).transpose()?;
        if let Some(local) = listen_addr
            && !local.ip().is_loopback()
            && !config.http.allow_remote
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("监听地址 {local} 不是回环地址；如确需远程访问请允许远程（--http-allow-remote）"),
            ));
        }
        let ipc = match (prebound.take_ipc()?, &config.ipc_endpoint) {
            (Some(adopted), _) => Some(adopted),
            (None, Some(text)) => {
                let endpoint = app_mcp_protocol::Endpoint::parse(text)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                if !endpoint.is_ipc() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("ipc_endpoint 必须是本地 IPC 端点（unix: / pipe:）：{text}"),
                    ));
                }
                let listener = crate::ipc::IpcListener::bind(&endpoint).await?;
                Some((endpoint.to_string(), listener))
            }
            (None, None) => None,
        };
        let waker = config
            .waker
            .build()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.0.message))?;
        let shared = Arc::new(HubShared::new(config, waker));
        // 先于任何监听读回休眠记录：App 回连时能按实例 ID 认领快照。
        shared.load_persisted();
        shared.load_inboxes();
        let ipc_endpoint = ipc.as_ref().map(|(e, _)| e.clone());
        let mut hub = Hub {
            shared: shared.clone(),
            listen_addr,
            ipc_endpoint,
            started_at_ms: unix_millis(),
            tasks: Mutex::new(vec![
                tokio::spawn(shared.clone().notify_loop()),
                tokio::spawn(shared.clone().dormant_sweep_loop()),
                tokio::spawn(shared.clone().lease_idle_loop()),
                tokio::spawn(shared.clone().persist_loop()),
            ]),
            instance: Mutex::new(instance),
        };
        let health = hub.health_base();
        // 先于任何监听器开始服务：/status 从这里读监听位置与启动时刻。
        let _ = shared.endpoints.set(hub.endpoint_registry());
        let mut tasks = Vec::new();
        if let Some(listener) = listener {
            let router = Router::new(
                shared.clone(),
                Transport::Tcp,
                shared.config.http.clone(),
                shared.config.mcp_http,
                health.clone(),
            );
            tasks.push(tokio::spawn(router.serve_tcp(listener)));
        }
        if let Some((endpoint, listener)) = ipc {
            // IPC 上与 TCP 同样的路由；对端用户已由操作系统核对，不需要令牌（spec/protocol.md 1.4）。
            let router = Router::new(
                shared.clone(),
                Transport::Ipc,
                HttpOptions::default(),
                shared.config.mcp_http,
                health,
            );
            tasks.push(tokio::spawn(router.serve_ipc(listener)));
            tracing::info!(%endpoint, "本地 IPC 连接服务已启动");
        }
        let upstreams: Vec<(String, UpstreamConfig)> = lock(&shared.upstreams)
            .iter()
            .map(|(n, s)| (n.clone(), s.config.clone()))
            .collect();
        for (name, cfg) in upstreams {
            tasks.push(tokio::spawn(crate::upstream::run(shared.clone(), name, cfg)));
        }
        // 名字服务发现（spec/naming.md 第 5 节）：每个连接器一个事件驱动的任务。
        for index in 0..shared.config.connectors.len() {
            tasks.push(tokio::spawn(shared.clone().naming_loop(index)));
        }
        lock(&hub.tasks).extend(tasks);
        match listen_addr {
            Some(addr) => tracing::info!(
                "HTTP 服务已启动：App 连接 ws://{addr}/app{}",
                if shared.config.mcp_http { format!("，MCP http://{addr}/mcp") } else { String::new() }
            ),
            None => tracing::info!("Hub 已启动（未开启 TCP 服务）"),
        }
        let registry = hub.endpoint_registry();
        if let Some(inst) = hub.instance.get_mut().unwrap_or_else(|e| e.into_inner()).as_mut() {
            inst.publish(&registry)?;
            tracing::info!(path = %inst.registry_path().display(), "已写登记文件");
        }
        Ok(hub)
    }

    /// 内部共享状态（crate 内的测试检查任务表用）。
    #[cfg(all(test, feature = "mcp-server"))]
    pub(crate) fn shared(&self) -> &Arc<HubShared> {
        &self.shared
    }

    /// 按需启动（spec/protocol.md 1.9）：等到没有连接、调用、唤醒、在线 App 与 MCP 会话并持续 `idle`，且已停止接受新连接后返回；
    /// 之后调用方应 [`Hub::shutdown`]（新连接留在服务管理器持有的监听套接字上，由它再次启动进程）。
    pub async fn wait_idle(&self, idle: Duration) {
        let shared = self.shared.clone();
        self.shared.activity.wait_idle(idle, move || shared.idle_blocker()).await;
    }

    /// HTTP 服务（`/app`、`/mcp`、`/healthz`）实际监听的地址；未开启时为 `None`。
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr
    }

    /// 本地 IPC 连接服务的端点字符串（`unix:…` / `pipe:…`，可直接作为原生 SDK 的 `host_url`）；
    /// 未开启时为 `None`。
    pub fn ipc_endpoint(&self) -> Option<&str> {
        self.ipc_endpoint.as_deref()
    }

    /// 本进程的 Host 身份（`service` / `version` / `user` / `pid`）。
    pub fn identity(&self) -> &HostIdentity {
        &self.shared.identity
    }

    /// 登记文件的内容（实际监听位置与身份；配置了 [`HubConfig::run_dir`] 时已写入 `endpoints.json`）。
    pub fn endpoint_registry(&self) -> EndpointRegistry {
        EndpointRegistry {
            identity: self.shared.identity.clone(),
            listen: self.listen_addr.map(|a| a.to_string()),
            ipc_endpoint: self.ipc_endpoint.clone(),
            started_at_ms: self.started_at_ms,
        }
    }

    /// `/healthz` 的公共部分（`mcp_path` 等由各监听器的 [`Router`] 填写）。
    pub(super) fn health_base(&self) -> Health {
        Health {
            identity: self.shared.identity.clone(),
            listen: self.listen_addr.map(|a| a.to_string()),
            ipc_endpoint: self.ipc_endpoint.clone(),
            app_path: crate::http_server::APP_PATH.to_owned(),
            mcp_path: None,
            token_required_for_browsers: false,
        }
    }

    /// 停止：中止后台任务（含上游子进程）、关闭所有 App 连接，删除登记文件并释放单实例锁。
    pub async fn shutdown(self) {
        // listen 流先收到结束信号，下面等待 App 连接关闭的间隙里发出最终结果。
        self.shared.begin_closing();
        for t in lock(&self.tasks).drain(..) {
            t.abort();
        }
        self.shared.persist_flush();
        let conns = self.shared.registry().all_connections();
        for c in &conns {
            c.close();
        }
        // 给写任务一点时间发送 Close 帧。
        tokio::time::sleep(Duration::from_millis(20)).await;
        lock(&self.instance).take();
        tracing::info!("Hub 已停止");
    }

}
