//! 按名寻址（spec/naming.md 4.2、spec/hub-api.md 3.16）：由宿主语言实现的名字服务（Android：Kotlin 的
//! `PackageManager` 发现 + `bindService` 拨号），适配为 `app_mcp_hub::connector::HostedConnector`。

#[cfg(unix)]
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use app_mcp_hub as hub;

/// 宿主发现的一个 App（只读安装元数据得到，发现时不启动其进程）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct NamedApp {
    /// 清单中的 appId（不由包名推导）。
    pub app_id: String,
    /// 可被系统激活（Android：导出了 `dev.appmcp.TOOLS` Service）。
    #[uniffi(default = true)]
    pub activatable: bool,
    /// 当前正在运行（未知时为 `false`）。
    #[uniffi(default = false)]
    pub running: bool,
    /// 平台名字（日志 / `apps.list` 的 `nameService.name`；Android：`<包名>/<Service 类名>`）。
    #[uniffi(default = "")]
    pub detail: String,
    /// 安装元数据中的静态清单 JSON（Android：`<meta-data android:name="dev.appmcp.manifest">` 指向的资源）；
    /// 其 `appId` 必须等于 `app_id`，否则忽略该清单。
    #[uniffi(default = None)]
    pub manifest_json: Option<String>,
}

/// 一次拨号的结果。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DialOutcome {
    /// 得到通道：`fd` 为 socketpair 的一端，所有权交给 Hub（Kotlin `ParcelFileDescriptor.detachFd()`）；
    /// `lease` 标识这次拨号持有的系统资源（Hub 关闭通道后以它回调 `release`）；`peer_uid` 为目标包的 uid（核对通道对端）。
    Channel { fd: i32, lease: u64, peer_uid: Option<u32> },
    /// 失败：`code` 为 spec/naming.md 第 12 节的错误码（如 `ACTIVATION_DENIED`、`HUB_NOT_TRUSTED`、`NAME_NOT_FOUND`），
    /// 未知码按 `ACTIVATION_DENIED` 处理。失败时宿主自行释放已占用的资源，不会再收到 `release`。
    Failed { code: String, message: String },
    /// 目标已安装、组件存在，但系统拒绝绑定（Android `bindService` 返回 false 或 `SecurityException`：关联启动 / 自启动管控）。
    /// Hub 以 `USER_ACTION_REQUIRED`（`reason: "os-permission"`）结束调用，面向用户的消息只用 `app_label`；
    /// `message` 为内部说明，只进日志。宿主自行释放已占用的资源，不会再收到 `release`。
    Blocked { package_name: String, app_label: String, message: String },
}

/// 由宿主语言实现的名字服务。各方法在 Hub 的阻塞线程上调用，可以阻塞（`dial` 不超过 `timeout_ms`）。
#[uniffi::export(foreign)]
pub trait HubNameService: Send + Sync {
    /// 一次性枚举（Hub 启动时）：只读安装元数据，不得启动任何 App 进程。
    fn discover(&self) -> Vec<NamedApp>;
    /// 拨号 `appmcp://<app_id>`：绑定目标 App（未运行时由系统激活）并换得通道。
    fn dial(&self, app_id: String, timeout_ms: u64) -> DialOutcome;
    /// 释放一次成功拨号持有的资源（Android：`unbindService`）。每个租约恰好调用一次。
    fn release(&self, lease: u64);
}

/// 宿主名字服务 → Hub 的 [`hub::connector::HostNameService`]。
#[cfg(unix)]
pub(crate) struct NameServiceAdapter(pub(crate) Arc<dyn HubNameService>);

/// 发现记录的来源名：已知平台原样使用，其余为 `"host"`（来源名是 `&'static str`）。
#[cfg(unix)]
pub(crate) fn source_kind(kind: &str) -> &'static str {
    match kind {
        "android" => "android",
        _ => "host",
    }
}

#[cfg(unix)]
pub(crate) fn hosted_name(app: NamedApp) -> Option<hub::connector::HostedName> {
    let address = match app_mcp_protocol::naming::Address::new(&app.app_id, None) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(app_id = app.app_id, "名字服务给出的 appId 不合法，忽略：{e}");
            return None;
        }
    };
    let manifest = app.manifest_json.as_deref().and_then(|text| match app_mcp_manifest::load_str(text) {
        Ok(loaded) => Some(loaded.manifest),
        Err(e) => {
            tracing::warn!(app_id = app.app_id, "安装元数据中的清单不合法，忽略：{e}");
            None
        }
    });
    Some(hub::connector::HostedName {
        name: hub::DiscoveredName { address, activatable: app.activatable, running: app.running, detail: app.detail },
        manifest,
    })
}

#[cfg(unix)]
impl hub::connector::HostNameService for NameServiceAdapter {
    fn discover(&self) -> Result<Vec<hub::connector::HostedName>, hub::ConnectorError> {
        let service = self.0.clone();
        let apps = catch_unwind(AssertUnwindSafe(|| service.discover())).map_err(|_| {
            hub::ConnectorError::new(app_mcp_protocol::naming::codes::NAME_NOT_FOUND, "名字服务的发现回调抛出异常")
        })?;
        Ok(apps.into_iter().filter_map(hosted_name).collect())
    }

    fn dial(
        &self,
        address: &app_mcp_protocol::naming::Address,
        timeout: std::time::Duration,
    ) -> Result<hub::connector::HostedChannel, hub::ConnectorError> {
        use app_mcp_protocol::naming::codes;
        use std::os::fd::{FromRawFd, OwnedFd};
        let service = self.0.clone();
        let (app_id, timeout_ms) = (address.app_id.clone(), u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
        let outcome = catch_unwind(AssertUnwindSafe(|| service.dial(app_id, timeout_ms)))
            .map_err(|_| hub::ConnectorError::new(codes::ACTIVATION_DENIED, format!("拨号 {address} 的回调抛出异常")))?;
        match outcome {
            DialOutcome::Channel { fd, lease, peer_uid } if fd >= 0 => {
                // @security 宿主按契约转移一个打开的 fd 的所有权（`detachFd()`），之后不再使用它；从这里起由 Hub 唯一持有。
                let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                Ok(hub::connector::HostedChannel { fd, lease, peer_uid })
            }
            DialOutcome::Channel { fd, lease, .. } => {
                self.release(lease);
                Err(hub::ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 的通道 fd 无效：{fd}")))
            }
            DialOutcome::Failed { code, message } => {
                Err(hub::ConnectorError::new(hub::connector::naming_code(&code), message))
            }
            DialOutcome::Blocked { package_name, app_label, message } => Err(hub::ConnectorError::blocked(
                hub::BlockedTarget { package_name, app_name: app_label },
                message,
            )),
        }
    }

    fn release(&self, lease: u64) {
        let service = self.0.clone();
        if catch_unwind(AssertUnwindSafe(|| service.release(lease))).is_err() {
            tracing::warn!(lease, "名字服务的释放回调抛出异常");
        }
    }
}
