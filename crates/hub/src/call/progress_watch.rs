//! 调用进度（spec/hub-api.md 3.12）：按 callId 登记进度路由，合并后转发给调用方。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_protocol::ToolsProgressParams;
use tokio::sync::mpsc;

use crate::hub::{HubShared, ProgressRoute, lock};
use crate::progress::ProgressThrottle;

use super::ProgressSink;

/// 一次调用的进度接收与合并（[`HubShared::watch_progress`]）；丢弃时注销路由。
pub(crate) struct ProgressWatch {
    shared: Arc<HubShared>,
    call_id: String,
    token: u64,
    pub(super) rx: mpsc::UnboundedReceiver<ToolsProgressParams>,
    sink: ProgressSink,
    throttle: ProgressThrottle,
    started: tokio::time::Instant,
}

impl ProgressWatch {
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub(super) fn offer(&mut self, p: ToolsProgressParams) {
        let now = self.elapsed_ms();
        if let Some(u) = self.throttle.offer(p.into(), now) {
            let _ = self.sink.send(u);
        }
    }

    pub(super) fn flush(&mut self) {
        let now = self.elapsed_ms();
        if let Some(u) = self.throttle.flush(now) {
            let _ = self.sink.send(u);
        }
    }

    /// 暂存的进度需要转发的时刻。
    pub(super) fn flush_deadline(&self) -> Option<tokio::time::Instant> {
        self.throttle.flush_at().map(|ms| self.started + Duration::from_millis(ms))
    }
}

impl Drop for ProgressWatch {
    fn drop(&mut self) {
        let mut routes = lock(&self.shared.progress_routes);
        if routes.get(&self.call_id).is_some_and(|r| r.token == self.token) {
            routes.remove(&self.call_id);
        }
    }
}

impl HubShared {
    /// 登记一次调用的进度路由：此后该连接发来的同 callId `tools/progress` 经合并后发往 `sink`；返回值被丢弃时注销。
    pub(super) fn watch_progress(self: &Arc<Self>, call_id: &str, conn_id: u64, sink: ProgressSink) -> ProgressWatch {
        let (tx, rx) = mpsc::unbounded_channel();
        let token = self.next_id();
        lock(&self.progress_routes).insert(call_id.to_owned(), ProgressRoute { token, conn_id, tx });
        ProgressWatch {
            shared: self.clone(),
            call_id: call_id.to_owned(),
            token,
            rx,
            sink,
            throttle: ProgressThrottle::new(u64::try_from(self.config.progress_interval.as_millis()).unwrap_or(u64::MAX)),
            started: tokio::time::Instant::now(),
        }
    }

    /// App 连接发来的 `tools/progress`（spec/protocol.md 3.3）：记到调用对象上（[`crate::call_objects`]），并交给等待该调用进度的一方；
    /// 调用未登记进度（调用方没要进度、已结束）或来自其他连接时丢弃。
    pub(crate) fn route_progress(&self, conn_id: u64, p: ToolsProgressParams) {
        self.record_call_progress(conn_id, &p);
        let routes = lock(&self.progress_routes);
        match routes.get(&p.call_id) {
            Some(r) if r.conn_id == conn_id => {
                let _ = r.tx.send(p);
            }
            Some(_) => tracing::warn!(call_id = %p.call_id, "tools/progress 来自未处理该调用的连接，忽略"),
            None => tracing::trace!(call_id = %p.call_id, "tools/progress 没有接收方（未请求进度或调用已结束），忽略"),
        }
    }

}
