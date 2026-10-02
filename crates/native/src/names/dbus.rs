//! Linux：D-Bus 会话总线上的名字登记（spec/naming.md 4.1）。
//!
//! 名字 `dev.appmcp.App.<appId>`（默认）与 `dev.appmcp.App.<appId>.<instance>`（实例），接口 `dev.appmcp.App1`，
//! 方法 `Open() → h`：核对调用方 uid 后创建 socketpair，一端交给运行时、另一端作为 UNIX_FD 返回；之后消息走 fd，
//! 不经总线转发。

use std::sync::Arc;

use app_mcp_protocol::naming::{Address, dbus as names};
use zbus::fdo::{self, DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Header;

use super::{ChannelSink, NameRequest, NameServer, RegisterFuture, Refusal, Registration};

/// 激活时 dbus-daemon 设置的总线地址（指向激活本进程的那条总线）。
const STARTER_ADDRESS_ENV: &str = "DBUS_STARTER_ADDRESS";

pub(crate) struct DbusNameServer;

impl NameServer for DbusNameServer {
    fn register<'a>(&'a self, request: &'a NameRequest, sink: Arc<dyn ChannelSink>) -> RegisterFuture<'a> {
        Box::pin(async move { register(request, sink).await.map(|r| Box::new(r) as Box<dyn Registration>) })
    }
}

struct DbusRegistration {
    /// @invariant 持有连接即持有名字；被丢弃时连接关闭，总线释放名字。
    _conn: zbus::Connection,
    names: Vec<String>,
}

impl Registration for DbusRegistration {
    fn names(&self) -> Vec<String> {
        self.names.clone()
    }
}

/// 总线连接的构造器：显式地址 → 激活方地址（`DBUS_STARTER_ADDRESS`）→ 会话总线（`DBUS_SESSION_BUS_ADDRESS`）。
fn builder(address: Option<&str>) -> zbus::Result<zbus::connection::Builder<'static>> {
    let starter = std::env::var(STARTER_ADDRESS_ENV).ok().filter(|a| !a.is_empty());
    match address.map(str::to_owned).or(starter) {
        Some(a) => zbus::connection::Builder::address(a.as_str()),
        None => zbus::connection::Builder::session(),
    }
}

async fn register(request: &NameRequest, sink: Arc<dyn ChannelSink>) -> Result<DbusRegistration, String> {
    let default = Address::new(&request.app_id, None).map_err(|e| e.to_string())?;
    let instance = match &request.instance {
        Some(i) => Some(Address::new(&request.app_id, Some(i)).map_err(|e| e.to_string())?),
        None => None,
    };
    let mut b = builder(request.address.as_deref())
        .map_err(|e| format!("无法连接 D-Bus 会话总线：{e}"))?
        .serve_at(names::object_path(&default), AppObject { sink: sink.clone() })
        .map_err(|e| e.to_string())?;
    if let Some(inst) = &instance {
        b = b.serve_at(names::object_path(inst), AppObject { sink }).map_err(|e| e.to_string())?;
    }
    let conn = b.build().await.map_err(|e| format!("无法连接 D-Bus 会话总线：{e}"))?;

    let mut owned = Vec::new();
    // 默认名字：第一个进程拥有；已被同一 App 的其他进程拥有时只登记实例名字（spec/naming.md 4.1）。
    let default_name = names::bus_name(&default);
    match request_name(&conn, &default_name).await? {
        true => owned.push(default_name),
        false if instance.is_none() => {
            return Err(format!("D-Bus 名字 {default_name} 已被其他进程占用，未登记"));
        }
        false => {}
    }
    if let Some(inst) = &instance {
        let name = names::bus_name(inst);
        if !request_name(&conn, &name).await? {
            return Err(format!("D-Bus 实例名字 {name} 已被其他进程占用，未登记"));
        }
        owned.push(name);
    }
    Ok(DbusRegistration { _conn: conn, names: owned })
}

/// 以 `DoNotQueue` 请求名字：成为（或已是）主所有者时为 `true`，已被占用时为 `false`。
async fn request_name(conn: &zbus::Connection, name: &str) -> Result<bool, String> {
    match conn.request_name_with_flags(name, RequestNameFlags::DoNotQueue.into()).await {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(true),
        Ok(_) => Ok(false),
        Err(zbus::Error::NameTaken) => Ok(false),
        Err(e) => Err(format!("请求 D-Bus 名字 {name} 失败：{e}")),
    }
}

/// 导出在 `/dev/appmcp/App[/<inst>]` 上的对象。
struct AppObject {
    sink: Arc<dyn ChannelSink>,
}

#[zbus::interface(name = "dev.appmcp.App1")]
impl AppObject {
    /// 打开一条通道：返回 socketpair 的一端（UNIX_FD）。
    ///
    /// @security 只接受同一用户的调用方（`GetConnectionUnixUser`），否则 `AccessDenied`（spec/naming.md 4.1、10.1）。
    /// @error 已有连接 / 通道 → `org.freedesktop.DBus.Error.LimitsExceeded`（Hub 记 `CHANNEL_LIMIT`）；客户端已停止 → `Failed`。
    async fn open(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> fdo::Result<zbus::zvariant::OwnedFd> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("无法确定调用方".to_owned()))?
            .to_owned();
        let uid = DBusProxy::new(conn).await?.get_connection_unix_user(sender.clone().into()).await?;
        let me = app_mcp_protocol::endpoint::current_uid();
        if uid != me {
            return Err(fdo::Error::AccessDenied(format!("调用方 {sender} 属于其他用户（uid {uid}），拒绝打开通道")));
        }
        let (mine, theirs) = std::os::unix::net::UnixStream::pair()
            .map_err(|e| fdo::Error::IOError(format!("无法创建 socketpair：{e}")))?;
        match self.sink.offer(mine) {
            Ok(()) => Ok(std::os::fd::OwnedFd::from(theirs).into()),
            Err(Refusal::Busy) => Err(fdo::Error::LimitsExceeded(format!(
                "{}：App 已有连接，同时只接受一条通道",
                app_mcp_protocol::naming::codes::CHANNEL_LIMIT
            ))),
            Err(e @ (Refusal::Stopped | Refusal::Invalid(_))) => Err(fdo::Error::Failed(e.to_string())),
        }
    }
}
