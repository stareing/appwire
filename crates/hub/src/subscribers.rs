//! 服务器主动通知的订阅方（spec/hub-api.md 3.6「通知」，docs/plans/12-mcp-stateless.md S7）。
//!
//! 订阅方按 Hub 内部的订阅方 ID（与 MCP 会话号同一序号空间，[`crate::hub::HubShared::next_id`]）寻址，两种：
//!
//! - **legacy 会话**：处理过 `initialize` 的连接在 `notifications/initialized` 时登记其 peer，会话结束时移除；
//!   列表变化与 `resources/subscribe` 订阅的资源变化经该 peer 发送（行为与 S7 之前相同）。
//! - **`subscriptions/listen` 流**（MCP 2026-07-28）：无会话请求的一个 listen 请求，订阅寿命以该请求为界——客户端关闭流
//!   （HTTP 断开 / `notifications/cancelled`）或 Hub 停止时结束并移除；只发送握手时接受的类别（rmcp [`SubscriptionSink`] 按
//!   接受的过滤器校验并加 `subscriptionId`）。每个主体同时打开的流数有上限（B-07，[`crate::HubConfig::max_listen_streams`]）。
//!
//! @why 一张表：列表变化、资源变化、断开清理对两种订阅方是同一套逻辑（P-04），只在"怎么发"上不同。

use std::collections::HashMap;

use rmcp::model::ResourceUpdatedNotificationParam;
#[cfg(feature = "mcp-server")]
use rmcp::service::SubscriptionSink;
use rmcp::{Peer, RoleServer};

#[cfg(feature = "mcp-server")]
use crate::task::Principal;

/// 一个订阅方。
#[derive(Clone)]
pub(crate) enum Subscriber {
    /// legacy MCP 会话的 peer。
    ///
    /// @why 无 MCP 出口（无 `mcp-server`）的构建不登记任何订阅方，表恒为空；表与通知路径仍保留，避免各调用点按 feature 分叉。
    #[cfg_attr(not(feature = "mcp-server"), allow(dead_code))]
    Session(Peer<RoleServer>),
    /// 一个 `subscriptions/listen` 流及其主体（上限按主体计）。只有 MCP 出口（feature `mcp-server`）会登记。
    #[cfg(feature = "mcp-server")]
    Listen { sink: SubscriptionSink, principal: Principal },
}

impl Subscriber {
    /// 发送列表变化通知（只发订阅方接受的类别）。
    ///
    /// @output `false` = 订阅方已关闭（调用方应移除）。
    pub(crate) async fn notify_lists_changed(&self, tools: bool, resources: bool) -> bool {
        match self {
            Subscriber::Session(peer) => {
                (!tools || peer.notify_tool_list_changed().await.is_ok())
                    && (!resources || peer.notify_resource_list_changed().await.is_ok())
            }
            #[cfg(feature = "mcp-server")]
            Subscriber::Listen { sink, .. } => {
                let accepted = sink.accepted();
                let tools = tools && accepted.tools_list_changed == Some(true);
                let resources = resources && accepted.resources_list_changed == Some(true);
                (!tools || sink.notify_tool_list_changed().await.is_ok())
                    && (!resources || sink.notify_resource_list_changed().await.is_ok())
            }
        }
    }

    /// 发送资源变化通知（调用方只对订阅了该 URI 的订阅方调用）。
    ///
    /// @output `false` = 订阅方已关闭（调用方应移除）。
    pub(crate) async fn notify_resource_updated(&self, uri: String) -> bool {
        match self {
            Subscriber::Session(peer) => peer.notify_resource_updated(ResourceUpdatedNotificationParam::new(uri)).await.is_ok(),
            #[cfg(feature = "mcp-server")]
            Subscriber::Listen { sink, .. } => sink.notify_resource_updated(uri).await.is_ok(),
        }
    }
}

/// 打开 listen 流被拒绝：该主体的流数已达上限。
#[cfg(feature = "mcp-server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListenLimitReached {
    pub max: usize,
}

/// 订阅方 ID → 订阅方。
#[derive(Default)]
pub(crate) struct SubscriberTable {
    map: HashMap<u64, Subscriber>,
}

impl SubscriberTable {
    #[cfg(feature = "mcp-server")]
    pub(crate) fn insert_session(&mut self, id: u64, peer: Peer<RoleServer>) {
        self.map.insert(id, Subscriber::Session(peer));
    }

    /// 登记一个 listen 流；该主体已有 `max` 个流时拒绝（`max = 0` 即不接受任何 listen 流）。
    ///
    /// @invariant 计数与插入在同一把锁内完成，并发打开不会超出上限。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn try_insert_listen(
        &mut self,
        id: u64,
        sink: SubscriptionSink,
        principal: Principal,
        max: usize,
    ) -> Result<(), ListenLimitReached> {
        if self.listen_count_of(&principal) >= max {
            return Err(ListenLimitReached { max });
        }
        self.map.insert(id, Subscriber::Listen { sink, principal });
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: u64) -> Option<Subscriber> {
        self.map.remove(&id)
    }

    /// legacy 会话的 peer（只对会话发送的通知用，如渐进暴露的单会话 `list_changed`）。
    pub(crate) fn session(&self, id: u64) -> Option<Peer<RoleServer>> {
        match self.map.get(&id) {
            Some(Subscriber::Session(peer)) => Some(peer.clone()),
            _ => None,
        }
    }

    /// 指定 ID 中仍登记的订阅方。
    pub(crate) fn pick(&self, ids: &[u64]) -> Vec<(u64, Subscriber)> {
        ids.iter().filter_map(|id| self.map.get(id).map(|s| (*id, s.clone()))).collect()
    }

    pub(crate) fn all(&self) -> Vec<(u64, Subscriber)> {
        self.map.iter().map(|(id, s)| (*id, s.clone())).collect()
    }

    pub(crate) fn session_count(&self) -> usize {
        self.map.values().filter(|s| matches!(s, Subscriber::Session(_))).count()
    }

    /// listen 流数（订阅方中会话以外的都是 listen 流）。
    pub(crate) fn listen_count(&self) -> usize {
        self.map.len() - self.session_count()
    }

    #[cfg(feature = "mcp-server")]
    fn listen_count_of(&self, principal: &Principal) -> usize {
        self.map.values().filter(|s| matches!(s, Subscriber::Listen { principal: p, .. } if p == principal)).count()
    }
}
