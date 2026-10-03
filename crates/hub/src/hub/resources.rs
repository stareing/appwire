//! 资源读取与订阅：转发 `resources/read`，维护订阅表并向 App 订阅 / 取消订阅。

use std::sync::Arc;

use app_mcp_protocol::{
    ErrorKind, ResourceInfo, ResourceSubscribeParams, ResourcesReadParams, ResourcesReadResult, ToolError, method,
};
use serde_json::Value;

use crate::types::HubEvent;

use super::{HubShared, lock, parse_resource_uri, request_error, resource_uri};

impl HubShared {
    // ------------------------------------------------------------------
    // 资源
    // ------------------------------------------------------------------

    pub(crate) async fn read_app_resource(
        self: &Arc<Self>,
        app_id: &str,
        name: &str,
        selected: Option<String>,
        caller: &crate::task::CallerKey,
    ) -> Result<(ResourceInfo, ResourcesReadResult), ToolError> {
        // 只有休眠实例提供该资源：先唤醒（spec/lifecycle.md §9）。
        let plan = self
            .registry()
            .wake_plan_resource(app_id, name, selected.as_deref());
        let selected = match plan {
            Some(plan) => {
                self.admit_wake(app_id, None, caller)?;
                Some(self.wake_and_wait(&plan, std::pin::pin!(std::future::pending::<()>())).await?)
            }
            None => selected,
        };
        let target = self
            .registry()
            .route_resource(app_id, name, selected.as_deref())?;
        let _work = target.conn.begin_work();
        let params = serde_json::to_value(ResourcesReadParams {
            name: name.to_owned(),
        })
        .unwrap_or(Value::Null);
        let v = target
            .conn
            .request(method::RESOURCES_READ, params, self.config.response_timeout)
            .await
            .map_err(|e| request_error(e, app_id))?;
        let result = serde_json::from_value::<ResourcesReadResult>(v.clone()).unwrap_or(
            ResourcesReadResult {
                contents: v,
                mime_type: None,
            },
        );
        Ok((target.resource, result))
    }

    /// 订阅资源（`session` 为订阅方 ID——legacy MCP 会话或 listen 流——或 [`API_SUBSCRIBER`]）。
    pub(crate) fn subscribe(self: &Arc<Self>, session: u64, uri: &str) -> Result<(), ToolError> {
        let Some((app_id, _)) = parse_resource_uri(uri) else {
            return Err(ToolError::new(
                ErrorKind::ResourceNotFound,
                format!("无法识别的资源 URI：{uri}"),
            ));
        };
        if self.app_hidden(app_id) {
            return Err(ToolError::new(ErrorKind::ResourceNotFound, format!("资源「{uri}」不存在")));
        }
        if crate::hub_state::is_hub_state_uri(uri) {
            return Err(ToolError::new(ErrorKind::InvalidInput, format!("Hub 状态资源「{uri}」不支持订阅，需要时直接读取。")));
        }
        lock(&self.resource_subs)
            .entry(uri.to_owned())
            .or_default()
            .insert(session);
        self.ensure_subscriptions(app_id);
        Ok(())
    }

    /// 取消订阅；没有订阅方时转发 `resources/unsubscribe`。
    pub(crate) fn unsubscribe(self: &Arc<Self>, session: u64, uri: &str) {
        let now_empty = {
            let mut subs = lock(&self.resource_subs);
            match subs.get_mut(uri) {
                Some(set) => {
                    set.remove(&session);
                    let empty = set.is_empty();
                    if empty {
                        subs.remove(uri);
                    }
                    empty
                }
                None => false,
            }
        };
        let Some((app_id, name)) = parse_resource_uri(uri) else {
            return;
        };
        if !now_empty {
            return;
        }
        let conns: Vec<_> = {
            let mut reg = self.registry();
            let holders: Vec<_> = reg
                .resource_holders(app_id, name)
                .into_iter()
                .filter(|(_, s)| *s)
                .collect();
            for (c, _) in &holders {
                reg.mark_subscribed(app_id, c.id, name, false);
            }
            holders.into_iter().map(|(c, _)| c).collect()
        };
        // 可能在会话析构（Drop）中调用，此时不一定处于 tokio 运行时内。
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for conn in conns {
            let params = serde_json::to_value(ResourceSubscribeParams {
                name: name.to_owned(),
            })
            .unwrap_or(Value::Null);
            let timeout = self.config.response_timeout;
            rt.spawn(async move {
                if let Err(e) = conn
                    .request(method::RESOURCES_UNSUBSCRIBE, params, timeout)
                    .await
                {
                    tracing::debug!(error = ?e, "resources/unsubscribe 失败");
                }
            });
        }
    }

    /// 让该 App 所有提供了已订阅资源的实例都处于订阅状态（连接、同步后调用）。
    pub(crate) fn ensure_subscriptions(self: &Arc<Self>, app_id: &str) {
        let names: Vec<String> = lock(&self.resource_subs)
            .keys()
            .filter_map(|uri| parse_resource_uri(uri))
            .filter(|(a, _)| *a == app_id)
            .map(|(_, n)| n.to_owned())
            .collect();
        let mut todo = Vec::new();
        {
            let mut reg = self.registry();
            for name in names {
                for (conn, subscribed) in reg.resource_holders(app_id, &name) {
                    if !subscribed {
                        reg.mark_subscribed(app_id, conn.id, &name, true);
                        todo.push((conn, name.clone()));
                    }
                }
            }
        }
        for (conn, name) in todo {
            let shared = self.clone();
            let app_id = app_id.to_owned();
            tokio::spawn(async move {
                let params = serde_json::to_value(ResourceSubscribeParams { name: name.clone() })
                    .unwrap_or(Value::Null);
                let r = conn
                    .request(
                        method::RESOURCES_SUBSCRIBE,
                        params,
                        shared.config.response_timeout,
                    )
                    .await;
                if let Err(e) = r {
                    tracing::warn!(app_id, resource = %name, error = ?e, "向 App 订阅资源失败");
                    shared
                        .registry()
                        .mark_subscribed(&app_id, conn.id, &name, false);
                }
            });
        }
    }

    /// SDK 报告资源内容变化：发事件，并通知订阅了该资源的订阅方（legacy 会话与 listen 流）。
    pub(crate) fn resource_updated(self: &Arc<Self>, app_id: &str, name: &str) {
        // 被 `hide` 隐藏的 App 的资源变化不通知（隐藏前建立的订阅也不再收到）。
        if self.app_hidden(app_id) {
            return;
        }
        let uri = resource_uri(app_id, name);
        self.emit(HubEvent::ResourceUpdated { uri: uri.clone() });
        let ids: Vec<u64> = lock(&self.resource_subs)
            .get(&uri)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        let subscribers = lock(&self.subscribers).pick(&ids);
        for (id, subscriber) in subscribers {
            let uri = uri.clone();
            let shared = self.clone();
            tokio::spawn(async move {
                if !subscriber.notify_resource_updated(uri).await {
                    shared.remove_subscriber(id);
                }
            });
        }
    }

}
