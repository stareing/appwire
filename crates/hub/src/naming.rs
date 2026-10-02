//! 按名寻址的 Hub 侧（spec/naming.md）：名字服务发现记录的维护、按名拨号与拨入通道的移交。
//!
//! - 发现（第 5 节）：每个连接器一个任务——先订阅名字事件，再扫描一次，之后只随事件更新；无轮询、不激活。
//! - 拨号（第 3.1 节）：由唤醒路径（[`HubShared::wake_and_wait`]）在没有活连接时调用；通道交给 App 连接服务，
//!   SDK 在其上先发 `app/hello`，就绪后认领这次拨号的待派调用。
//! - 关闭（第 7 节）：通道宽限后由会话关闭（[`crate::app_server`]），实例转为休眠快照，Hub 不再持有任何活引用。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use app_mcp_protocol::naming::{Address, codes};
use app_mcp_protocol::{ErrorKind, ToolError, user_action_reason};
use futures::StreamExt;
use serde_json::json;

use crate::connector::{Connector, ConnectorError, DiscoveredName, NameEvent};
use crate::hub::HubShared;
use crate::registry::NamedEntry;

impl HubShared {
    /// 按名拨号的路由（spec/naming.md 9.2）：发现记录中该 App 可激活或正在运行时，返回连接器与默认地址。
    pub(crate) fn named_route(&self, app_id: &str) -> Option<(Arc<dyn Connector>, Address)> {
        let index = self.registry().named(app_id).filter(|n| n.activatable || n.running)?.connector;
        let connector = self.config.connectors.get(index)?.clone();
        let address = Address::new(app_id, None).ok()?;
        Some((connector, address))
    }

    /// 某连接器的发现任务（Hub 运行期间）：订阅事件 → 扫描一次 → 随事件更新。事件流结束（总线断开）即结束，不重试。
    pub(crate) async fn naming_loop(self: Arc<Self>, index: usize) {
        let Some(connector) = self.config.connectors.get(index).cloned() else { return };
        let kind = connector.kind();
        // @why 先订阅再扫描：两者之间出现的名字由事件补上，不会漏。
        let events = match connector.watch().await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(connector = kind, error = %e, "无法订阅名字服务事件，只做启动扫描");
                None
            }
        };
        match connector.discover().await {
            Ok(names) => self.apply_discovered(index, kind, names),
            Err(e) => tracing::warn!(connector = kind, error = %e, "名字服务发现失败"),
        }
        let Some(mut events) = events else { return };
        while let Some(event) = events.next().await {
            self.apply_name_event(index, kind, event);
        }
        tracing::info!(connector = kind, "名字服务事件流结束");
    }

    /// 启动扫描的结果登记为发现记录（按 appId 合并默认名字与实例名字）。
    fn apply_discovered(&self, index: usize, kind: &str, names: Vec<DiscoveredName>) {
        let mut apps: BTreeMap<String, NamedEntry> = BTreeMap::new();
        let now = SystemTime::now();
        for n in names {
            let e = apps.entry(n.address.app_id.clone()).or_insert_with(|| NamedEntry {
                source: kind.to_owned(),
                connector: index,
                detail: String::new(),
                activatable: false,
                running: false,
                first_seen: now,
                last_seen: now,
                install_manifest: false,
            });
            e.activatable |= n.activatable;
            e.running |= n.running;
            if n.address.instance.is_none() {
                e.detail = n.detail;
            }
        }
        let mut changed = false;
        for (app_id, mut entry) in apps {
            if entry.detail.is_empty() {
                entry.detail = default_detail(kind, &app_id);
            }
            tracing::info!(app_id, connector = kind, activatable = entry.activatable, running = entry.running, "名字服务发现 App");
            changed |= self.record_named(index, &app_id, entry);
        }
        if changed {
            self.mark_tools_changed();
        }
    }

    /// 记下发现记录；连接器带有安装元数据中的清单时一并登记（未运行的 App 也按清单列出工具，spec/naming.md 5.6）。
    fn record_named(&self, index: usize, app_id: &str, mut entry: NamedEntry) -> bool {
        let manifest = self.config.connectors.get(index).and_then(|c| c.manifest(app_id));
        entry.install_manifest = manifest.is_some();
        let mut registry = self.registry();
        let mut changed = registry.set_named(app_id, Some(entry));
        if let Some(m) = manifest {
            registry.set_manifest(m);
            changed = true;
        }
        changed
    }

    /// 卸载事件：移除发现记录，以及来自安装元数据的清单（spec/naming.md 5.4）。
    fn forget_named(&self, app_id: &str) -> bool {
        let mut registry = self.registry();
        let install_manifest = registry.named(app_id).is_some_and(|n| n.install_manifest);
        let mut changed = registry.set_named(app_id, None);
        if install_manifest {
            changed |= registry.clear_manifest(app_id);
        }
        changed
    }

    fn apply_name_event(&self, index: usize, kind: &str, event: NameEvent) {
        let (address, running) = match event {
            NameEvent::Appeared(a) => (a, true),
            // 实例名字消失不代表 App 不再运行（默认名字可能仍在）：只跟踪默认名字的消失。
            NameEvent::Vanished(a) if a.instance.is_some() => return,
            NameEvent::Vanished(a) => (a, false),
            NameEvent::Installed(name) => return self.apply_installed(index, kind, name),
            NameEvent::Removed(a) => {
                tracing::info!(app_id = a.app_id, connector = kind, "名字服务：App 已卸载");
                if self.forget_named(&a.app_id) {
                    self.mark_tools_changed();
                }
                return;
            }
        };
        let app_id = address.app_id.clone();
        let make = || {
            let now = SystemTime::now();
            NamedEntry {
                source: kind.to_owned(),
                connector: index,
                detail: default_detail(kind, &app_id),
                activatable: false,
                running: true,
                first_seen: now,
                last_seen: now,
                install_manifest: false,
            }
        };
        let changed = self.registry().set_named_running(&app_id, running, make);
        tracing::debug!(app_id, running, "名字服务事件");
        if changed {
            self.mark_tools_changed();
        }
    }

    /// 安装 / 更新事件：与启动扫描同样登记（首次见到时间沿用旧记录）。
    fn apply_installed(&self, index: usize, kind: &str, name: DiscoveredName) {
        if name.address.instance.is_some() {
            return;
        }
        let now = SystemTime::now();
        let app_id = name.address.app_id.clone();
        let entry = NamedEntry {
            source: kind.to_owned(),
            connector: index,
            detail: if name.detail.is_empty() { default_detail(kind, &app_id) } else { name.detail },
            activatable: name.activatable,
            running: name.running,
            first_seen: now,
            last_seen: now,
            install_manifest: false,
        };
        tracing::info!(app_id, connector = kind, "名字服务：App 已安装 / 更新");
        if self.record_named(index, &app_id, entry) {
            self.mark_tools_changed();
        }
    }

    /// 按名拨号并把通道交给 App 连接服务（spec/naming.md 3.1）。系统确认未安装时移除发现记录（5.4）。
    pub(crate) async fn dial_and_serve(self: &Arc<Self>, connector: Arc<dyn Connector>, address: Address) -> Result<(), ToolError> {
        let app_id = address.app_id.clone();
        match connector.dial(&address, self.config.wake_timeout).await {
            Ok(channel) => {
                tracing::info!(app_id, %address, pid = ?channel.pid, "按名拨号成功");
                tokio::spawn(crate::app_server::handle_dialed_channel(self.clone(), channel));
                Ok(())
            }
            Err(e) => {
                if e.code == codes::NAME_NOT_FOUND && self.registry().set_named(&app_id, None) {
                    self.mark_tools_changed();
                }
                Err(dial_error(&app_id, &e))
            }
        }
    }
}

/// 没有默认名字的记录（只见到实例名字）在日志与 `apps.list` 中的名字。
fn default_detail(kind: &str, app_id: &str) -> String {
    match (kind, Address::new(app_id, None)) {
        ("dbus", Ok(a)) => app_mcp_protocol::naming::dbus::bus_name(&a),
        (_, Ok(a)) => a.to_string(),
        (_, Err(_)) => app_id.to_owned(),
    }
}

/// 拨号失败 → 工具错误（spec/naming.md 第 12 节末段）：确认未安装为 `APP_NOT_INSTALLED`；系统拦截已安装的目标
/// （`ACTIVATION_BLOCKED`）为 `USER_ACTION_REQUIRED`（[`blocked_error`]）；其余为 `LAUNCH_FAILED`。`data.code` 为名字服务错误码。
pub(crate) fn dial_error(app_id: &str, e: &ConnectorError) -> ToolError {
    if e.code == codes::ACTIVATION_BLOCKED {
        return blocked_error(app_id, e);
    }
    let kind = if e.code == codes::NAME_NOT_FOUND { ErrorKind::AppNotInstalled } else { ErrorKind::LaunchFailed };
    ToolError::new(kind, format!("按名拨号 App「{app_id}」失败：{}", e.message))
        .with_details(json!({ "appId": app_id, "code": e.code }))
}

/// 系统阻止 Hub 启动目标 App（spec/protocol.md 第 4 节 `USER_ACTION_REQUIRED`，`reason: "os-permission"`）。
///
/// @security `message` 面向用户，只含应用名；宿主的内部说明（组件名等）只进日志（E-05）。
fn blocked_error(app_id: &str, e: &ConnectorError) -> ToolError {
    let (package_name, app_name) = match &e.blocked {
        Some(t) if !t.app_name.trim().is_empty() => (Some(t.package_name.as_str()), t.app_name.trim()),
        Some(t) => (Some(t.package_name.as_str()), app_id),
        None => (None, app_id),
    };
    tracing::warn!(app_id, code = e.code, detail = %e.message, "系统拒绝 Hub 启动 App，需用户在系统设置中放行");
    let mut details = json!({
        "reason": user_action_reason::OS_PERMISSION,
        "appId": app_id,
        "appName": app_name,
        "code": e.code,
    });
    if let (Some(pkg), Some(obj)) = (package_name.filter(|p| !p.is_empty()), details.as_object_mut()) {
        obj.insert("packageName".into(), json!(pkg));
    }
    ToolError::new(
        ErrorKind::UserActionRequired,
        format!("系统阻止了 AppWire Hub 启动『{app_name}』。请在系统设置中允许『{app_name}』自启动 / 关联启动后重试。"),
    )
    .with_details(details)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dial_errors_keep_naming_code() {
        let e = dial_error("shop", &ConnectorError::new(codes::NAME_NOT_FOUND, "x"));
        assert_eq!(e.kind, ErrorKind::AppNotInstalled);
        assert_eq!(e.details.as_ref().and_then(|d| d["code"].as_str()), Some("NAME_NOT_FOUND"));
        let e = dial_error("shop", &ConnectorError::new(codes::ACTIVATION_TIMEOUT, "x"));
        assert_eq!(e.kind, ErrorKind::LaunchFailed);
        assert_eq!(e.details.as_ref().and_then(|d| d["code"].as_str()), Some("ACTIVATION_TIMEOUT"));
        // 宿主未细分的拒绝仍是 LAUNCH_FAILED（不冒充需要用户操作）
        let e = dial_error("shop", &ConnectorError::new(codes::ACTIVATION_DENIED, "x"));
        assert_eq!(e.kind, ErrorKind::LaunchFailed);
    }

    #[test]
    fn blocked_target_becomes_user_action_required() {
        let target = crate::connector::BlockedTarget { package_name: "dev.example.shop".into(), app_name: "小店".into() };
        let internal = "系统拒绝绑定 ComponentInfo{dev.example.shop/dev.appmcp.android.ToolsService}";
        let e = dial_error("shop", &ConnectorError::blocked(target, internal));
        assert_eq!(e.kind, ErrorKind::UserActionRequired);
        assert_eq!(e.message, "系统阻止了 AppWire Hub 启动『小店』。请在系统设置中允许『小店』自启动 / 关联启动后重试。");
        assert!(!e.message.contains("ComponentInfo"), "内部信息不得出现在面向用户的消息中");
        assert_eq!(
            e.details,
            Some(json!({"reason": "os-permission", "appId": "shop", "appName": "小店",
                        "packageName": "dev.example.shop", "code": "ACTIVATION_BLOCKED"}))
        );
        let rpc: app_mcp_protocol::RpcError = e.into();
        assert_eq!(rpc.code, -32019);
    }

    #[test]
    fn blocked_without_label_falls_back_to_app_id() {
        let e = dial_error("shop", &ConnectorError::new(codes::ACTIVATION_BLOCKED, "x"));
        assert_eq!(e.kind, ErrorKind::UserActionRequired);
        assert!(e.message.contains("『shop』"), "{}", e.message);
        let d = e.details.unwrap_or_default();
        assert_eq!(d["reason"], "os-permission");
        assert!(d.get("packageName").is_none(), "{d}");
        let blank = crate::connector::BlockedTarget { package_name: "p".into(), app_name: "  ".into() };
        let e = dial_error("shop", &ConnectorError::blocked(blank, "x"));
        assert!(e.message.contains("『shop』"), "{}", e.message);
    }
}
