//! 单个 SDK 连接的发送端：发送通道 + 待响应请求表。
//!
//! 连接任务（见 [`crate::app_server`]）持有接收端并负责写 WebSocket；
//! 注册表与 MCP 侧通过 [`Connection`] 向 SDK 发请求 / 通知。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::Duration;

use app_mcp_protocol::{Message, RequestId, Response, RpcError};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot};

/// 写任务要处理的指令。
#[derive(Debug, PartialEq, Eq)]
pub enum Outgoing {
    /// 一条 JSON-RPC 文本消息。
    Text(String),
    /// 发送 Close 帧并结束。
    Close,
}

/// 请求失败的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    /// SDK 返回了 JSON-RPC 错误。
    Rpc(RpcError),
    /// 在等待时间内没有收到响应。
    Timeout,
    /// 连接已断开（或在等待期间断开）。
    Disconnected,
}

type Pending = HashMap<RequestId, oneshot::Sender<Result<Value, RpcError>>>;

/// 一个 SDK 连接。
#[derive(Debug)]
pub struct Connection {
    pub id: u64,
    tx: mpsc::UnboundedSender<Outgoing>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    /// 被新连接替换或被主动关闭时触发，通知连接任务退出。
    shutdown: Notify,
    /// 已路由到本连接、尚未完成的调用 / 资源读取数（`app/sleep` 据此拒绝）。
    inflight: AtomicUsize,
}

/// [`Connection::begin_work`] 的守卫：析构时计数减一。
#[derive(Debug)]
pub struct WorkGuard(std::sync::Arc<Connection>);

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Connection {
    pub fn new(id: u64) -> (std::sync::Arc<Self>, mpsc::UnboundedReceiver<Outgoing>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = Connection {
            id,
            tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            shutdown: Notify::new(),
            inflight: AtomicUsize::new(0),
        };
        (std::sync::Arc::new(conn), rx)
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        // 锁中毒只可能来自其他线程 panic；继续使用内部数据即可。
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 发送一条消息；连接已关闭时返回 `false`。
    pub fn send(&self, msg: &Message) -> bool {
        self.tx.send(Outgoing::Text(msg.to_json())).is_ok()
    }

    pub fn notify(&self, method: &str, params: Value) -> bool {
        self.send(&Message::notification(method, params))
    }

    /// 发出请求，返回请求 ID 与接收响应的通道。
    pub fn start_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(RequestId, oneshot::Receiver<Result<Value, RpcError>>), RequestError> {
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending().insert(id.clone(), tx);
        if !self.send(&Message::request(id.clone(), method, params)) {
            self.pending().remove(&id);
            return Err(RequestError::Disconnected);
        }
        Ok((id, rx))
    }

    /// 发出请求并等待响应，最多等待 `timeout`。
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let (id, rx) = self.start_request(method, params)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(RequestError::Rpc(e)),
            Ok(Err(_)) => Err(RequestError::Disconnected),
            Err(_) => {
                self.forget(&id);
                Err(RequestError::Timeout)
            }
        }
    }

    /// 放弃等待某个请求（超时或取消后调用）。
    pub fn forget(&self, id: &RequestId) {
        self.pending().remove(id);
    }

    /// 把响应交给等待方；未知 ID 返回 `false`。
    pub fn resolve(&self, resp: Response) -> bool {
        match self.pending().remove(&resp.id) {
            Some(tx) => {
                // 等待方可能已放弃（例如心跳 ping），忽略发送失败。
                let _ = tx.send(resp.outcome);
                true
            }
            None => false,
        }
    }

    /// 让所有等待中的请求以 `Disconnected` 结束。
    pub fn fail_all(&self) {
        self.pending().clear();
    }

    /// 请求连接任务关闭连接。
    pub fn close(&self) {
        let _ = self.tx.send(Outgoing::Close);
        self.shutdown.notify_one();
    }

    /// 等待关闭信号。
    pub async fn shutdown_requested(&self) {
        self.shutdown.notified().await;
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// 登记一个进行中的调用 / 读取，直到守卫被丢弃。
    pub fn begin_work(self: &std::sync::Arc<Self>) -> WorkGuard {
        self.inflight.fetch_add(1, Ordering::SeqCst);
        WorkGuard(self.clone())
    }

    /// 进行中的调用 / 读取数。
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn request_roundtrip_and_errors() {
        let (conn, mut rx) = Connection::new(1);
        let c2 = conn.clone();
        let task = tokio::spawn(async move {
            c2.request("ping", Value::Null, Duration::from_secs(5))
                .await
        });
        let Some(Outgoing::Text(text)) = rx.recv().await else {
            panic!("expected text")
        };
        let Message::Request(req) = Message::parse(&text).unwrap() else {
            panic!("expected request")
        };
        assert_eq!(req.method, "ping");
        assert!(conn.resolve(Response {
            id: req.id.clone(),
            outcome: Ok(json!({}))
        }));
        assert_eq!(task.await.unwrap(), Ok(json!({})));
        assert!(!conn.resolve(Response {
            id: req.id,
            outcome: Ok(json!({}))
        }));

        let c2 = conn.clone();
        let task =
            tokio::spawn(async move { c2.request("x", Value::Null, Duration::from_secs(5)).await });
        let _ = rx.recv().await;
        conn.fail_all();
        assert_eq!(task.await.unwrap(), Err(RequestError::Disconnected));
    }

    #[tokio::test]
    async fn request_timeout_forgets() {
        let (conn, _rx) = Connection::new(1);
        let r = conn
            .request("x", Value::Null, Duration::from_millis(10))
            .await;
        assert_eq!(r, Err(RequestError::Timeout));
        assert!(conn.pending().is_empty());
    }

    #[test]
    fn work_guard_counts() {
        let (conn, _rx) = Connection::new(1);
        let a = conn.begin_work();
        let b = conn.begin_work();
        assert_eq!(conn.inflight(), 2);
        drop(a);
        assert_eq!(conn.inflight(), 1);
        drop(b);
        assert_eq!(conn.inflight(), 0);
    }

    #[tokio::test]
    async fn closed_channel_is_disconnected() {
        let (conn, rx) = Connection::new(1);
        drop(rx);
        assert!(conn.is_closed());
        assert_eq!(
            conn.request("x", Value::Null, Duration::from_secs(1)).await,
            Err(RequestError::Disconnected)
        );
    }
}
