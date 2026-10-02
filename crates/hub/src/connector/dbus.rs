//! Linux：D-Bus 会话总线连接器（spec/naming.md 4.1）。
//!
//! - 发现：`ListActivatableNames`（有激活文件，未运行也可列出）+ `ListNames`（正在运行），只读，不激活。
//! - 事件：`NameOwnerChanged`（名字出现 / 消失），无轮询。
//! - 拨号：向 `dev.appmcp.App.<id>` 调用 `dev.appmcp.App1.Open()`（不带 `NO_AUTO_START`：未运行则由总线激活），
//!   取回 UNIX_FD 后核对 socketpair 对端的 uid（`SO_PEERCRED`），之后消息走 fd。

use std::collections::BTreeMap;
use std::time::Duration;

use app_mcp_protocol::naming::{Address, codes, dbus as names};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::OnceCell;
use zbus::fdo::DBusProxy;

use super::{ConnectorError, DialedChannel, DiscoveredName, NameEvent};

/// D-Bus 会话总线连接器。总线连接在首次使用时建立（Hub 自己的连接，不是对任何 App 的引用）。
pub struct DbusConnector {
    /// 总线地址；`None` = 会话总线（`DBUS_SESSION_BUS_ADDRESS`）。
    address: Option<String>,
    conn: OnceCell<zbus::Connection>,
}

impl std::fmt::Debug for DbusConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbusConnector").field("address", &self.address).finish_non_exhaustive()
    }
}

impl DbusConnector {
    /// `address` 为 D-Bus 地址（如 `unix:path=/run/user/1000/bus`）；`None` = 当前用户的会话总线。
    pub fn new(address: Option<String>) -> Self {
        Self { address: address.filter(|a| !a.is_empty()), conn: OnceCell::new() }
    }

    async fn connection(&self) -> Result<&zbus::Connection, ConnectorError> {
        self.conn
            .get_or_try_init(|| async {
                let builder = match &self.address {
                    Some(a) => zbus::connection::Builder::address(a.as_str()),
                    None => zbus::connection::Builder::session(),
                };
                builder
                    .map_err(bus_unreachable)?
                    .build()
                    .await
                    .map_err(bus_unreachable)
            })
            .await
    }

    /// 请总线重新读取激活目录（spec/naming.md 4.1：写入 / 删除激活文件后调用；dbus-broker 是否自动发现未确认，U-05）。
    pub async fn reload_config(&self) -> Result<(), ConnectorError> {
        self.driver().await?.reload_config().await.map_err(|e| bus_call_failed("ReloadConfig", e))
    }

    async fn driver(&self) -> Result<DBusProxy<'static>, ConnectorError> {
        let conn = self.connection().await?;
        DBusProxy::new(conn).await.map_err(bus_unreachable)
    }
}

fn bus_unreachable(e: zbus::Error) -> ConnectorError {
    ConnectorError::new(codes::NAME_NOT_FOUND, format!("无法连接 D-Bus 会话总线：{e}"))
}

#[async_trait::async_trait]
impl super::Connector for DbusConnector {
    fn kind(&self) -> &'static str {
        "dbus"
    }

    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError> {
        let driver = self.driver().await?;
        let activatable = driver.list_activatable_names().await.map_err(|e| bus_call_failed("ListActivatableNames", e))?;
        let running = driver.list_names().await.map_err(|e| bus_call_failed("ListNames", e))?;
        let mut found: BTreeMap<String, DiscoveredName> = BTreeMap::new();
        let mut note = |name: &str, is_activatable: bool| {
            let Some(address) = names::parse_bus_name(name) else { return };
            let entry = found.entry(name.to_owned()).or_insert_with(|| DiscoveredName {
                address,
                activatable: false,
                running: false,
                detail: name.to_owned(),
            });
            if is_activatable {
                // 只有默认名字能被激活；实例名字的激活文件不合规范，忽略其可激活性。
                entry.activatable = entry.address.instance.is_none();
            } else {
                entry.running = true;
            }
        };
        for n in &activatable {
            note(n.as_str(), true);
        }
        for n in &running {
            note(n.as_str(), false);
        }
        Ok(found.into_values().collect())
    }

    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError> {
        let driver = self.driver().await?;
        let stream = driver.receive_name_owner_changed().await.map_err(|e| bus_call_failed("NameOwnerChanged", e))?;
        Ok(stream
            .filter_map(|signal| async move {
                let args = signal.args().ok()?;
                let address = names::parse_bus_name(args.name.as_str())?;
                Some(if args.new_owner.is_some() { NameEvent::Appeared(address) } else { NameEvent::Vanished(address) })
            })
            .boxed())
    }

    async fn dial(&self, address: &Address, timeout: Duration) -> Result<DialedChannel, ConnectorError> {
        let conn = self.connection().await?;
        let name = names::bus_name(address);
        let path = names::object_path(address);
        let call = conn.call_method(Some(name.as_str()), path.as_str(), Some(names::INTERFACE), names::OPEN_METHOD, &());
        let reply = match tokio::time::timeout(timeout, call).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(e)) => return Err(open_failed(&name, e)),
            Err(_) => {
                return Err(ConnectorError::new(
                    codes::ACTIVATION_TIMEOUT,
                    format!("{} 秒内没有得到 {name} 的 Open() 回复（激活或 App 启动过慢）", timeout.as_secs_f32()),
                ));
            }
        };
        let fd: zbus::zvariant::OwnedFd = reply
            .body()
            .deserialize()
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, format!("{name} 的 Open() 回复不是 fd：{e}")))?;
        let std_stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
        let io_err = |e: std::io::Error| ConnectorError::new(codes::ACTIVATION_DENIED, format!("{name} 返回的通道不可用：{e}"));
        std_stream.set_nonblocking(true).map_err(io_err)?;
        let stream = tokio::net::UnixStream::from_std(std_stream).map_err(io_err)?;
        // @security 通道对端（创建 socketpair 的进程）必须是同一用户（spec/naming.md 4.1、10.3）。
        let cred = stream.peer_cred().map_err(io_err)?;
        let me = app_mcp_protocol::endpoint::current_uid();
        if cred.uid() != me {
            return Err(ConnectorError::new(
                codes::PEER_IDENTITY_MISMATCH,
                format!("{name} 的通道来自其他用户（uid {}，pid {:?}），已拒绝", cred.uid(), cred.pid()),
            ));
        }
        let pid = cred.pid().and_then(|p| u32::try_from(p).ok());
        Ok(DialedChannel { stream: Box::new(stream), pid })
    }
}

fn bus_call_failed(what: &str, e: impl std::fmt::Display) -> ConnectorError {
    ConnectorError::new(codes::NAME_NOT_FOUND, format!("D-Bus {what} 失败：{e}"))
}

/// `Open()` 失败 → 名字服务错误码（spec/naming.md 第 12 节）。
fn open_failed(name: &str, e: zbus::Error) -> ConnectorError {
    let zbus::Error::MethodError(err_name, detail, _) = &e else {
        return ConnectorError::new(codes::ACTIVATION_DENIED, format!("调用 {name} 的 Open() 失败：{e}"));
    };
    let err_name = err_name.as_str();
    let detail = detail.clone().unwrap_or_default();
    let code = match err_name.strip_prefix("org.freedesktop.DBus.Error.").unwrap_or(err_name) {
        // 没有所有者也没有激活文件；激活文件指向的程序不存在 / 激活文件无效。
        "ServiceUnknown" | "NameHasNoOwner" | "Spawn.ExecFailed" | "Spawn.FileInvalid" | "Spawn.ServiceNotFound" => {
            codes::NAME_NOT_FOUND
        }
        "UnknownMethod" | "UnknownObject" | "UnknownInterface" => codes::NAME_NOT_FOUND,
        "AccessDenied" => codes::BIND_PERMISSION_DENIED,
        "LimitsExceeded" => codes::CHANNEL_LIMIT,
        "NoReply" | "Timeout" | "TimedOut" => codes::ACTIVATION_TIMEOUT,
        _ => codes::ACTIVATION_DENIED,
    };
    ConnectorError::new(code, format!("{name} 的 Open() 失败（{err_name}）：{detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_errors_map_to_naming_codes() {
        let cases = [
            ("org.freedesktop.DBus.Error.ServiceUnknown", codes::NAME_NOT_FOUND),
            ("org.freedesktop.DBus.Error.Spawn.ExecFailed", codes::NAME_NOT_FOUND),
            ("org.freedesktop.DBus.Error.Spawn.ChildExited", codes::ACTIVATION_DENIED),
            ("org.freedesktop.DBus.Error.AccessDenied", codes::BIND_PERMISSION_DENIED),
            ("org.freedesktop.DBus.Error.LimitsExceeded", codes::CHANNEL_LIMIT),
            ("org.freedesktop.DBus.Error.NoReply", codes::ACTIVATION_TIMEOUT),
            ("com.example.Other", codes::ACTIVATION_DENIED),
        ];
        for (name, code) in cases {
            let msg = zbus::message::Message::method_call("/x", "M")
                .and_then(|b| b.destination("com.example.X"))
                .and_then(|b| b.build(&()))
                .expect("消息");
            let err = zbus::Error::MethodError(
                zbus::names::OwnedErrorName::try_from(name).expect("错误名"),
                Some("细节".into()),
                msg,
            );
            assert_eq!(open_failed("dev.appmcp.App.x", err).code, code, "{name}");
        }
    }
}
