//! 列表变化通知、事件广播，以及通知订阅方（legacy MCP 会话、`subscriptions/listen` 流）的登记与移除。

use std::sync::atomic::Ordering;
use std::sync::Arc;

#[cfg(feature = "mcp-server")]
use app_mcp_protocol::{ErrorKind, ToolError};
#[cfg(feature = "mcp-server")]
use rmcp::{Peer, RoleServer};

#[cfg(feature = "mcp-server")]
use crate::task::Principal;
use crate::types::HubEvent;

use super::{HubShared, lock};

/// 一个已登记的 `subscriptions/listen` 流（[`HubShared::open_listen`]）；析构时移除该流及其资源订阅。
#[cfg(feature = "mcp-server")]
pub(crate) struct ListenRegistration {
    shared: Arc<HubShared>,
    id: u64,
}

#[cfg(feature = "mcp-server")]
impl Drop for ListenRegistration {
    fn drop(&mut self) {
        self.shared.remove_subscriber(self.id);
    }
}

impl HubShared {
    pub(crate) fn emit(&self, ev: HubEvent) {
        // 没有接收方时发送失败，忽略。
        let _ = self.events.send(ev);
    }

    pub(crate) fn mark_tools_changed(&self) {
        self.tools_rev.send_modify(|v| *v = v.wrapping_add(1));
        self.tools_dirty.store(true, Ordering::SeqCst);
        self.dirty.notify_one();
    }

    pub(crate) fn mark_resources_changed(&self) {
        self.resources_dirty.store(true, Ordering::SeqCst);
        self.dirty.notify_one();
    }

    /// 合并 `list_changed_debounce` 内的多次变化，发一次事件并通知所有订阅方（legacy 会话与 listen 流）。
    pub(super) async fn notify_loop(self: Arc<Self>) {
        loop {
            self.dirty.notified().await;
            tokio::time::sleep(self.config.list_changed_debounce).await;
            let tools = self.tools_dirty.swap(false, Ordering::SeqCst);
            let resources = self.resources_dirty.swap(false, Ordering::SeqCst);
            if !tools && !resources {
                continue;
            }
            if tools {
                self.emit(HubEvent::ToolsChanged);
            }
            if resources {
                self.emit(HubEvent::ResourcesChanged);
            }
            let subscribers = lock(&self.subscribers).all();
            for (id, subscriber) in subscribers {
                if !subscriber.notify_lists_changed(tools, resources).await {
                    tracing::debug!(subscriber = id, "MCP 订阅方已关闭，移除");
                    self.remove_subscriber(id);
                }
            }
        }
    }

    #[cfg(feature = "mcp-server")]
    pub(crate) fn register_session(&self, id: u64, peer: Peer<RoleServer>) {
        lock(&self.subscribers).insert_session(id, peer);
    }

    /// 已初始化的 MCP 会话数。
    pub(crate) fn mcp_session_count(&self) -> usize {
        lock(&self.subscribers).session_count()
    }

    /// 登记一个 `subscriptions/listen` 流并订阅其接受的资源；返回的守卫析构时移除该流及其资源订阅（所有结束路径，S-06）。
    ///
    /// @error 该主体的流数已达 [`HubConfig::max_listen_streams`] → `RATE_LIMITED`。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn open_listen(
        self: &Arc<Self>,
        sink: rmcp::service::SubscriptionSink,
        principal: Principal,
    ) -> Result<ListenRegistration, ToolError> {
        let id = self.next_id();
        let uris = sink.accepted().resource_subscriptions.clone().unwrap_or_default();
        let caller = crate::task::CallerKey::principal(&principal);
        lock(&self.subscribers)
            .try_insert_listen(id, sink, principal, self.config.max_listen_streams)
            .map_err(|e| {
                ToolError::new(
                    ErrorKind::RateLimited,
                    format!("同时打开的 subscriptions/listen 流已达上限（{} 个），请先关闭不用的流。", e.max),
                )
            })?;
        let registration = ListenRegistration { shared: self.clone(), id };
        for uri in uris {
            // 接受过滤器时已按同样的规则筛过（McpSession::accepted_subscription_filter）；这里失败只可能是其间策略变化。
            if let Err(e) = self.subscribe(id, &uri, &caller) {
                tracing::debug!(%uri, error = %e.message, "listen 流的资源订阅未建立");
            }
        }
        Ok(registration)
    }

    /// 没有任何资源订阅（测试检查订阅方移除后的清理）。
    #[cfg(all(test, feature = "mcp-server"))]
    pub(crate) fn has_no_resource_subscriptions(&self) -> bool {
        lock(&self.resource_subs).is_empty()
    }

    /// 等到 Hub 开始停止（listen 流据此正常结束）。
    #[cfg(feature = "mcp-server")]
    pub(crate) async fn closing(&self) {
        let mut rx = self.closing.subscribe();
        let _ = rx.wait_for(|closing| *closing).await;
    }

    pub(super) fn begin_closing(&self) {
        self.closing.send_replace(true);
    }

    /// 移除订阅方（legacy 会话或 listen 流）及其资源订阅。
    pub(crate) fn remove_subscriber(self: &Arc<Self>, id: u64) {
        lock(&self.subscribers).remove(id);
        let uris: Vec<String> = lock(&self.resource_subs)
            .iter()
            .filter(|(_, s)| s.contains(&id))
            .map(|(u, _)| u.clone())
            .collect();
        for uri in uris {
            self.unsubscribe(id, &uri);
        }
    }

}
